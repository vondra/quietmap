//! The surface-layer runner over one wave's zoom: road, rail, industrial,
//! building and airport ground ops from one preparation, painted cell by cell
//! as the stream delivers them (the host prepares cell N+1 while the card
//! paints cell N) with the paint's own batch lookahead.
//!
//! An ERROR on one cell ends that cell and nothing else: it is reported as
//! `fail`, counted into the exit status, and the next cell starts. A panic is
//! not caught here — it ends the process, and the worker that owns the engine
//! restarts it, as the orchestrator contract has it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use noise_compute::admin;
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;
use tile_painter::region_runner::{batch_slot, block_batch_origin};
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::{bake_tile_vector_rx_refl, ObstacleData};

use crate::cell_measurement::CellMeasurement;
use crate::cell_preparation::{prepare_region, EncodedLineLayer, PreparedRegion};
use crate::cell_stream::{report_cell_done, report_cell_failed, report_cell_started, CellRequest};
use crate::cuda_bridge::RelevantSourceCuda;
use crate::relevant_source_tile::{
    partition_and_paint_tile, BatchDeviceRaster, PendingTileWrite, RegionDeviceObstacles,
    TileDeviceReceivers,
};
use crate::source_frame::RegionMetricFrame;
use crate::surface_layers::{
    LAYER_AREA_SOURCE, LAYER_COUNT, LAYER_EVENT_ENERGY, LAYER_LDEN_WEIGHTS,
};

const REGION_TILE_BATCH_SIDE: u32 = 4;
const LINE_HALO_M: f64 = 10_000.0;
/// Cells resident on the card at once: the one painting and the next one.
const RESIDENT_CELLS: usize = 2;
/// Painted tiles waiting for the writer before the painter blocks (3 MB each).
const PENDING_WRITES: usize = 8;
pub struct RelevantSourceRunConfiguration {
    pub prepared_directory: PathBuf,
    pub h3r4_directory: PathBuf,
    pub output_directory: PathBuf,
    /// Web-Mercator zoom of the painted tiles.
    pub zoom: u8,
}

impl RelevantSourceRunConfiguration {
    /// Everything the painter reads hangs off the one prepared root the worker
    /// is given, so the h3r4 tree is derived here and never named twice. The
    /// dataset year is the tree's own `<year>/h3r4` child — one truth, declared
    /// by `scripts/dataset-year.json` when the tree was prepared — unless the
    /// caller names one (`DATA_YEAR`, the project-wide override that file
    /// documents), which then must exist.
    pub fn for_prepared_root(
        prepared_directory: PathBuf,
        output_directory: PathBuf,
        zoom: u8,
        dataset_year: Option<&str>,
    ) -> Result<Self> {
        let year = match dataset_year {
            Some(year) => year.to_owned(),
            None => crate::cell_stream::prepared_dataset_year(&prepared_directory)?,
        };
        let h3r4_directory = prepared_directory.join(&year).join("h3r4");
        if !h3r4_directory.is_dir() {
            bail!(
                "prepared root {} has no {year}/h3r4 tree",
                prepared_directory.display()
            );
        }
        Ok(Self {
            prepared_directory,
            h3r4_directory,
            output_directory,
            zoom,
        })
    }
}

/// What the cell producer hands the painter: a cell on the card holding one of
/// the [`RESIDENT_CELLS`] permits, or a cell that will never reach the card.
enum StreamedRegion {
    Prepared(Box<PreparedRegion>),
    Failed { cell: String, message: String },
}

