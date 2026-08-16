//! CPU-only H0 mechanism census over deterministic actual-store tile pairs.

#[path = "h0_pair_diagnostic_cpu.rs"]
mod cpu_replay;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use h3o::CellIndex;
use noise_compute::admin;
use noise_compute::compute::element::{LineLayer, LinePiece, H0_NODE_CAP};
use noise_compute::constants::RAILWAY_REACH_CEILING;
use noise_compute::propagation::geo::point_to_segment_full;
use noise_compute::propagation::h0_streaming_reduction::{
    collect_h0_semantic_candidates, reduce_h0, H0Reduction,
};
use noise_compute::propagation::streaming_reduction::{
    candidate_wedge_owns, range_ordered, source_frame_mlon, source_frame_vector_from_world,
    WedgeDecision,
};
use raster_reader::fused_tile_z13::TileBbox as RasterTileBbox;
use tile_painter::grid::{pixel_lat, pixel_lon, tile_bbox, TILE_PX};
use tile_painter::region_runner::tile_centre_r4;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::ObstacleData;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;

const SAMPLE_PIXELS: [(u32, u32); 16] = [
    (32, 32),
    (32, 160),
    (32, 288),
    (32, 479),
    (160, 32),
    (160, 160),
    (160, 288),
    (160, 479),
    (288, 32),
    (288, 160),
    (288, 288),
    (288, 479),
    (479, 32),
    (479, 160),
    (479, 288),
    (479, 479),
];

#[derive(Clone, Copy)]
enum Layer {
    Road,
    Rail,
}

impl Layer {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "road" => Ok(Self::Road),
            "rail" => Ok(Self::Rail),
            _ => bail!("layer must be road or rail, got {value:?}"),
        }
    }

    const fn line_layer(self) -> LineLayer {
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

    fn load_rows(self, h3r4: &Path, ring: &[u64], centre: CellIndex) -> Result<Vec<LineRow>> {
        let centre_latlon = h3o::LatLng::from(centre);
        let admin = admin::admin_for_latlng(centre_latlon.lat(), centre_latlon.lng());
        Ok(match self {
            Self::Road => RoadData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
            Self::Rail => RailData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
        })
    }
}

fn select_source(
    rows: &[LineRow],
    receiver_latitude: f64,
    receiver_longitude: f64,
) -> Option<(usize, &LineRow)> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let distance = point_to_segment_full(
                receiver_latitude,
                receiver_longitude,
                line.start_lat,
                line.start_lon,
                line.end_lat,
                line.end_lon,
            )
            .d_endpoint_m;
            (distance <= line.max_distance_m).then_some((index, line, distance))
        })
        .min_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| left.0.cmp(&right.0))
        })
        .map(|(index, line, _)| (index, line))
}

fn explain_zero(reduction: &H0Reduction, candidates: &[cpu_replay::RawCandidate]) -> &'static str {
    if candidates.is_empty() {
        return "EMPTY_CANDIDATE_STREAM";
    }
    let mut wedge_owned = false;
    let mut guarded = false;
    for entry in candidates {
        let candidate = entry.candidate;
        for node in reduction.nodes() {
            match candidate_wedge_owns(
                candidate.endpoint0_m(),
                candidate.endpoint1_m(),
                node.receiver_vector_m,
                candidate.near_f32(),
            ) {
                WedgeDecision::DoesNotOwn => {}
                WedgeDecision::Owns => {
                    wedge_owned = true;
                    assert!(
                        !range_ordered(node.node_distance_m, candidate.near_f32())
                            .expect("the reducer accepted finite geometry"),
                        "scalar oracle found an eligible node missing from the mask"
                    );
                }
                WedgeDecision::NearGuardedDegenerate => guarded = true,
                WedgeDecision::HardFault => unreachable!("the reducer would already have failed"),
            }
        }
    }
    if wedge_owned {
        "RANGE_REJECTED"
    } else if guarded {
        "ONLY_NEAR_GUARDED_DEGENERATES"
    } else {
        "NO_WEDGE_OWNED_A_NODE"
    }
}

