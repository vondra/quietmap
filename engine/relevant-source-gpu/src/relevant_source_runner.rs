//! Road/rail runner over one wave's zoom and phase/pair measurement for relevant-source painting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use h3o::{CellIndex, LatLng};
use noise_compute::admin;
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;
use tile_painter::region_runner::{batch_slot, block_batch_origin, region_tiles};
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::{bake_tile_vector_rx_refl, ObstacleData};
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::wire_hm3::{SOURCE_ID_RAIL, SOURCE_ID_ROAD};

use crate::cuda_bridge::RelevantSourceCuda;
use crate::obstacle_transfer::FlattenedObstacleGeometry;
use crate::relevant_source_tile::{
    partition_paint_and_write_tile, BatchDeviceRaster, RegionDeviceLineSources,
    RegionDeviceObstacles, TileDeviceReceivers, TilePaintMeasurement,
};
use crate::source_frame::{source_identity_fingerprint, DeviceLineSource, RegionMetricFrame};

const REGION_TILE_BATCH_SIDE: u32 = 4;
const LINE_HALO_M: f64 = 10_000.0;
const W1_ZOOM: u8 = 12;

/// The ray cadence each wave's etalon was painted with, as the production roles
/// build it: W1 (z12) runs the surface heatmap's coarse middle (scatter_band's
/// SHADOW_MID_STRIDE default), W2 (z13) the exact popup cadence
/// (SURFACE_SHADOW_STRIDE=1, the stride4 role's -DSHADOW_MID_STRIDE=1).
fn coarse_middle_cadence(zoom: u8) -> bool {
    zoom <= W1_ZOOM
}

pub struct RelevantSourceRunConfiguration {
    pub prepared_directory: PathBuf,
    pub h3r4_directory: PathBuf,
    pub output_directory: PathBuf,
    /// Web-Mercator zoom of the painted tiles: 12 for W1, 13 for W2.
    pub zoom: u8,
    pub regions: Vec<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct LayerMeasurement {
    pub loaded_sources: u64,
    pub tiles: u64,
    pub corner_pairs: u64,
    pub pixel_pairs: u64,
    pub relevant_source_references: u64,
    /// Relevant sources of every painted block, in paint order.
    pub block_source_counts: Vec<u32>,
    pub corner_gpu_milliseconds: f64,
    pub paint_gpu_milliseconds: f64,
    pub output_bytes: u64,
}

impl LayerMeasurement {
    fn add_tile(&mut self, tile: TilePaintMeasurement) {
        self.tiles += 1;
        self.corner_pairs += tile.corner_pairs;
        self.pixel_pairs += tile.pixel_pairs;
        self.relevant_source_references += tile.relevant_source_references;
        self.block_source_counts.extend(tile.block_source_counts);
        self.corner_gpu_milliseconds += tile.corner_gpu_milliseconds;
        self.paint_gpu_milliseconds += tile.paint_gpu_milliseconds;
        self.output_bytes += tile.output_bytes;
    }

    /// `(min, median, p99, max)` of relevant sources per block.
    pub fn block_source_quantiles(&self) -> (u32, u32, u32, u32) {
        let mut counts = self.block_source_counts.clone();
        if counts.is_empty() {
            return (0, 0, 0, 0);
        }
        counts.sort_unstable();
        let at = |fraction: f64| counts[((counts.len() - 1) as f64 * fraction).round() as usize];
        (counts[0], at(0.5), at(0.99), counts[counts.len() - 1])
    }
}

#[derive(Debug, Default)]
pub struct RelevantSourceRunMeasurement {
    pub road: LayerMeasurement,
    pub rail: LayerMeasurement,
    pub wall_seconds: f64,
    pub cpu_seconds: f64,
    pub source_load_seconds: f64,
    pub raster_and_receiver_seconds: f64,
    pub host_tile_seconds: f64,
}

impl RelevantSourceRunMeasurement {
    pub fn gpu_seconds(&self) -> f64 {
        (self.road.corner_gpu_milliseconds
            + self.road.paint_gpu_milliseconds
            + self.rail.corner_gpu_milliseconds
            + self.rail.paint_gpu_milliseconds)
            / 1000.0
    }

