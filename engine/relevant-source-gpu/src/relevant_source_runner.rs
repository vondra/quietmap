//! The surface-layer runner over one wave's zoom: road, rail, industrial,
//! building and airport ground ops from one preparation, painted cell by cell
//! as the stream delivers them (the host prepares cell N+1 while the card
//! paints cell N) with the paint's own batch lookahead.
//!
//! An ERROR on one cell ends that cell and nothing else: it is reported as
//! `fail`, counted into the exit status, and the next cell starts. A panic is
//! not caught here — it ends the process, and the worker that owns the engine
//! restarts it, as the orchestrator contract has it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use noise_compute::admin;
use raster_reader::RealRasters;
use tile_painter::region_runner::{batch_slot, group_tiles_into_batches};
use tile_painter::source_loader_structure::StructureData;

use crate::batch_raster_lookahead::{spawn_batch_raster_builders, BatchRequest, ReadyBatch};
use crate::cell_measurement::CellMeasurement;
use crate::cell_preparation::{prepare_region, EncodedLineLayer, PreparedRegion};
use crate::cell_stream::{report_cell_done, report_cell_failed, report_cell_started, CellRequest};
use crate::cuda_bridge::RelevantSourceCuda;
use crate::pending_tile_write::PendingTileWrite;
use crate::relevant_source_tile::{
    partition_and_paint_tile, BatchDeviceRaster, RegionDeviceObstacles, TileDeviceReceivers,
    TilePaintMeasurement,
};
use crate::source_frame::{DeviceLineSource, RegionMetricFrame};
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
    admin::set_admin_h3r4_directory(&configuration.h3r4_directory);
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
        structure_data,
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
    // halo. Several builder threads prepare the batches ahead of the card and
    // deliver them in batch order (`batch_raster_lookahead`), so a box with
    // spare cores keeps the card fed. The collapse and brotli write of every
    // painted tile go to one writer thread behind a bounded channel, off the
    // painter's critical path.
    let batches: Vec<BatchRequest> = group_tiles_into_batches(&tiles, REGION_TILE_BATCH_SIDE)
        .into_iter()
        .collect();
    let obstacle_set = structure_data.set();
    let layer_sources: Vec<&[DeviceLineSource]> = layers
        .iter()
        .map(|encoded| encoded.sources.as_slice())
        .collect();
    let (write_sender, write_receiver) = sync_channel::<PendingTileWrite>(PENDING_WRITES);
    let writer = thread::Builder::new().name("tile-writer".into());
    let output_bytes = thread::scope(|scope| -> Result<[u64; LAYER_COUNT]> {
        let writer = writer.spawn_scoped(scope, move || -> Result<[u64; LAYER_COUNT]> {
            let mut bytes = [0_u64; LAYER_COUNT];
            for pending in write_receiver {
                let layer = pending.layer();
                bytes[layer] += pending.write()?;
            }
            Ok(bytes)
        })?;
        let batch_channels = spawn_batch_raster_builders(
            scope,
            &batches,
            zoom,
            REGION_TILE_BATCH_SIDE,
            LINE_HALO_M,
            &frame,
            &layer_sources,
            rasters,
            obstacle_set,
        )?;
        for index in 0..batches.len() {
            let ready = batch_channels[index % batch_channels.len()]
                .recv()
                .context("a batch raster builder ended before its batches did")?;
            measurement.raster_prepare_seconds += ready.prepare_seconds;
            paint_batch_tiles(
                configuration,
                &ready,
                &frame,
                &layers,
                &structure_data,
                &device_obstacles,
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

/// Paint one delivered batch: every requested tile in every layer a source
/// reaches, and the all-`NO_DATA` tile in every layer none reaches. A batch no
/// layer reaches arrives without rasters and never touches the card.
#[allow(clippy::too_many_arguments)]
fn paint_batch_tiles(
    configuration: &RelevantSourceRunConfiguration,
    ready: &ReadyBatch,
    frame: &RegionMetricFrame,
    layers: &[EncodedLineLayer],
    structure_data: &StructureData,
    device_obstacles: &RegionDeviceObstacles,
    cuda: &RelevantSourceCuda,
    write_sender: &SyncSender<PendingTileWrite>,
    n_days: f64,
    measurement: &mut CellMeasurement,
) -> Result<()> {
    let zoom = configuration.zoom;
    assert_eq!(
        layers.len(),
        ready.reached_layers.len(),
        "a batch's reach flags must cover exactly the cell's painted layers"
    );
    let rastered = match &ready.rasters {
        Some(batch) => Some((batch, BatchDeviceRaster::upload(frame, &batch.tiles[0])?)),
        None => None,
    };
    for &(x, y) in &ready.requested_tiles {
        let receiver_started = Instant::now();
        let painted = match &rastered {
            Some((batch, batch_raster)) => {
                let tile = &batch.tiles[batch_slot(batch, x, y)];
                let receivers = TileDeviceReceivers::upload(frame, tile, structure_data.set())?;
                let interior = Arc::new(structure_data.interior_estimate(tile));
                Some((receivers, interior, batch_raster))
            }
            None => None,
        };
        measurement.receiver_seconds += receiver_started.elapsed().as_secs_f64();
        for (encoded, reached) in layers.iter().zip(&ready.reached_layers) {
            let layer = encoded.layer;
            let output_path = output_tile_path(
                &configuration.output_directory,
                encoded.directory_name,
                zoom,
                x,
                y,
            );
            let Some((receivers, interior, batch_raster)) = painted.as_ref().filter(|_| *reached)
            else {
                write_sender
                    .send(PendingTileWrite::Silent {
                        layer,
                        source_id: encoded.source_id,
                        output_path,
                    })
                    .context("the tile writer thread is gone")?;
                measurement.layers[layer].add_tile(TilePaintMeasurement::default());
                continue;
            };
            let tile_started = Instant::now();
            let (tile_measurement, energy) = partition_and_paint_tile(
                cuda,
                &encoded.sources,
                &encoded.device_sources,
                device_obstacles,
                batch_raster,
                receivers,
                LAYER_LDEN_WEIGHTS[layer],
            )?;
            write_sender
                .send(PendingTileWrite::Painted {
                    energy,
                    interior: Arc::clone(interior),
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
