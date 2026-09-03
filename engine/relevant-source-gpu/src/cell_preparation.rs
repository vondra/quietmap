//! One streamed cell's sources on the host and then on the card: the arrows of
//! its `grid_disk(1)` ring encoded into device sources, its obstacles flattened,
//! and — for airport ground ops — the event calendar its own ring declares.

use std::collections::BTreeMap;
use std::time::Instant;

use anyhow::{Context, Result};
use h3o::{CellIndex, LatLng};
use noise_compute::admin;
use noise_compute::constants::ground_ops_max_radius;
use noise_compute::emission::aircraft::{ClassWeights, GROUND_OPS_SOURCE_HEIGHT_M};
use tile_painter::r4_source_cache::SourceSel;
use tile_painter::region_runner::region_tiles;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_building::BuildingData;
use tile_painter::source_loader_industrial::IndustrialData;
use tile_painter::source_loader_obstacle::ObstacleData;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::source_loader_traffic::AirportTrafficData;
use tile_painter::worklist::{resolve_class_weights, resolve_n_days};

use crate::cell_stream::StreamedCell;
use crate::obstacle_transfer::FlattenedObstacleGeometry;
use crate::relevant_source_runner::RelevantSourceRunConfiguration;
use crate::relevant_source_tile::{RegionDeviceLineSources, RegionDeviceObstacles};
use crate::source_frame::{
    source_identity_fingerprint, DeviceLineSource, RegionMetricFrame, BAND_COUNT, PERIOD_COUNT,
    SOURCE_FLAG_GROUND_OPS_AIRCRAFT, SOURCE_FLAG_GROUND_OPS_GSE,
};
use crate::surface_layers::{GROUND_OPS_LAYER, LAYER_NAMES, LAYER_SOURCE_IDS};

/// One layer's encoded sources, on the host and on the card.
pub struct EncodedLineLayer {
    pub layer: usize,
    pub directory_name: &'static str,
    pub source_id: u8,
    pub sources: Vec<DeviceLineSource>,
    pub fingerprint: u64,
    pub device_sources: RegionDeviceLineSources,
}

/// One cell's sources, obstacles and barriers loaded and encoded on the host,
/// nothing on the card yet.
pub struct HostPreparedRegion {
    pub region_r4: u64,
    pub tiles: Vec<(u32, u32)>,
    pub frame: RegionMetricFrame,
    pub layers: Vec<(usize, Vec<DeviceLineSource>)>,
    pub barrier_data: BarrierData,
    pub obstacle_data: ObstacleData,
    pub flattened_obstacles: FlattenedObstacleGeometry,
    /// Event days this cell's ground-ops energies are divided by.
    pub n_days: f64,
    /// Host seconds of the load and encode, before any permit wait.
    pub host_seconds: f64,
}

impl HostPreparedRegion {
    /// Put the cell on the card: called only once a residency permit is held.
    pub fn upload(self, permit_wait_seconds: f64) -> Result<PreparedRegion> {
        let upload_started = Instant::now();
        let device_obstacles = RegionDeviceObstacles::upload(&self.flattened_obstacles)?;
        let layers = self
            .layers
            .into_iter()
            .map(|(layer, sources)| encode_layer(layer, sources))
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedRegion {
            region_r4: self.region_r4,
            tiles: self.tiles,
            frame: self.frame,
            layers,
            barrier_data: self.barrier_data,
            obstacle_data: self.obstacle_data,
            device_obstacles,
            n_days: self.n_days,
            prepare_seconds: self.host_seconds + upload_started.elapsed().as_secs_f64(),
            permit_wait_seconds,
        })
    }
}

/// One cell's sources, obstacles and barriers loaded, encoded and resident on
/// the card, ready to paint: the unit the cell producer hands the painter.
pub struct PreparedRegion {
    pub region_r4: u64,
    pub tiles: Vec<(u32, u32)>,
    pub frame: RegionMetricFrame,
    pub layers: Vec<EncodedLineLayer>,
    pub barrier_data: BarrierData,
    pub obstacle_data: ObstacleData,
    pub device_obstacles: RegionDeviceObstacles,
    pub n_days: f64,
    pub prepare_seconds: f64,
    pub permit_wait_seconds: f64,
}