    pub fn attempted_pairs(&self) -> u64 {
        self.road.corner_pairs
            + self.road.pixel_pairs
            + self.rail.corner_pairs
            + self.rail.pixel_pairs
    }
}

struct EncodedLineLayer {
    directory_name: &'static str,
    source_id: u8,
    sources: Vec<DeviceLineSource>,
    fingerprint: u64,
    device_sources: RegionDeviceLineSources,
}

pub fn run_relevant_source_wave(
    configuration: &RelevantSourceRunConfiguration,
) -> Result<RelevantSourceRunMeasurement> {
    let started = Instant::now();
    let starting_usage = ProcessUsage::read();
    admin::init_admin_table(&admin::default_admin_path(&configuration.h3r4_directory))
        .context("load the road/rail admin table")?;
    let rasters = RealRasters::new(&configuration.prepared_directory);
    let cuda = RelevantSourceCuda::initialize()?;
    let mut measurement = RelevantSourceRunMeasurement::default();

    for &region_r4 in &configuration.regions {
        process_region(configuration, region_r4, &rasters, &cuda, &mut measurement)?;
    }
    measurement.wall_seconds = started.elapsed().as_secs_f64();
    measurement.cpu_seconds = ProcessUsage::read().seconds() - starting_usage.seconds();
    Ok(measurement)
}

fn process_region(
    configuration: &RelevantSourceRunConfiguration,
    region_r4: u64,
    rasters: &RealRasters,
    cuda: &RelevantSourceCuda,
    measurement: &mut RelevantSourceRunMeasurement,
) -> Result<()> {
    let cell = CellIndex::try_from(region_r4).context("invalid R4 region")?;
    let zoom = configuration.zoom;
    let tiles = region_tiles(region_r4, zoom);
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let frame = RegionMetricFrame::for_cell(cell);

    let load_started = Instant::now();
    let centre = LatLng::from(cell);
    let region_admin = admin::admin_for_latlng(centre.lat(), centre.lng());
    let road_rows =
        RoadData::load_for_r4s(&configuration.h3r4_directory, &ring, region_admin)?.into_rows();
    let rail_rows =
        RailData::load_for_r4s(&configuration.h3r4_directory, &ring, region_admin)?.into_rows();
    let barrier_data = BarrierData::load_for_r4s(&configuration.h3r4_directory, &ring)?;
    let obstacle_data =
        ObstacleData::load_for_r4s(&configuration.h3r4_directory, region_r4, &ring)?;
    let flattened_obstacles = FlattenedObstacleGeometry::from_set(&frame, obstacle_data.set());
    let device_obstacles = RegionDeviceObstacles::upload(&flattened_obstacles)?;
    let road = encode_line_layer("road", SOURCE_ID_ROAD, &frame, &road_rows)?;
    let rail = encode_line_layer("rail", SOURCE_ID_RAIL, &frame, &rail_rows)?;
    measurement.road.loaded_sources += road.sources.len() as u64;
    measurement.rail.loaded_sources += rail.sources.len() as u64;
    measurement.source_load_seconds += load_started.elapsed().as_secs_f64();
    let layers = [road, rail];

    let mut batches: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(x, y) in &tiles {
        batches
            .entry((
                x / REGION_TILE_BATCH_SIDE * REGION_TILE_BATCH_SIDE,
                y / REGION_TILE_BATCH_SIDE * REGION_TILE_BATCH_SIDE,
            ))
            .or_default()
            .push((x, y));
    }
    for ((block_x, block_y), requested_tiles) in batches {
        let raster_started = Instant::now();
        let (base_x, base_y) = block_batch_origin(block_x, block_y, REGION_TILE_BATCH_SIDE, zoom);
        let mut batch = TileBatch::build_opt_rx_refl(
            zoom,
            base_x,
            base_y,
            REGION_TILE_BATCH_SIDE,
            LINE_HALO_M,
            rasters,
        );
        for &(x, y) in &requested_tiles {
            let slot = batch_slot(&batch, x, y);
            bake_tile_vector_rx_refl(&mut batch.tiles[slot], obstacle_data.set());
        }
        let batch_raster = BatchDeviceRaster::upload(&frame, &batch.tiles[0])?;
        measurement.raster_and_receiver_seconds += raster_started.elapsed().as_secs_f64();

        for (x, y) in requested_tiles {
            let tile = &batch.tiles[batch_slot(&batch, x, y)];
            let receiver_started = Instant::now();
            let receivers = TileDeviceReceivers::upload(&frame, tile, obstacle_data.set())?;
            let interior = obstacle_data.interior_estimate(tile);
            let barriers = barrier_data.for_tile(&tile.bbox, LINE_HALO_M);
            measurement.raster_and_receiver_seconds += receiver_started.elapsed().as_secs_f64();
            for layer in &layers {
                let output_path = output_tile_path(
                    &configuration.output_directory,
                    layer.directory_name,
                    zoom,
                    x,
                    y,
                );
                let partition_path = partition_tile_path(
                    &configuration.output_directory,
                    layer.directory_name,
                    zoom,
                    x,
                    y,
                );
                let tile_started = Instant::now();
                let tile_measurement = partition_paint_and_write_tile(
                    cuda,
                    &frame,
                    &layer.sources,
                    layer.fingerprint,
                    &layer.device_sources,
                    &device_obstacles,
                    &batch_raster,
                    &receivers,
                    &barriers,
                    &interior,
                    coarse_middle_cadence(zoom),
                    layer.source_id,
                    &output_path,
                    &partition_path,
                )?;
                measurement.host_tile_seconds += tile_started.elapsed().as_secs_f64()
                    - (tile_measurement.corner_gpu_milliseconds
                        + tile_measurement.paint_gpu_milliseconds)
                        / 1000.0;
                match layer.directory_name {
                    "road" => measurement.road.add_tile(tile_measurement),
                    "rail" => measurement.rail.add_tile(tile_measurement),
                    _ => unreachable!(),
                }
            }
        }
    }
    Ok(())
}

fn encode_line_layer(
    directory_name: &'static str,
    source_id: u8,
    frame: &RegionMetricFrame,
    rows: &[tile_painter::source_line::LineRow],
) -> Result<EncodedLineLayer> {
    let sources: Vec<_> = rows.iter().map(|row| frame.encode_line(row)).collect();
    let fingerprint = source_identity_fingerprint(&sources);
    let device_sources = RegionDeviceLineSources::upload(&sources)?;
    Ok(EncodedLineLayer {
        directory_name,
        source_id,
        sources,
        fingerprint,
        device_sources,
    })
}

fn output_tile_path(root: &Path, layer: &str, zoom: u8, x: u32, y: u32) -> PathBuf {
    root.join(layer)
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin"))
}

fn partition_tile_path(root: &Path, layer: &str, zoom: u8, x: u32, y: u32) -> PathBuf {
    root.join("relevant-source-partitions")
        .join(layer)
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{y}.rsp"))
}

#[derive(Clone, Copy)]
struct ProcessUsage {
    user_seconds: f64,
    system_seconds: f64,
}

impl ProcessUsage {
    fn read() -> Self {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        assert_eq!(status, 0, "getrusage failed");
        let usage = unsafe { usage.assume_init() };
        Self {
            user_seconds: timeval_seconds(usage.ru_utime),
            system_seconds: timeval_seconds(usage.ru_stime),
        }
    }

    fn seconds(self) -> f64 {
        self.user_seconds + self.system_seconds
    }
}

fn timeval_seconds(value: libc::timeval) -> f64 {
    value.tv_sec as f64 + value.tv_usec as f64 * 1.0e-6
}
