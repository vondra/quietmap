//! One cell's tile-batch rasters built on several CPU threads and handed to the
//! card in batch order, and the screen that decides a batch needs none.
//!
//! The single producer this replaces starved the card whenever the host's CPU
//! was weak or busy: measured on the 2026-09-05 world repaint, `gpu_s/wall_s`
//! was 42-69 % on four of the painting hosts and one cell spent 314 s
//! preparing rasters for 9 s of paint.
//!
//! Neither the order nor the content of a batch depends on the thread count.
//! Each batch is a pure function of its block: `TileBatch::build_opt_rx_refl`
//! reads only the block's tile coordinates and the mmap'd rasters, and
//! `bake_tile_vector_rx_refl` only that tile's receiver lattice and the
//! region's obstacle set, neither of which any builder mutates. Builder `t`
//! owns batches `t, t + threads, …` on a channel of its own, and the card
//! reads the channels round-robin, so batch `k` is always the k-th delivered.

use std::num::NonZeroUsize;
use std::sync::mpsc::{sync_channel, Receiver};
use std::thread::{self, Scope};
use std::time::Instant;

use anyhow::Result;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;
use tile_painter::region_runner::{batch_slot, block_batch_origin};
use tile_painter::source_loader_structure::bake_tile_vector_rx_refl;

use crate::source_frame::{DeviceLineSource, RegionMetricFrame};
use crate::tile_source_incidence::{no_source_reaches_tiles, TileMetricLattice};

/// Builder threads, and with them the host's lookahead: every channel is a
/// rendezvous, so a builder holds its finished batch until the card takes it
/// and the host never carries more than `BUILDER_THREADS + 1` batches — the
/// one being painted included.
///
/// A 4x4 z13 batch with the surface painter's 10 km line halo is 77.3 MB at the
/// latitude of the world's dense cells and 110.2 MB at its worst — z13 row
/// 8188, the southernmost row any `gpu-surface` cell of this run owns, where a
/// degree of longitude is a tenth of its equatorial width and the shared halo
/// is that much wider (measured with `TileBatch::estimate_heap_bytes`, which is
/// the build's own sizing). The single producer already held three batches (one
/// building, one queued, one painting), so three builders cost one batch of
/// host memory more than it did and prepare three blocks at once — enough for
/// the hosts above, where the card was busy for only 42-69 % of the wall. A
/// host with fewer cores than that gets one builder per core instead.
///
/// Raising it buys nothing measurable: on the reference card with the workset
/// in tmpfs, eight builders paint the wbench-2 surface lane and the five most
/// raster-bound cells of the 2026-09-06 pass in the same wall as three
/// (59.0 s vs 59.0 s over those five cells, two rounds each), because three
/// already hide the raster prepare behind the per-tile receiver and partition
/// work that is the real critical path there.
const BUILDER_THREADS: usize = 3;

/// One block of a cell's tile grid: the block's north-west corner, which sizes
/// the shared terrain halo, and the tiles of that block the cell owns.
pub type BatchRequest = ((u32, u32), Vec<(u32, u32)>);

/// One batch on its way to the card: the tiles of that block the cell owns,
/// which of the cell's layers have a source anywhere near them, the built
/// rasters, and what building them cost.
pub struct ReadyBatch {
    pub requested_tiles: Vec<(u32, u32)>,
    /// One flag per entry of the cell's layer list, in that order.
    pub reached_layers: Vec<bool>,
    /// `None` when no layer is reached: every requested tile is then the
    /// all-`NO_DATA` tile in every layer, so the shared halo, the sixteen inner
    /// cores and the facade bake are never built for it.
    pub rasters: Option<TileBatch>,
    pub prepare_seconds: f64,
}