/// Paint the cells as they arrive on `cells`, one `start`/`done`/`fail` line
/// each, and return how many failed.
///
/// The producer takes cells from the iterator lazily — a world run feeds them
/// over hours — and prepares cell N+1 while the card paints cell N, uploading
/// it only after taking one of the RESIDENT_CELLS permits (the painting cell
/// holds the other), so no third cell is ever on the card; the painter returns
/// the permit once a cell's device buffers are dropped.
pub fn run_relevant_source_stream(
    configuration: &RelevantSourceRunConfiguration,
    cells: impl Iterator<Item = CellRequest> + Send,
) -> Result<usize> {
    admin::init_admin_table(&admin::default_admin_path(&configuration.h3r4_directory))
        .context("load the road/rail admin table")?;
    let rasters = RealRasters::new(&configuration.prepared_directory);
    let cuda = RelevantSourceCuda::initialize()?;
    let (sender, receiver) = sync_channel(1);
    let (permit_sender, permit_receiver) = sync_channel::<()>(RESIDENT_CELLS);
    for _ in 0..RESIDENT_CELLS {
        permit_sender
            .send(())
            .expect("the permit channel holds every permit");
    }
    let abandoned_permits = permit_sender.clone();
    let producer = thread::Builder::new().name("cell-prepare".into());
    thread::scope(|scope| -> Result<usize> {
        // The producer returns the faults of the STREAM itself, which name no
        // cell and so are never `fail` lines; the painter counts the cells.
        let handle = producer.spawn_scoped(scope, move || -> usize {
            let mut stream_faults = 0;
            for request in cells {
                let cell = match request {
                    CellRequest::Cell(cell) => cell,
                    CellRequest::Rejected { cell, message } => {
                        if sender
                            .send(StreamedRegion::Failed { cell, message })
                            .is_err()
                        {
                            return stream_faults;
                        }
                        continue;
                    }
                    CellRequest::Unreadable { message } => {
                        eprintln!("relevant-source-surface: {message}");
                        stream_faults += 1;
                        continue;
                    }
                };
                let label = cell.label();
                let host_prepared = match prepare_region(configuration, &cell) {
                    Ok(host_prepared) => host_prepared,
                    Err(error) => {
                        if sender.send(failed(label, &error)).is_err() {
                            return stream_faults;
                        }
                        continue;
                    }
                };
                let permit_wait = Instant::now();
                if permit_receiver.recv().is_err() {
                    return stream_faults;
                }
                let message = match host_prepared.upload(permit_wait.elapsed().as_secs_f64()) {
                    Ok(prepared) => StreamedRegion::Prepared(Box::new(prepared)),
                    Err(error) => {
                        // The upload left nothing on the card: hand the residency
                        // permit straight back or the pipeline stalls on a cell
                        // that never arrives.
                        let _ = abandoned_permits.send(());
                        failed(label, &error)
                    }
                };
                if sender.send(message).is_err() {
                    return stream_faults;
                }
            }
            stream_faults
        })?;
        let mut failures = 0;
        loop {
            let waited = Instant::now();
            let Ok(streamed) = receiver.recv() else {
                break;
            };
            let card_wait_seconds = waited.elapsed().as_secs_f64();
            let prepared = match streamed {
                StreamedRegion::Failed { cell, message } => {
                    report_cell_failed(&cell, &message);
                    failures += 1;
                    continue;
                }
                StreamedRegion::Prepared(prepared) => prepared,
            };
            let region_r4 = prepared.region_r4;
            report_cell_started(region_r4);
            let started = Instant::now();
            let painted =
                paint_region(configuration, *prepared, &rasters, &cuda, card_wait_seconds);
            // The painted cell's device buffers are dropped: hand its permit back.
            let _ = permit_sender.send(());
            match painted {
                Ok(mut measurement) => {
                    measurement.wall_seconds = started.elapsed().as_secs_f64();
                    report_cell_done(region_r4, &measurement.statistics(configuration.zoom));
                }
                Err(error) => {
                    report_cell_failed(&format!("{region_r4:x}"), &format!("{error:#}"));
                    failures += 1;
                }
            }
        }
        Ok(failures + handle.join().expect("cell producer thread"))
    })
}

/// An error chain as the one-line message of a `fail`.
fn failed(cell: String, error: &anyhow::Error) -> StreamedRegion {
    StreamedRegion::Failed {
        cell,
        message: format!("{error:#}"),
    }
}

