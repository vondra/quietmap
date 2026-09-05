//! One cell's tile-batch rasters built on several CPU threads and handed to the
//! card in batch order.
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

/// Builder threads, and with them the host's lookahead: every channel is a
/// rendezvous, so a builder holds its finished batch until the card takes it
/// and the host never carries more than `BUILDER_THREADS + 1` batches — the
/// one being painted included.
///
/// A 4x4 z13 batch with the surface painter's 10 km line halo is 77 MB at the
/// latitude of the world's dense cells and at most 88 MB anywhere it is
/// painted (measured with `TileBatch::estimate_heap_bytes`, which is the
/// build's own sizing). The single producer already held three batches (one
/// building, one queued, one painting), so three builders cost one batch of
/// host memory more than it did and prepare three blocks at once — enough for
/// the hosts above, where the card was busy for only 42-69 % of the wall. A
/// host with fewer cores than that gets one builder per core instead.
const BUILDER_THREADS: usize = 3;

/// One block of a cell's tile grid: the block's north-west corner, which sizes
/// the shared terrain halo, and the tiles of that block the cell owns.
pub type BatchRequest = ((u32, u32), Vec<(u32, u32)>);

/// One batch on its way to the card: the tiles of that block the cell owns,
/// the built rasters, and what building them cost.
pub struct ReadyBatch {
    pub requested_tiles: Vec<(u32, u32)>,
    pub batch: TileBatch,
    pub prepare_seconds: f64,
}

/// Start the builders over `batches` in paint order and return the channels the
/// card reads them back from: batch `index` arrives on channel
/// `index % channels.len()`, so reading the channels round-robin restores the
/// batch order whatever the threads did.
pub fn spawn_batch_raster_builders<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    batches: &'env [BatchRequest],
    zoom: u8,
    batch_side: u32,
    halo_m: f64,
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
                    let (base_x, base_y) = block_batch_origin(*block_x, *block_y, batch_side, zoom);
                    let mut batch = TileBatch::build_opt_rx_refl(
                        zoom, base_x, base_y, batch_side, halo_m, rasters,
                    );
                    for &(x, y) in requested_tiles {
                        let slot = batch_slot(&batch, x, y);
                        bake_tile_vector_rx_refl(&mut batch.tiles[slot], obstacles);
                    }
                    let ready = ReadyBatch {
                        requested_tiles: requested_tiles.clone(),
                        batch,
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
        let batches: Vec<BatchRequest> = (0..7)
            .map(|step| ((4412 + step, 2784), vec![(4412 + step, 2784)]))
            .collect();
        let delivered = thread::scope(|scope| -> Result<Vec<(u32, u32)>> {
            let channels =
                spawn_batch_raster_builders(scope, &batches, 13, 1, 30.0, &rasters, &obstacles)?;
            (0..batches.len())
                .map(|index| Ok(channels[index % channels.len()].recv()?.requested_tiles[0]))
                .collect()
        })
        .expect("every batch reaches the card");
        let expected: Vec<(u32, u32)> = batches.iter().map(|(block, _)| *block).collect();
        assert_eq!(delivered, expected);
    }
}