/// Load and encode exactly the layers this cell asked for. Every layer reads
/// the cell's own `grid_disk(1)` ring, so what a cell paints depends on its own
/// declared read set and on nothing else in the stream — the property that lets
/// two workers split one area and still produce the same bytes.
pub fn prepare_region(
    configuration: &RelevantSourceRunConfiguration,
    cell: &StreamedCell,
) -> Result<HostPreparedRegion> {
    let started = Instant::now();
    let region_r4 = cell.region_r4;
    let index = CellIndex::try_from(region_r4).context("invalid R4 region")?;
    let tiles = match cell.tile_window {
        Some(window) => window
            .select(region_tiles(region_r4, configuration.zoom))
            .context("select the requested tile window")?,
        None => region_tiles(region_r4, configuration.zoom),
    };
    let ring: Vec<u64> = index
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let frame = RegionMetricFrame::for_cell(index);
    let centre = LatLng::from(index);
    let region_admin = admin::admin_for_latlng(centre.lat(), centre.lng());
    let h3r4 = &configuration.h3r4_directory;
    let barrier_data = BarrierData::load_for_r4s(h3r4, &ring)?;
    let obstacle_data = ObstacleData::load_for_r4s(h3r4, region_r4, &ring)?;
    let flattened_obstacles = FlattenedObstacleGeometry::from_set(&frame, obstacle_data.set());
    let ground_ops = if cell.layers.contains(&GROUND_OPS_LAYER) {
        GroundOpsCalendar::resolve(h3r4, &ring)?
    } else {
        GroundOpsCalendar::silent()
    };
    let mut layers = Vec::with_capacity(cell.layers.len());
    for &layer in &cell.layers {
        let sources = match layer {
            0 => RoadData::load_for_r4s(h3r4, &ring, region_admin)?
                .into_rows()
                .iter()
                .map(|row| frame.encode_line(row))
                .collect(),
            1 => RailData::load_for_r4s(h3r4, &ring, region_admin)?
                .into_rows()
                .iter()
                .map(|row| frame.encode_line(row))
                .collect(),
            2 => IndustrialData::load_for_r4s(h3r4, &ring)?
                .into_rows()
                .iter()
                .map(|row| frame.encode_point(row))
                .collect(),
            3 => BuildingData::load_for_r4s(h3r4, &ring)?
                .into_rows()
                .iter()
                .map(|row| frame.encode_point(row))
                .collect(),
            GROUND_OPS_LAYER => encode_ground_ops_sources(
                &frame,
                &AirportTrafficData::load_for_r4s(h3r4, &ring)?,
                &ground_ops.class_weights,
            ),
            other => unreachable!("layer index {other} is outside {LAYER_NAMES:?}"),
        };
        layers.push((layer, sources));
    }
    Ok(HostPreparedRegion {
        region_r4,
        tiles,
        frame,
        layers,
        barrier_data,
        obstacle_data,
        flattened_obstacles,
        n_days: ground_ops.n_days,
        host_seconds: started.elapsed().as_secs_f64(),
    })
}

fn encode_layer(layer: usize, sources: Vec<DeviceLineSource>) -> Result<EncodedLineLayer> {
    let fingerprint = source_identity_fingerprint(&sources);
    let device_sources = RegionDeviceLineSources::upload(&sources)?;
    Ok(EncodedLineLayer {
        layer,
        directory_name: LAYER_NAMES[layer],
        source_id: LAYER_SOURCE_IDS[layer],
        sources,
        fingerprint,
        device_sources,
    })
}

/// One cell's ground-ops calendar: the n_days divisor and the GA class-weight
/// vector resolved from THIS cell's `grid_disk(1)` traffic arrows. Keyed on the
/// cell alone, never on the run (the CPU builder resolves one calendar over its
/// whole run's region union): a cell's bytes must not depend on which other
/// cells a worker happened to be given.
struct GroundOpsCalendar {
    n_days: f64,
    class_weights: ClassWeights,
}