fn paint_region(
    configuration: &RelevantSourceRunConfiguration,
    prepared: PreparedRegion,
    rasters: &RealRasters,
    cuda: &RelevantSourceCuda,
    card_wait_seconds: f64,
) -> Result<CellMeasurement> {
    let paint_started = Instant::now();
    let zoom = configuration.zoom;
    let PreparedRegion {
        region_r4: _,
        tiles,
        frame,
        layers,
        barrier_data,
        obstacle_data,
        device_obstacles,
        n_days,
        prepare_seconds,
        permit_wait_seconds,
    } = prepared;
    let mut measurement =
        CellMeasurement::new(layers.iter().map(|encoded| encoded.layer).collect());
    measurement.prepare_seconds = prepare_seconds;
    measurement.permit_wait_seconds = permit_wait_seconds;
    measurement.card_wait_seconds = card_wait_seconds;
    for encoded in &layers {
        measurement.layers[encoded.layer].loaded_sources += encoded.sources.len() as u64;
    }
    // The tiles this cell owns, grouped into the batches that share one terrain
    // halo. The halo of the next batch is built while the GPU paints this one:
    // one producer thread, one batch of lookahead, the GPU never waits on a halo
    // it could have had earlier and the host never runs more than two ahead. The
    // collapse and brotli write of every painted tile go to one writer thread
    // behind a bounded channel, off the painter's critical path.
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
                n_days,
                &mut measurement,
            )?;
        }
        drop(write_sender);
        writer.join().expect("tile writer thread")
    })?;
    for (layer, bytes) in output_bytes.into_iter().enumerate() {
        measurement.layers[layer].output_bytes += bytes;
    }
    measurement.paint_seconds = paint_started.elapsed().as_secs_f64();
    Ok(measurement)
}

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
    n_days: f64,
    measurement: &mut CellMeasurement,
) -> Result<()> {
    let zoom = configuration.zoom;
    for &(x, y) in requested_tiles {
        let tile = &batch.tiles[batch_slot(batch, x, y)];
        let receiver_started = Instant::now();
        let receivers = TileDeviceReceivers::upload(frame, tile, obstacle_data.set())?;
        let interior = Arc::new(obstacle_data.interior_estimate(tile));
        let barriers = barrier_data.for_tile(&tile.bbox, LINE_HALO_M);
        measurement.receiver_seconds += receiver_started.elapsed().as_secs_f64();
        for encoded in layers {
            let layer = encoded.layer;
            let output_path = output_tile_path(
                &configuration.output_directory,
                encoded.directory_name,
                zoom,
                x,
                y,
            );
            let partition_path = partition_tile_path(
                &configuration.output_directory,
                encoded.directory_name,
                zoom,
                x,
                y,
            );
            let tile_started = Instant::now();
            let (tile_measurement, energy) = partition_and_paint_tile(
                cuda,
                frame,
                &encoded.sources,
                encoded.fingerprint,
                &encoded.device_sources,
                device_obstacles,
                batch_raster,
                &receivers,
                &barriers,
                LAYER_LDEN_WEIGHTS[layer],
                &partition_path,
            )?;
            write_sender
                .send(PendingTileWrite {
                    energy,
                    interior: Arc::clone(&interior),
                    layer,
                    area_source: LAYER_AREA_SOURCE[layer],
                    event_days: LAYER_EVENT_ENERGY[layer].then_some(n_days),
                    source_id: encoded.source_id,
                    output_path,
                })
                .context("the tile writer thread is gone")?;
            measurement.host_tile_seconds += tile_started.elapsed().as_secs_f64()
                - (tile_measurement.corner_gpu_milliseconds
                    + tile_measurement.paint_gpu_milliseconds)
                    / 1000.0;
            measurement.layers[layer].add_tile(tile_measurement);
        }
    }
    Ok(())
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
