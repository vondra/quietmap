//! The surface-layer runner over one wave's zoom: road, rail, industrial and
//! building from one preparation, as a streaming pipeline over cells (the host
//! prepares cell N+1 while the card paints cell N) with the paint's own batch
//! lookahead, and the phase/pair measurement of every step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use h3o::{CellIndex, LatLng};
use noise_compute::admin;
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;
use tile_painter::region_runner::{batch_slot, block_batch_origin, region_tiles};
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_building::BuildingData;
use tile_painter::source_loader_industrial::IndustrialData;
use tile_painter::source_loader_obstacle::{bake_tile_vector_rx_refl, ObstacleData};
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::wire_hm3::{
    SOURCE_ID_BUILDING, SOURCE_ID_INDUSTRIAL, SOURCE_ID_RAIL, SOURCE_ID_ROAD,
};

use crate::cuda_bridge::RelevantSourceCuda;
use crate::obstacle_transfer::FlattenedObstacleGeometry;
use crate::relevant_source_tile::{
    partition_and_paint_tile, BatchDeviceRaster, PendingTileWrite, RegionDeviceLineSources,
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

/// The surface layers one preparation paints, in output order.
pub const LAYER_NAMES: [&str; LAYER_COUNT] = ["road", "rail", "industrial", "building"];
pub const LAYER_COUNT: usize = 4;
const LAYER_SOURCE_IDS: [u8; LAYER_COUNT] = [
    SOURCE_ID_ROAD,
    SOURCE_ID_RAIL,
    SOURCE_ID_INDUSTRIAL,
    SOURCE_ID_BUILDING,
];
/// Point-grid area sources get the CPU's median footprint fill before the write.
const LAYER_AREA_SOURCE: [bool; LAYER_COUNT] = [false, false, true, true];

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
    /// One entry per [`LAYER_NAMES`] layer.
    pub layers: Vec<LayerMeasurement>,
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
        self.layers
            .iter()
            .map(|layer| layer.corner_gpu_milliseconds + layer.paint_gpu_milliseconds)
            .sum::<f64>()
            / 1000.0
    }

    pub fn attempted_pairs(&self) -> u64 {
        self.layers
            .iter()
            .map(|layer| layer.corner_pairs + layer.pixel_pairs)
            .sum()
    }
}

struct EncodedLineLayer {
    directory_name: &'static str,
    source_id: u8,
    sources: Vec<DeviceLineSource>,
    fingerprint: u64,
    device_sources: RegionDeviceLineSources,
}

/// Cells resident on the card at once: the one painting and the next one.
const RESIDENT_CELLS: usize = 2;

/// One cell's sources, obstacles and barriers loaded and encoded on the host,
/// nothing on the card yet.
struct HostPreparedRegion {
    region_r4: u64,
    frame: RegionMetricFrame,
    batches: BTreeMap<(u32, u32), Vec<(u32, u32)>>,
    layers: Vec<Vec<DeviceLineSource>>,
    barrier_data: BarrierData,
    obstacle_data: ObstacleData,
    flattened_obstacles: FlattenedObstacleGeometry,
    started: Instant,
}

impl HostPreparedRegion {
    /// Put the cell on the card: called only once a residency permit is held.
    fn upload(self) -> Result<PreparedRegion> {
        let device_obstacles = RegionDeviceObstacles::upload(&self.flattened_obstacles)?;
        let layers = self
            .layers
            .into_iter()
            .enumerate()
            .map(|(layer, sources)| encode_layer(layer, sources))
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedRegion {
            region_r4: self.region_r4,
            frame: self.frame,
            batches: self.batches,
            layers,
            barrier_data: self.barrier_data,
            obstacle_data: self.obstacle_data,
            device_obstacles,
            prepare_seconds: self.started.elapsed().as_secs_f64(),
        })
    }
}

/// One cell's sources, obstacles and barriers loaded, encoded and resident on the
/// card, ready to paint: the unit the cell producer hands the painter.
struct PreparedRegion {
    region_r4: u64,
    frame: RegionMetricFrame,
    batches: BTreeMap<(u32, u32), Vec<(u32, u32)>>,
    layers: Vec<EncodedLineLayer>,
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
    let mut measurement = RelevantSourceRunMeasurement {
        layers: vec![LayerMeasurement::default(); LAYER_COUNT],
        ..RelevantSourceRunMeasurement::default()
    };