fn main() -> Result<()> {
    let values: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        values.len() == 6,
        "usage: h0-actual-store-census H3R4_DIR OUTPUT_DIR Z X Y road|rail"
    );
    let h3r4 = PathBuf::from(&values[0]);
    let output = PathBuf::from(&values[1]);
    let zoom: u8 = values[2].parse().context("invalid Z")?;
    let tile_x: u32 = values[3].parse().context("invalid X")?;
    let tile_y: u32 = values[4].parse().context("invalid Y")?;
    let layer = Layer::parse(&values[5])?;
    ensure!(h3r4.is_dir(), "missing H3R4_DIR");
    fs::create_dir(&output)
        .with_context(|| format!("output must be a fresh directory: {}", output.display()))?;

    let r4 = tile_centre_r4(zoom, tile_x, tile_y).context("tile centre outside H3")?;
    let centre = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = centre
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let rows = layer.load_rows(&h3r4, &ring, centre)?;
    let barriers_data = BarrierData::load_for_r4s(&h3r4, &ring)?;
    let obstacle_data = ObstacleData::load_for_r4s(&h3r4, r4, &ring)?;
    let obstacle_set = obstacle_data
        .set()
        .context("actual-store census requires vector obstacles")?;
    let flat = noise_gpu::flatten_obstacles(obstacle_set);
    let bbox = tile_bbox(zoom, tile_x, tile_y);
    let barriers = barriers_data.for_tile(
        &RasterTileBbox {
            west_lon: bbox.west_lon,
            east_lon: bbox.east_lon,
            north_lat: bbox.north_lat,
            south_lat: bbox.south_lat,
        },
        layer.halo_m(),
    );

    let mut report = BufWriter::new(File::create(output.join("census.tsv"))?);
    writeln!(
        report,
        "pair\tpixel_y\tpixel_x\tsource_index\traw_multiplicity\tsemantic_count\tC_p\tN_p\tQ_p\tE_p\tA_p\tguarded\tzero_reason"
    )?;
    let mut distribution: BTreeMap<(u64, usize, usize), u64> = BTreeMap::new();
    let mut pair_count = 0_u64;
    let mut zero_count = 0_u64;
    let mut raw_total = 0_u64;
    for (sample_index, (pixel_y, pixel_x)) in SAMPLE_PIXELS.into_iter().enumerate() {
        ensure!(pixel_y < TILE_PX as u32 && pixel_x < TILE_PX as u32);
        let receiver_latitude = pixel_lat(&bbox, pixel_y);
        let receiver_longitude = pixel_lon(&bbox, pixel_x);
        let Some((source_index, source)) =
            select_source(&rows, receiver_latitude, receiver_longitude)
        else {
            writeln!(
                report,
                "{sample_index}\t{pixel_y}\t{pixel_x}\t-\t0\t0\t0\t0\t0\t0\t0\t0\tNO_IN_REACH_SOURCE"
            )?;
            continue;
        };
        let raw = cpu_replay::collect_raw_candidates(
            &flat,
            &barriers,
            source,
            receiver_latitude,
            receiver_longitude,
        )?;
        let source_mlon = source_frame_mlon(source.start_lat, source.end_lat)
            .map_err(|error| anyhow::anyhow!("source mlon: {error:?}"))?;
        let source0 = source_frame_vector_from_world(
            source.start_lat,
            source.start_lon,
            receiver_latitude,
            receiver_longitude,
            source_mlon,
        )
        .map_err(|error| anyhow::anyhow!("source start frame: {error:?}"))?;
        let source1 = source_frame_vector_from_world(
            source.end_lat,
            source.end_lon,
            receiver_latitude,
            receiver_longitude,
            source_mlon,
        )
        .map_err(|error| anyhow::anyhow!("source end frame: {error:?}"))?;
        let piece = LinePiece {
            start_m: [source0.x, source0.y],
            end_m: [source1.x, source1.y],
            emission_per_m: 1.0,
        };
        let reduction = reduce_h0(
            piece,
            layer.line_layer(),
            raw.iter().map(|entry| entry.candidate),
        )
        .map_err(|error| anyhow::anyhow!("H0 reduction: {error:?}"))?;
        ensure!(reduction.nodes().len() <= H0_NODE_CAP);
        ensure!(reduction.candidate_visit_count() == raw.len() as u64);
        let semantic =
            collect_h0_semantic_candidates(piece, raw.iter().map(|entry| entry.candidate))
                .map_err(|error| anyhow::anyhow!("S_pair construction: {error:?}"))?;
        let zero_reason = if reduction.any_node_is_admitted() {
            "-"
        } else {
            zero_count += 1;
            explain_zero(&reduction, &raw)
        };
        let c = reduction.candidate_visit_count();
        let n = reduction.nodes().len();
        let q = reduction.admitted_node_count();
        *distribution.entry((c, n, q)).or_default() += 1;
        raw_total += raw.len() as u64;
        pair_count += 1;
        writeln!(
            report,
            "{sample_index}\t{pixel_y}\t{pixel_x}\t{source_index}\t{}\t{}\t{c}\t{n}\t{q}\t0\t0\t{}\t{zero_reason}",
            raw.len(),
            semantic.records.len(),
            reduction.guarded_degenerate_candidate_count(),
        )?;
    }
    ensure!(pair_count > 0, "no sampled receiver had an in-reach source");
    report.flush()?;

    let mut summary = BufWriter::new(File::create(output.join("summary.txt"))?);
    writeln!(summary, "H0_ACTUAL_STORE_CENSUS=PASS")?;
    writeln!(summary, "tile=z{zoom}/{tile_x}/{tile_y}")?;
    writeln!(summary, "region_source_rows={}", rows.len())?;
    writeln!(summary, "barrier_rows={}", barriers.len())?;
    writeln!(summary, "sampled_pairs={pair_count}")?;
    writeln!(summary, "zero_masks={zero_count}")?;
    writeln!(summary, "raw_multiplicity={raw_total}")?;
    writeln!(summary, "E_p=0")?;
    writeln!(summary, "A_p=0")?;
    writeln!(summary, "node_cap={H0_NODE_CAP}")?;
    for ((c, n, q), count) in distribution {
        writeln!(summary, "distribution C={c} N={n} Q={q} pairs={count}")?;
    }
    summary.flush()?;
    Ok(())
}
