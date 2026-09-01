//! Road/rail runner over one wave's zoom: a streaming pipeline over cells (the host
//! prepares cell N+1 while the card paints cell N) with the paint's own batch
//! lookahead, and the phase/pair measurement of every step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
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
    /// Source, barrier and obstacle loading plus the device uploads of every
    /// cell, on the cell producer thread (only the first cell's is on the wall).
    pub source_load_seconds: f64,
    /// Terrain halo build and facade baking per 4x4 batch, on the producer
    /// thread that runs one batch ahead of the GPU.
    pub raster_prepare_seconds: f64,
    /// Receiver, enclosure and barrier preparation per tile, serial with the GPU.
    pub receiver_seconds: f64,
    pub host_tile_seconds: f64,
    /// Time the painter waited for the next prepared cell: the card idle on the host.
    pub card_wait_seconds: f64,
    /// Time the cell producer waited for the painter to take a cell: the host idle on the card.
    pub host_wait_seconds: f64,
    /// `(cell, prepare seconds, paint seconds)` in stream order.
    pub cells: Vec<(u64, f64, f64)>,
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

/// One cell's sources, obstacles and barriers loaded, encoded and resident on the
/// card, ready to paint: the unit the cell producer hands the painter.
struct PreparedRegion {
    region_r4: u64,
    frame: RegionMetricFrame,
    batches: BTreeMap<(u32, u32), Vec<(u32, u32)>>,
    layers: [EncodedLineLayer; 2],
    barrier_data: BarrierData,
    obstacle_data: ObstacleData,
    device_obstacles: RegionDeviceObstacles,
    prepare_seconds: f64,
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

    // The cell stream: one producer loads and uploads cell N+1 (and blocks on the
    // channel once it is one cell ahead, so at most two cells are resident on the
    // card) while the painter works on cell N.
    let (sender, receiver) = sync_channel(1);
    let producer = thread::Builder::new().name("cell-prepare".into());
    let (host_wait_seconds, cells_prepared) = thread::scope(|scope| -> Result<(f64, usize)> {
        let handle = producer.spawn_scoped(scope, move || -> Result<(f64, usize)> {
            let mut host_wait_seconds = 0.0;
            let mut prepared_count = 0;
            for &region_r4 in &configuration.regions {
                let prepared = prepare_region(configuration, region_r4)?;
                prepared_count += 1;
                let wait_started = Instant::now();
                if sender.send(prepared).is_err() {
                    break;
                }
                host_wait_seconds += wait_started.elapsed().as_secs_f64();
            }
            Ok((host_wait_seconds, prepared_count))
        })?;
        loop {
            let wait_started = Instant::now();
            let Ok(prepared) = receiver.recv() else {
                break;
            };
            measurement.card_wait_seconds += wait_started.elapsed().as_secs_f64();
            paint_region(configuration, prepared, &rasters, &cuda, &mut measurement)?;
        }
        handle.join().expect("cell producer thread")
    })?;
    measurement.host_wait_seconds = host_wait_seconds;
    if cells_prepared != configuration.regions.len() {
        bail!(
            "cell producer prepared {cells_prepared} of {} cells",
            configuration.regions.len()
        );
    }
    measurement.wall_seconds = started.elapsed().as_secs_f64();
    measurement.cpu_seconds = ProcessUsage::read().seconds() - starting_usage.seconds();
    Ok(measurement)
}

fn prepare_region(
    configuration: &RelevantSourceRunConfiguration,
    region_r4: u64,
) -> Result<PreparedRegion> {
    let started = Instant::now();
    let cell = CellIndex::try_from(region_r4).context("invalid R4 region")?;
    let zoom = configuration.zoom;
    let tiles = region_tiles(region_r4, zoom);
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let frame = RegionMetricFrame::for_cell(cell);
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
    Ok(PreparedRegion {
        region_r4,
        frame,
        batches,
        layers: [road, rail],
        barrier_data,
        obstacle_data,
        device_obstacles,
        prepare_seconds: started.elapsed().as_secs_f64(),
    })
}

fn paint_region(
    configuration: &RelevantSourceRunConfiguration,
    prepared: PreparedRegion,
    rasters: &RealRasters,
    cuda: &RelevantSourceCuda,
    measurement: &mut RelevantSourceRunMeasurement,
) -> Result<()> {
    let paint_started = Instant::now();
    let zoom = configuration.zoom;
    let PreparedRegion {
        region_r4,
        frame,
        batches,
        layers,
        barrier_data,
        obstacle_data,
        device_obstacles,
        prepare_seconds,
    } = prepared;
    measurement.road.loaded_sources += layers[0].sources.len() as u64;
    measurement.rail.loaded_sources += layers[1].sources.len() as u64;
    measurement.source_load_seconds += prepare_seconds;
    // The terrain halo of the next batch is built while the GPU paints this one:
    // one producer thread, one batch of lookahead, the GPU never waits on a halo
    // it could have had earlier and the host never runs more than two ahead.
    let obstacle_set = obstacle_data.set();
    let (sender, receiver) = sync_channel(1);
    let producer = thread::Builder::new().name("batch-rasters".into());
    thread::scope(|scope| -> Result<()> {
        producer.spawn_scoped(scope, move || {
            for ((block_x, block_y), requested_tiles) in batches {
                let started = Instant::now();
                let (base_x, base_y) =
                    block_batch_origin(block_x, block_y, REGION_TILE_BATCH_SIDE, zoom);
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
                    bake_tile_vector_rx_refl(&mut batch.tiles[slot], obstacle_set);
                }
                if sender
                    .send((requested_tiles, batch, started.elapsed().as_secs_f64()))
                    .is_err()
                {
                    return;
                }
            }
        })?;
        for (requested_tiles, batch, prepare_seconds) in receiver {
            measurement.raster_prepare_seconds += prepare_seconds;
            let batch_raster = BatchDeviceRaster::upload(&frame, &batch.tiles[0])?;
            paint_batch_tiles(
                configuration,
                &batch,
                &requested_tiles,
                &frame,
                &layers,
                &obstacle_data,
                &barrier_data,
                &device_obstacles,
                &batch_raster,
                cuda,
                measurement,
            )?;
        }
        Ok(())
    })?;
    measurement.cells.push((
        region_r4,
        prepare_seconds,
        paint_started.elapsed().as_secs_f64(),
    ));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_batch_tiles(
    configuration: &RelevantSourceRunConfiguration,
    batch: &TileBatch,
    requested_tiles: &[(u32, u32)],
    frame: &RegionMetricFrame,
    layers: &[EncodedLineLayer; 2],
    obstacle_data: &ObstacleData,
    barrier_data: &BarrierData,
    device_obstacles: &RegionDeviceObstacles,
    batch_raster: &BatchDeviceRaster,
    cuda: &RelevantSourceCuda,
    measurement: &mut RelevantSourceRunMeasurement,
) -> Result<()> {
    let zoom = configuration.zoom;
    for &(x, y) in requested_tiles {
        let tile = &batch.tiles[batch_slot(batch, x, y)];
        let receiver_started = Instant::now();
        let receivers = TileDeviceReceivers::upload(frame, tile, obstacle_data.set())?;
        let interior = obstacle_data.interior_estimate(tile);
        let barriers = barrier_data.for_tile(&tile.bbox, LINE_HALO_M);
        measurement.receiver_seconds += receiver_started.elapsed().as_secs_f64();
        for layer in layers {
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
                frame,
                &layer.sources,
                layer.fingerprint,
                &layer.device_sources,
                device_obstacles,
                batch_raster,
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