/// Start the builders over `batches` in paint order and return the channels the
/// card reads them back from: batch `index` arrives on channel
/// `index % channels.len()`, so reading the channels round-robin restores the
/// batch order whatever the threads did.
///
/// `layer_sources` holds one region-wide source list per painted layer, in the
/// cell's layer order; a batch no list reaches carries no rasters.
#[allow(clippy::too_many_arguments)]
pub fn spawn_batch_raster_builders<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    batches: &'env [BatchRequest],
    zoom: u8,
    batch_side: u32,
    halo_m: f64,
    frame: &'env RegionMetricFrame,
    layer_sources: &'env [&'env [DeviceLineSource]],
    rasters: &'env RealRasters,
    obstacles: &'env ObstacleSet,
) -> Result<Vec<Receiver<ReadyBatch>>> {
    let threads = BUILDER_THREADS
        .min(thread::available_parallelism().map_or(1, NonZeroUsize::get))
        .min(batches.len().max(1));
    let mut channels = Vec::with_capacity(threads);
    for start in 0..threads {
        let (sender, receiver) = sync_channel(0);
        thread::Builder::new()
            .name(format!("batch-rasters-{start}"))
            .spawn_scoped(scope, move || {
                for index in (start..batches.len()).step_by(threads) {
                    let ((block_x, block_y), requested_tiles) = &batches[index];
                    let started = Instant::now();
                    let lattices: Vec<TileMetricLattice> = requested_tiles
                        .iter()
                        .map(|&(x, y)| TileMetricLattice::for_tile(frame, zoom, x, y))
                        .collect();
                    let reached_layers: Vec<bool> = layer_sources
                        .iter()
                        .map(|sources| !no_source_reaches_tiles(sources, &lattices))
                        .collect();
                    let built = reached_layers.iter().any(|reached| *reached).then(|| {
                        let (base_x, base_y) =
                            block_batch_origin(*block_x, *block_y, batch_side, zoom);
                        let mut batch = TileBatch::build_opt_rx_refl(
                            zoom, base_x, base_y, batch_side, halo_m, rasters,
                        );
                        for &(x, y) in requested_tiles {
                            let slot = batch_slot(&batch, x, y);
                            bake_tile_vector_rx_refl(&mut batch.tiles[slot], obstacles);
                        }
                        batch
                    });
                    let ready = ReadyBatch {
                        requested_tiles: requested_tiles.clone(),
                        reached_layers,
                        rasters: built,
                        prepare_seconds: started.elapsed().as_secs_f64(),
                    };
                    if sender.send(ready).is_err() {
                        return;
                    }
                }
            })?;
        channels.push(receiver);
    }
    Ok(channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Reading the channels round-robin must hand the card exactly the batch
    /// list, in order, however many builders the box got — the whole reason a
    /// batch's painted bytes cannot depend on the thread count. Driven through
    /// the real builders over an empty raster root, which samples every raster
    /// as absent: this is about delivery, not about levels.
    #[test]
    fn round_robin_delivers_every_batch_in_list_order() {
        let rasters = RealRasters::new(Path::new("/nonexistent-prepared-root"));
        let obstacles = ObstacleSet::empty();
        let frame = RegionMetricFrame::for_latitude_longitude(49.78, 14.17);
        let batches: Vec<BatchRequest> = (0..7)
            .map(|step| ((4412 + step, 2784), vec![(4412 + step, 2784)]))
            .collect();
        // A source over every tile of the sweep, so each batch really builds
        // its rasters instead of being screened out as silent.
        let sources: Vec<DeviceLineSource> = batches
            .iter()
            .map(|&((x, y), _)| {
                let lattice = TileMetricLattice::for_tile(&frame, 13, x, y);
                let [west, south, east, north] = lattice.outer_neighbourhood_rectangle();
                DeviceLineSource {
                    start_x_m: (west + east) * 0.5,
                    start_y_m: (south + north) * 0.5,
                    end_x_m: (west + east) * 0.5,
                    end_y_m: (south + north) * 0.5,
                    max_distance_m: 1.0,
                    ..DeviceLineSource::default()
                }
            })
            .collect();
        let layer_sources: Vec<&[DeviceLineSource]> = vec![&sources];
        let delivered = thread::scope(|scope| -> Result<Vec<((u32, u32), bool)>> {
            let channels = spawn_batch_raster_builders(
                scope,
                &batches,
                13,
                1,
                30.0,
                &frame,
                &layer_sources,
                &rasters,
                &obstacles,
            )?;
            (0..batches.len())
                .map(|index| {
                    let ready = channels[index % channels.len()].recv()?;
                    Ok((ready.requested_tiles[0], ready.rasters.is_some()))
                })
                .collect()
        })
        .expect("every batch reaches the card");
        let expected: Vec<((u32, u32), bool)> =
            batches.iter().map(|(block, _)| (*block, true)).collect();
        assert_eq!(delivered, expected);
    }

    /// A batch no layer reaches is delivered in its place with no rasters at
    /// all: the halo, the inner cores and the facade bake are the work this
    /// screen exists to skip.
    #[test]
    fn a_batch_no_source_reaches_carries_no_rasters() {
        let rasters = RealRasters::new(Path::new("/nonexistent-prepared-root"));
        let obstacles = ObstacleSet::empty();
        let frame = RegionMetricFrame::for_latitude_longitude(49.78, 14.17);
        let batches: Vec<BatchRequest> = vec![((4412, 2784), vec![(4412, 2784)])];
        let far_away = [DeviceLineSource {
            start_x_m: 400_000.0,
            start_y_m: 400_000.0,
            end_x_m: 400_100.0,
            end_y_m: 400_100.0,
            max_distance_m: 1_000.0,
            ..DeviceLineSource::default()
        }];
        let layer_sources: Vec<&[DeviceLineSource]> = vec![&far_away, &[]];
        let ready = thread::scope(|scope| -> Result<ReadyBatch> {
            let channels = spawn_batch_raster_builders(
                scope,
                &batches,
                13,
                1,
                30.0,
                &frame,
                &layer_sources,
                &rasters,
                &obstacles,
            )?;
            Ok(channels[0].recv()?)
        })
        .expect("the batch reaches the card");
        assert_eq!(ready.reached_layers, vec![false, false]);
        assert!(ready.rasters.is_none());
    }
}
