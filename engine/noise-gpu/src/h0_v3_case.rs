//! Shared exact-case loader for the CPU-only H0 V3 binaries.

#[path = "h0_pair_diagnostic_cpu.rs"]
mod cpu_replay;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use h3o::CellIndex;
use noise_compute::admin;
use noise_compute::compute::element::LineLayer;
use noise_compute::constants::RAILWAY_REACH_CEILING;
use noise_compute::propagation::h0_streaming_reduction::H0Candidate;
use noise_compute::propagation::h0_v3::{H0_V3_CASES, H0_V3_RAW_CANDIDATES_PER_PAIR_MAX};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::Barrier;
use raster_reader::fused_tile_z13::{default_batch_size, FusedTileZ13, TileBatch};
use raster_reader::RealRasters;
use serde::Serialize;
use tile_painter::h0_v3_tile_reference::H0V3TileError;
use tile_painter::region_runner::tile_centre_r4;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::ObstacleData;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepLayer {
    Road,
    Rail,
}

impl SweepLayer {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "road" => Ok(Self::Road),
            "rail" => Ok(Self::Rail),
            _ => bail!("layer must be road or rail, got {value:?}"),
        }
    }

    pub(super) const fn line_layer(self) -> LineLayer {
        match self {
            Self::Road => LineLayer::Road,
            Self::Rail => LineLayer::Rail,
        }
    }

    const fn halo_m(self) -> f64 {
        match self {
            Self::Road => 10_000.0,
            Self::Rail => RAILWAY_REACH_CEILING,
        }
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Road => "road",
            Self::Rail => "rail",
        }
    }

    fn load_rows(self, h3r4: &Path, ring: &[u64], centre: CellIndex) -> Result<Vec<LineRow>> {
        let centre = h3o::LatLng::from(centre);
        let admin = admin::admin_for_latlng(centre.lat(), centre.lng());
        Ok(match self {
            Self::Road => RoadData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
            Self::Rail => RailData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
        })
    }
}

pub(super) struct CaseArguments {
    pub h3r4: PathBuf,
    pub prepared: PathBuf,
    pub output: PathBuf,
    pub zoom: u8,
    pub tile_x: u32,
    pub tile_y: u32,
    pub layer: SweepLayer,
    pub case_name: String,
    pub case_index: usize,
}

pub(super) struct LoadedCase<'a> {
    pub arguments: &'a CaseArguments,
    pub tile: &'a FusedTileZ13,
    pub rows: &'a [LineRow],
    pub barriers: &'a [Barrier],
    pub obstacle_set: &'a ObstacleSet,
    pub flat: &'a noise_gpu::ObstacleFlat,
}

#[derive(Debug, Clone)]
pub(super) struct RunIdentity {
    pub hostname: String,
    pub cpu_model: String,
    pub rayon_num_threads: usize,
    pub binary_sha256: String,
}

impl LoadedCase<'_> {
    pub(super) fn collect_candidates(
        &self,
        line: &LineRow,
        receiver_latitude: f64,
        receiver_longitude: f64,
    ) -> Result<Vec<H0Candidate>> {
        cpu_replay::collect_h0_candidates(
            self.flat,
            self.barriers,
            line,
            receiver_latitude,
            receiver_longitude,
            H0_V3_RAW_CANDIDATES_PER_PAIR_MAX,
        )
    }
}

fn assert_registered_environment() -> Result<()> {
    ensure!(
        std::env::var("SURFACE_BUDGET_ETA").as_deref() == Ok("0"),
        "every fixed H0 V3 arm requires SURFACE_BUDGET_ETA=0"
    );
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else {
            continue;
        };
        let forbidden = key.starts_with("QM_ARC_")
            || key == "QM_SEG_SAMPLES"
            || key.starts_with("QM_OBSTACLES_")
            || key == "QM_VECTOR_BUILDINGS"
            || (key.starts_with("SURFACE_") && key != "SURFACE_BUDGET_ETA");
        ensure!(!forbidden, "unregistered H0 V3 tuning environment: {key}");
    }
    Ok(())
}