    // The cell stream: one producer loads and encodes cell N+1 on the host while
    // the painter works on cell N, and uploads it only after taking one of the
    // RESIDENT_CELLS permits (the painting cell holds the other), so no third
    // cell is ever on the card; the painter returns the permit once a cell's
    // device buffers are dropped.
    let (sender, receiver) = sync_channel(1);
    let (permit_sender, permit_receiver) = sync_channel::<()>(RESIDENT_CELLS);
    for _ in 0..RESIDENT_CELLS {
        permit_sender
            .send(())
            .expect("the permit channel holds every permit");
    }
    let producer = thread::Builder::new().name("cell-prepare".into());
    let (host_wait_seconds, cells_prepared) = thread::scope(|scope| -> Result<(f64, usize)> {
        let handle = producer.spawn_scoped(scope, move || -> Result<(f64, usize)> {
            let mut host_wait_seconds = 0.0;
            let mut prepared_count = 0;
            for &region_r4 in &configuration.regions {
                let host_prepared = prepare_region(configuration, region_r4)?;
                let wait_started = Instant::now();
                if permit_receiver.recv().is_err() {
                    break;
                }
                host_wait_seconds += wait_started.elapsed().as_secs_f64();
                let prepared = host_prepared.upload()?;
                prepared_count += 1;
                if sender.send(prepared).is_err() {
                    break;
                }
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
            // The painted cell's device buffers are dropped: hand its permit back.
            let _ = permit_sender.send(());
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
) -> Result<HostPreparedRegion> {
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
    let industrial_rows =
        IndustrialData::load_for_r4s(&configuration.h3r4_directory, &ring)?.into_rows();
    let building_rows =
        BuildingData::load_for_r4s(&configuration.h3r4_directory, &ring)?.into_rows();
    let encoded = [
        road_rows.iter().map(|row| frame.encode_line(row)).collect(),
        rail_rows.iter().map(|row| frame.encode_line(row)).collect(),
        industrial_rows
            .iter()
            .map(|row| frame.encode_point(row))
            .collect(),
        building_rows
            .iter()
            .map(|row| frame.encode_point(row))
            .collect(),
    ];
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
    Ok(HostPreparedRegion {
        region_r4,
        frame,
        batches,
        layers: encoded.into(),
        barrier_data,
        obstacle_data,
        flattened_obstacles,
        started,
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
    for (layer, encoded) in layers.iter().enumerate() {
        measurement.layers[layer].loaded_sources += encoded.sources.len() as u64;
    }
    measurement.source_load_seconds += prepare_seconds;
    // The terrain halo of the next batch is built while the GPU paints this one:
    // one producer thread, one batch of lookahead, the GPU never waits on a halo
    // it could have had earlier and the host never runs more than two ahead. The
    // collapse and brotli write of every painted tile go to one writer thread
    // behind a bounded channel, off the painter's critical path.
    let obstacle_set = obstacle_data.set();
    let (sender, receiver) = sync_channel(1);
    let (write_sender, write_receiver) = sync_channel::<PendingTileWrite>(PENDING_WRITES);
    let producer = thread::Builder::new().name("batch-rasters".into());
    let writer = thread::Builder::new().name("tile-writer".into());
    let output_bytes = thread::scope(|scope| -> Result<[u64; LAYER_COUNT]> {
        let writer = writer.spawn_scoped(scope, move || -> Result<[u64; LAYER_COUNT]> {
            let mut bytes = [0_u64; LAYER_COUNT];
            for pending in write_receiver {
                let layer = pending.layer;
                bytes[layer] += pending.write()?;
            }
            Ok(bytes)
        })?;
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
                &write_sender,
                measurement,
            )?;
        }
        drop(write_sender);
        writer.join().expect("tile writer thread")
    })?;
    for (layer, bytes) in output_bytes.into_iter().enumerate() {
        measurement.layers[layer].output_bytes += bytes;
    }
    measurement.cells.push((
        region_r4,
        prepare_seconds,
        paint_started.elapsed().as_secs_f64(),
    ));
    Ok(())
}

/// Painted tiles waiting for the writer before the painter blocks (3 MB each).
const PENDING_WRITES: usize = 8;

#[allow(clippy::too_many_arguments)]
fn paint_batch_tiles(
    configuration: &RelevantSourceRunConfiguration,
    batch: &TileBatch,
    requested_tiles: &[(u32, u32)],
    frame: &RegionMetricFrame,
    layers: &[EncodedLineLayer],
    obstacle_data: &ObstacleData,
    barrier_data: &BarrierData,
    device_obstacles: &RegionDeviceObstacles,
    batch_raster: &BatchDeviceRaster,
    cuda: &RelevantSourceCuda,
    write_sender: &SyncSender<PendingTileWrite>,
    measurement: &mut RelevantSourceRunMeasurement,
) -> Result<()> {
    let zoom = configuration.zoom;
    for &(x, y) in requested_tiles {
        let tile = &batch.tiles[batch_slot(batch, x, y)];
        let receiver_started = Instant::now();
        let receivers = TileDeviceReceivers::upload(frame, tile, obstacle_data.set())?;
        let interior = Arc::new(obstacle_data.interior_estimate(tile));
        let barriers = barrier_data.for_tile(&tile.bbox, LINE_HALO_M);
        measurement.receiver_seconds += receiver_started.elapsed().as_secs_f64();
        for (layer_index, layer) in layers.iter().enumerate() {
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
            let (tile_measurement, energy) = partition_and_paint_tile(
                cuda,
                frame,
                &layer.sources,
                layer.fingerprint,
                &layer.device_sources,
                device_obstacles,
                batch_raster,
                &receivers,
                &barriers,
                coarse_middle_cadence(zoom),
                &partition_path,
            )?;
            write_sender
                .send(PendingTileWrite {
                    energy,
                    interior: Arc::clone(&interior),
                    layer: layer_index,
                    area_source: LAYER_AREA_SOURCE[layer_index],
                    source_id: layer.source_id,
                    output_path,
                })
                .context("the tile writer thread is gone")?;
            measurement.host_tile_seconds += tile_started.elapsed().as_secs_f64()
                - (tile_measurement.corner_gpu_milliseconds
                    + tile_measurement.paint_gpu_milliseconds)
                    / 1000.0;
            measurement.layers[layer_index].add_tile(tile_measurement);
        }
    }
    Ok(())
}

fn encode_layer(layer: usize, sources: Vec<DeviceLineSource>) -> Result<EncodedLineLayer> {
    let fingerprint = source_identity_fingerprint(&sources);
    let device_sources = RegionDeviceLineSources::upload(&sources)?;
    Ok(EncodedLineLayer {
        directory_name: LAYER_NAMES[layer],
        source_id: LAYER_SOURCE_IDS[layer],
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