impl GroundOpsCalendar {
    /// The calendar of a cell that paints no ground ops: nothing divides.
    fn silent() -> Self {
        Self {
            n_days: 1.0,
            class_weights: ClassWeights::uniform(),
        }
    }

    fn resolve(h3r4_directory: &std::path::Path, ring: &[u64]) -> Result<Self> {
        let has_traffic = ring.iter().any(|r4| {
            h3r4_directory
                .join(format!("{r4:015x}"))
                .join("airport_traffic.arrow")
                .exists()
        });
        if !has_traffic {
            return Ok(Self::silent());
        }
        let traffic = SourceSel {
            cruise: false,
            airborne: false,
            traffic: true,
        };
        let n_days = resolve_n_days(h3r4_directory, ring, traffic)?;
        let class_weights = resolve_class_weights(h3r4_directory, ring, traffic, n_days)?;
        Ok(Self {
            n_days: f64::from(n_days),
            class_weights,
        })
    }
}

/// One airport ground-ops microsegment's event energies summed per period and
/// vehicle kind (aircraft rows class-weighted), as the CPU prepare_microsegs does.
fn encode_ground_ops_sources(
    frame: &RegionMetricFrame,
    traffic: &AirportTrafficData,
    class_weights: &ClassWeights,
) -> Vec<DeviceLineSource> {
    struct Microsegment {
        start_lat: f64,
        start_lon: f64,
        end_lat: f64,
        end_lon: f64,
        length_m: f32,
        max_radius_m: f64,
        aircraft: [[f32; BAND_COUNT]; PERIOD_COUNT],
        gse: [[f32; BAND_COUNT]; PERIOD_COUNT],
        has_aircraft: bool,
        has_gse: bool,
    }
    let mut by_microsegment: BTreeMap<(u64, u16), Microsegment> = BTreeMap::new();
    for row in traffic.views() {
        let microsegment = by_microsegment
            .entry((row.osm_id, row.segment_idx))
            .or_insert_with(|| Microsegment {
                start_lat: f64::from(row.start_lat),
                start_lon: f64::from(row.start_lon),
                end_lat: f64::from(row.end_lat),
                end_lon: f64::from(row.end_lon),
                length_m: row.length_m,
                max_radius_m: ground_ops_max_radius(row.ops_kind),
                aircraft: [[0.0; BAND_COUNT]; PERIOD_COUNT],
                gse: [[0.0; BAND_COUNT]; PERIOD_COUNT],
                has_aircraft: false,
                has_gse: false,
            });
        let period = usize::from(row.period.min(2));
        if row.veh_kind == 0 {
            let weight = class_weights.get(row.class_idx) as f32;
            for band in 0..BAND_COUNT {
                microsegment.aircraft[period][band] += row.band_energy_lin[band] * weight;
            }
            microsegment.has_aircraft = true;
        } else {
            for band in 0..BAND_COUNT {
                microsegment.gse[period][band] += row.band_energy_lin[band];
            }
            microsegment.has_gse = true;
        }
    }
    let mut sources = Vec::new();
    for microsegment in by_microsegment.values() {
        for (present, flag, emission) in [
            (
                microsegment.has_aircraft,
                SOURCE_FLAG_GROUND_OPS_AIRCRAFT,
                &microsegment.aircraft,
            ),
            (
                microsegment.has_gse,
                SOURCE_FLAG_GROUND_OPS_GSE,
                &microsegment.gse,
            ),
        ] {
            if present {
                sources.push(frame.encode_ground_ops(
                    microsegment.start_lat,
                    microsegment.start_lon,
                    microsegment.end_lat,
                    microsegment.end_lon,
                    microsegment.length_m,
                    microsegment.max_radius_m,
                    GROUND_OPS_SOURCE_HEIGHT_M,
                    flag,
                    emission,
                ));
            }
        }
    }
    sources
}