pub(super) fn run_identity() -> Result<RunIdentity> {
    let configured_threads: usize = std::env::var("RAYON_NUM_THREADS")
        .context("RAYON_NUM_THREADS must be set for an H0 V3 evidence arm")?
        .parse()
        .context("invalid RAYON_NUM_THREADS")?;
    ensure!(configured_threads > 0);
    let rayon_num_threads = rayon::current_num_threads();
    ensure!(
        rayon_num_threads == configured_threads,
        "Rayon pool has {rayon_num_threads} threads, expected {configured_threads}"
    );
    let hostname = fs::read_to_string("/etc/hostname")
        .context("cannot read host identity")?
        .trim()
        .to_owned();
    ensure!(!hostname.is_empty());
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").context("cannot read CPU identity")?;
    let cpu_model = cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name\t: "))
        .context("CPU model name missing from /proc/cpuinfo")?
        .to_owned();
    let executable = std::env::current_exe().context("cannot resolve current executable")?;
    let binary_sha256 = noise_gpu::h0_v3_field::sha256_file_hex(&executable)
        .map_err(|error| anyhow::anyhow!("cannot hash current executable: {error:?}"))?;
    ensure!(binary_sha256.len() == 64);
    Ok(RunIdentity {
        hostname,
        cpu_model,
        rayon_num_threads,
        binary_sha256,
    })
}

pub(super) fn parse_arguments() -> Result<CaseArguments> {
    assert_registered_environment()?;
    let values: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        values.len() == 8,
        "usage: fixed-arm-binary H3R4_DIR PREPARED_ROOT OUTPUT_DIR Z X Y road|rail CASE"
    );
    let h3r4 = PathBuf::from(&values[0]);
    let prepared = PathBuf::from(&values[1]);
    let output = PathBuf::from(&values[2]);
    let zoom: u8 = values[3].parse().context("invalid Z")?;
    let tile_x: u32 = values[4].parse().context("invalid X")?;
    let tile_y: u32 = values[5].parse().context("invalid Y")?;
    let layer = SweepLayer::parse(&values[6])?;
    let case_name = values[7].clone();
    ensure!(h3r4.is_dir(), "missing H3R4_DIR");
    ensure!(prepared.is_dir(), "missing PREPARED_ROOT");
    let case_index = H0_V3_CASES
        .iter()
        .position(|case| {
            case.name == case_name
                && case.zoom == zoom
                && case.x == tile_x
                && case.y == tile_y
                && case.layer == layer.line_layer()
        })
        .context("arguments are not one exact pre-registered H0 V3 case")?;
    Ok(CaseArguments {
        h3r4,
        prepared,
        output,
        zoom,
        tile_x,
        tile_y,
        layer,
        case_name,
        case_index,
    })
}

pub(super) fn with_loaded_case<T>(
    arguments: &CaseArguments,
    consume: impl FnOnce(LoadedCase<'_>) -> Result<T>,
) -> Result<T> {
    let r4 = tile_centre_r4(arguments.zoom, arguments.tile_x, arguments.tile_y)
        .context("tile centre outside H3")?;
    let centre = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = centre
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let rows = arguments.layer.load_rows(&arguments.h3r4, &ring, centre)?;
    let barrier_data = BarrierData::load_for_r4s(&arguments.h3r4, &ring)?;
    let obstacle_data = ObstacleData::load_for_r4s(&arguments.h3r4, r4, &ring)?;
    let obstacle_set = obstacle_data
        .set()
        .context("H0 V3 requires the vector obstacle store")?;
    let flat = noise_gpu::flatten_obstacles(obstacle_set);
    let rasters = RealRasters::new(&arguments.prepared);
    let batch_size = default_batch_size();
    let batch_x = (arguments.tile_x / batch_size) * batch_size;
    let batch_y = (arguments.tile_y / batch_size) * batch_size;
    let mut batch = TileBatch::build(
        arguments.zoom,
        batch_x,
        batch_y,
        batch_size,
        arguments.layer.halo_m(),
        &rasters,
    );
    let tile_index =
        ((arguments.tile_y - batch_y) * batch_size + arguments.tile_x - batch_x) as usize;
    tile_painter::source_loader_obstacle::bake_tile_vector_rx_refl(
        &mut batch.tiles[tile_index],
        obstacle_set,
    );
    let tile = &batch.tiles[tile_index];
    let barriers = barrier_data.for_tile(&tile.bbox, arguments.layer.halo_m());
    consume(LoadedCase {
        arguments,
        tile,
        rows: &rows,
        barriers: &barriers,
        obstacle_set,
        flat: &flat,
    })
}

pub(super) fn map_tile_error(error: H0V3TileError<anyhow::Error>) -> anyhow::Error {
    match error {
        H0V3TileError::CandidateStore(error) => error,
        H0V3TileError::Pair(error) => anyhow::anyhow!("H0 V3 pair failed: {error:?}"),
        H0V3TileError::SourceOrder => anyhow::anyhow!("H0 V3 source order changed"),
    }
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut output = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
