//! GPU build stage for the gpu-airborne bin: scatter a prepped cell's candidate SoA into
//! per-tile accumulators and write HM3 tiles at the requested zoom. Routes one-pass vs the M2
//! chunked build, and exposes `process_region_gpu` (the batch `par_chunks` per-cell prep+build).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use h3o::CellIndex;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_gpu::airborne::{
    for_each_region_chunk, is_cell_unbuildable, AirborneGpu, AirborneScreeningEnvironment,
};
use noise_gpu::pack_airborne_segs;
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::r4_source_cache::R4SourceCache;
use tile_painter::source_loader_structure::InteriorEstimate;
use tile_painter::wire_hm3::{collapse_lden_u8, write_tile, SOURCE_ID_AIRCRAFT};

use crate::prep::{build_dem_blocks, prep_cell, PrepBlock, PreparedCell};
use crate::Args;

/// Host-wall split of the GPU build half of one cell. The persistent stream preserves these fields
/// per cell and aggregates them every 64 cells while distinguishing device input, scatter/copyback,
/// and output sealing from CPU preparation.
#[derive(Clone, Copy, Default)]
pub(crate) struct BuildTimings {
    /// Chunked-only reload of the source ring plus construction of borrowed source views.
    pub(crate) source_load: Duration,
    /// Chunked-only residual wall for candidate enumeration/preparation. This deliberately
    /// includes the loop/callback bookkeeping that cannot be split without changing the hot path.
    pub(crate) candidate_prepare_composite: Duration,
    /// Chunked-only packing of prepared candidates into the device SoA.
    pub(crate) pack: Duration,
    /// Chunked-only DEM receiver-grid construction.
    pub(crate) raster: Duration,
    pub(crate) accumulator_init: Duration,
    pub(crate) upload: Duration,
    pub(crate) scatter: Duration,
    pub(crate) seal: Duration,
}

pub(crate) struct BuiltCell {
    pub(crate) written: usize,
    pub(crate) skipped: usize,
    pub(crate) output_bytes: usize,
    pub(crate) timings: BuildTimings,
}

impl BuiltCell {
    fn empty() -> Self {
        Self {
            written: 0,
            skipped: 0,
            output_bytes: 0,
            timings: BuildTimings::default(),
        }
    }
}

/// M2 candidate-chunk size: how many sub-segs to upload per VRAM pass for a too-big cell, DERIVED
/// from the card's VRAM (like `default_batch_size` from L3 — no hand-set knob). A chunk's VRAM ≈ its
/// SoA + one tile-block's scatter scratch — empirically ~7 GB at 64M on the 11 GB fleet floor — so
/// budget `(vram − 4 GB headroom for the NPD LUTs + sources + scratch) ÷ 117 B/cand`, clamped to keep
/// the far-list offset (cand × batch_n², batch_n ≤ 4) < 2^31 and the host peak (~208 B/cand) sane. So
/// an 11 GB card → ~64M (~5 passes for Phoenix's 308M; the wall is the per-pass scatter calls, not
/// CPU prep), a 24 GB card → the 120M cap (~3 passes). Each pass's
/// per-tile energy is `merge_from`-summed (additive), reconstructing the one-pass result on ANY card.
/// `NOISE_GPU_AIRBORNE_CHUNK` stays ONLY as a test override (force many small passes to parity-test
/// the accumulation), not a tuning knob.
pub(crate) fn max_candidates_per_chunk(vram_total_bytes: u64) -> usize {
    if let Some(n) = std::env::var("NOISE_GPU_AIRBORNE_CHUNK")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n;
    }
    const HEADROOM: u64 = 4 << 30; // NPD LUTs + sources + scatter scratch + margin
    const BYTES_PER_CAND: u64 = 117; // measured: 64M ≈ 7 GB usable on the 11 GB floor
    let usable = vram_total_bytes.saturating_sub(HEADROOM);
    ((usable / BYTES_PER_CAND) as usize).clamp(8_000_000, 120_000_000)
}

/// Seal one airborne tile: collapse the accumulator to Lden bytes, then stamp the tile's
/// building-interior display estimate. Split out of [`write_tile_accumulator`] so the stamp is
/// pinned by a unit test with no `Args` or filesystem round trip.
fn aircraft_tile_cells(
    accum: &TileAccumulator,
    n_days: u16,
    interior: Option<&InteriorEstimate>,
) -> Vec<u8> {
    let mut cells = collapse_lden_u8(accum, n_days as f64);
    if let Some(interior) = interior {
        interior.apply(&mut cells);
    }
    cells
}

/// Collapse one tile's accumulator to Lden bytes, stamp the tile's building-interior display
/// estimate, and write it — or, if the (re)build shrank the tile to silence, unlink any stale prior
/// tile so an incremental recombine/pyramid can't read phantom energy (mirrors the CPU builder).
/// Returns exact bytes written (zero for silence). One source of truth for the write + stale-unlink,
/// shared by the one-pass (`gpu_build_cell_one_pass`) and M2 chunked builds.
///
/// The interior stamp is display semantics, not physics: an enclosed receiver shows its façade
/// donor's value minus the envelope ΔL. EVERY layer of a tile must carry it, or the layers disagree
/// indoors and the energy-summed `total` combine no longer commutes with the envelope loss — the
/// CPU aircraft builder (`region_runner`) and both surface builders always did, this writer did not,
/// so it painted the airborne layer 20–35 dB louder than road/rail inside every footprint. Measured
/// against the W2 exact-CPU etalon on 2026-08-28: 1 921 cells at exactly +20.0 dB (unclassified
/// footprints ≤ 6 m, `EnvelopeClass::Industrial`) and 72 at exactly +25.0 dB (> 6 m, `Default`) on
/// tile 13/4406/2782 alone.
fn write_tile_accumulator(
    args: &Args,
    n_days: u16,
    tx: u32,
    ty: u32,
    accum: &TileAccumulator,
    interior: &InteriorEstimate,
) -> Result<usize> {
    let out = args
        .output
        .join(args.zoom.to_string())
        .join(tx.to_string())
        .join(format!("{ty}.bin"));
    let cells = aircraft_tile_cells(accum, n_days, Some(interior));
    let written = write_tile(&out, &cells, SOURCE_ID_AIRCRAFT, !args.write_empty)?;
    if written > 0 {
        Ok(written)
    } else {
        if out.exists() {
            std::fs::remove_file(&out).with_context(|| format!("rm stale {}", out.display()))?;
        }
        Ok(0)
    }
}

/// One zeroed `TileAccumulator` per owned tile, parallel to each block's `btiles` — the running sum a
/// build folds its candidate chunk(s) into.
fn new_running(blocks: &[PrepBlock]) -> Vec<Vec<TileAccumulator>> {
    blocks
        .iter()
        .map(|b| b.btiles.iter().map(|_| TileAccumulator::new()).collect())
        .collect()
}

fn upload_cell_screening_environment(
    gpu: &AirborneGpu,
    blocks: &[PrepBlock],
    obstacles: &ObstacleSet,
) -> Result<AirborneScreeningEnvironment> {
    let shared_halo = &blocks[0].batch.tiles[0].halo;
    assert!(
        blocks.iter().all(|block| block
            .batch
            .tiles
            .iter()
            .all(|tile| std::sync::Arc::ptr_eq(&tile.halo, shared_halo))),
        "prepared airborne cell must share one elevation halo"
    );
    gpu.upload_screening_environment(obstacles, shared_halo)
}

/// Upload ONE candidate chunk's SoA, scatter every block against it, and ADD each block's per-tile
/// energy into `running` (`merge_from` — additive in the linear domain). `resident` drops at function
/// end, freeing the chunk's VRAM before the caller's next chunk. THE shared scatter core: the
/// one-pass build folds a single whole-region chunk; the M2 chunked build folds many. An empty SoA is
/// fine — the absent environment takes the zero-output path (which still stale-unlinks at write).
fn scatter_chunk_into_running(
    gpu: &AirborneGpu,
    blocks: &[PrepBlock],
    environment: Option<&AirborneScreeningEnvironment>,
    running: &mut [Vec<TileAccumulator>],
    sll: Vec<f64>,
    sf: Vec<f32>,
    si: Vec<i32>,
    nreg: usize,
) -> Result<(Duration, Duration)> {
    let upload_started = Instant::now();
    let resident = gpu.upload_region(sll, sf, si, nreg)?;
    let upload = upload_started.elapsed();
    let scatter_started = Instant::now();
    assert_eq!(environment.is_none(), resident.is_empty());
    for (block, run) in blocks.iter().zip(running.iter_mut()) {
        let accums = if let Some(environment) = environment {
            gpu.scatter_region_with_environment(
                &resident,
                &block.tile_refs(),
                environment,
                &block.interiors,
            )?
        } else {
            block
                .btiles
                .iter()
                .map(|_| TileAccumulator::new())
                .collect()
        };
        for (acc_run, acc_chunk) in run.iter_mut().zip(accums.iter()) {
            acc_run.merge_from(acc_chunk);
        }
    }
    Ok((upload, scatter_started.elapsed()))
}

/// Collapse + write every accumulated tile (`write_tile_accumulator`: write, else stale-unlink),
/// returning (written, skipped). The shared tail of both build paths.
fn write_running(
    args: &Args,
    n_days: u16,
    blocks: &[PrepBlock],
    running: &[Vec<TileAccumulator>],
) -> Result<(usize, usize, usize)> {
    let (mut written, mut skipped, mut output_bytes) = (0usize, 0usize, 0usize);
    for (block, run) in blocks.iter().zip(running.iter()) {
        for (slot, &(tx, ty)) in block.btiles.iter().enumerate() {
            let interior = &block.interiors[slot];
            let bytes = write_tile_accumulator(args, n_days, tx, ty, &run[slot], interior)?;
            if bytes > 0 {
                written += 1;
                output_bytes += bytes;
            } else {
                skipped += 1;
            }
        }
    }
    Ok((written, skipped, output_bytes))
}

/// GPU build stage for a one-pass (fits-one-pass) cell: fold its single whole-region SoA chunk into
/// the running accumulators, then write. The one-pass case is just "one chunk" of the same fold+write
/// the M2 chunked path uses (`scatter_chunk_into_running` + `write_running`) — `merge_from` from a
/// zeroed accumulator is an exact f32 copy, so output is byte-identical to a direct write. Empty /
/// silent regions are NOT skipped early: the single (possibly empty) fold zeros + stale-unlinks every
/// tile (a bare `continue` would leave ghost tiles in an incremental rebuild).
pub(crate) fn gpu_build_cell_one_pass(
    gpu: &AirborneGpu,
    args: &Args,
    n_days: u16,
    p: PreparedCell,
) -> Result<BuiltCell> {
    // No owned tiles (off-grid cell) → nothing to scatter; skip the device upload (matches the serial
    // path's early Ok((0,0))). A cell WITH tiles but zero candidates still folds its empty SoA below.
    if p.blocks.is_empty() {
        return Ok(BuiltCell::empty());
    }
    let accumulator_started = Instant::now();
    let mut running = new_running(&p.blocks);
    let accumulator_init = accumulator_started.elapsed();
    let obstacles = p
        .obstacles
        .as_deref()
        .expect("non-empty prepared cell has vector obstacles");
    let (environment, environment_upload) = if p.nreg > 0 {
        let started = Instant::now();
        let environment = upload_cell_screening_environment(gpu, &p.blocks, obstacles)?;
        (Some(environment), started.elapsed())
    } else {
        (None, Duration::ZERO)
    };
    let (candidate_upload, scatter) = scatter_chunk_into_running(
        gpu,
        &p.blocks,
        environment.as_ref(),
        &mut running,
        p.sll,
        p.sf,
        p.si,
        p.nreg,
    )?;
    let upload = environment_upload + candidate_upload;
    let seal_started = Instant::now();
    let (written, skipped, output_bytes) = write_running(args, n_days, &p.blocks, &running)?;
    Ok(BuiltCell {
        written,
        skipped,
        output_bytes,
        timings: BuildTimings {
            accumulator_init,
            upload,
            scatter,
            seal: seal_started.elapsed(),
            ..BuildTimings::default()
        },
    })
}

/// M2 chunked build (the fallback for a cell whose full region won't fit one host/VRAM pass): build
/// the cell in [`max_candidates_per_chunk`]-sized candidate passes (VRAM-derived, not a knob),
/// summing each pass's per-tile energy
/// into running accumulators (`TileAccumulator::merge_from` — additive in the linear domain, so the
/// sum reconstructs the one-pass result), then write once. Unlike the A2 fast path this re-loads the
/// region's sources HERE (the GPU thread's own cache), since a too-big cell never crossed the prep
/// channel with a packed SoA — accepted: it's the ~5 densest cells of 44k, so the lost prep-ahead is
/// noise. Bounds host RAM to the source Arcs + one chunk's candidates, and VRAM to one chunk's SoA +
/// a block's scatter scratch — so even an 11 GB / a 16 GB card builds Phoenix. Routed to by
/// BOTH triggers: `prep_cell`'s host-budget guard (`too_big`) and a one-pass VRAM limit
/// (`is_cell_unbuildable` from `gpu_build_cell_one_pass`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn gpu_build_cell_chunked(
    gpu: &AirborneGpu,
    cache: &mut R4SourceCache,
    rasters: &RealRasters,
    args: &Args,
    n_days: u16,
    z: u8,
    bn: u32,
    r4: u64,
    tiles: &[(u32, u32)],
) -> Result<BuiltCell> {
    // `tiles` is the cell's owned-tile worklist (passed in, NOT recomputed) so a dev `--bbox`/
    // `--tile-x` subset that falls through to the chunked path builds the SAME tiles the one-pass
    // path would — production passes `region_tiles(r4,z)` (the whole cell), so it's unchanged there.
    if tiles.is_empty() {
        return Ok(BuiltCell::empty());
    }
    // Load the region's grid_disk(1) airborne sources (held for the whole chunk loop, since every
    // chunk re-reads them — the cheap part; the expensive candidate Vec is what we chunk).
    let source_load_started = Instant::now();
    let cell = CellIndex::try_from(r4)?;
    let mut arcs = Vec::with_capacity(7);
    for nbr in cell.grid_disk::<Vec<_>>(1) {
        arcs.push(cache.get_or_load(u64::from(nbr))?);
    }
    let views: Vec<_> = arcs.iter().flat_map(|a| a.airborne.views()).collect();
    let source_load = source_load_started.elapsed();

    // DEM tile-blocks + interior estimates (same topology and stamp as the one-pass prep, built
    // ONCE, reused across chunks) + a zeroed running accumulator per owned tile.
    let raster_started = Instant::now();
    let (blocks, obstacles) = build_dem_blocks(rasters, &args.h3r4_dir, z, bn, r4, tiles)?;
    let raster = raster_started.elapsed();
    let accumulator_started = Instant::now();
    let mut running = new_running(&blocks);
    let accumulator_init = accumulator_started.elapsed();
    let mut pack = Duration::ZERO;
    let environment_started = Instant::now();
    let environment = upload_cell_screening_environment(gpu, &blocks, &obstacles)?;
    let environment_upload = environment_started.elapsed();
    let mut candidate_upload = Duration::ZERO;
    let mut scatter = Duration::ZERO;
    // Fold each VRAM-sized candidate chunk into the running accumulators — the SAME scatter core the
    // one-pass build runs once, here run once per chunk (additive, so the sum = the one-pass result).
    let candidate_loop_started = Instant::now();
    for_each_region_chunk(
        &views,
        r4,
        z,
        max_candidates_per_chunk(gpu.vram_total_bytes()),
        |chunk| {
            let nreg = chunk.len();
            let pack_started = Instant::now();
            let (sll, sf, si) = pack_airborne_segs(&chunk);
            pack += pack_started.elapsed();
            drop(chunk);
            let (chunk_upload, chunk_scatter) = scatter_chunk_into_running(
                gpu,
                &blocks,
                Some(&environment),
                &mut running,
                sll,
                sf,
                si,
                nreg,
            )?;
            candidate_upload += chunk_upload;
            scatter += chunk_scatter;
            Ok(())
        },
    )?;
    // Candidate construction happens between callback invocations inside for_each_region_chunk.
    // Subtract the callback's explicitly measured pack/device work from the enclosing host wall;
    // the honest remainder is candidate preparation plus small loop/callback bookkeeping.
    let candidate_prepare_composite = candidate_loop_started
        .elapsed()
        .saturating_sub(pack + candidate_upload + scatter);
    let upload = environment_upload + candidate_upload;
    let seal_started = Instant::now();
    let (written, skipped, output_bytes) = write_running(args, n_days, &blocks, &running)?;
    Ok(BuiltCell {
        written,
        skipped,
        output_bytes,
        timings: BuildTimings {
            source_load,
            candidate_prepare_composite,
            pack,
            raster,
            accumulator_init,
            upload,
            scatter,
            seal: seal_started.elapsed(),
        },
    })
}

/// Build every owned tile of one region on the GPU (the BATCH `par_chunks` path): CPU-prep then
/// GPU-build, SERIAL — `prep_cell` + `gpu_build_cell_one_pass` share the exact code the stream pipeline
/// splits across two threads, so this path is unchanged in behaviour. `tiles` is the cell's owned
/// tile list (`region_tiles` for `--regions-file`, a bbox/single-tile subset for the dev modes),
/// forwarded to `prep_cell` so the dev paths keep their explicit subset; the build-wide
/// `!any_source_arrow` guard in `main` already returns Ok(()) before any GPU work for a
/// no-airborne chunk.
/// Route a prepped cell to its build path: one-pass (`gpu_build_cell_one_pass`) for the common case, else the
/// M2 chunked build (`gpu_build_cell_chunked`) when the cell is too big for ONE host pass (`too_big`,
/// set by `prep_cell`'s host-budget guard) OR hits a one-pass VRAM limit (`is_cell_unbuildable` — a
/// card too small for the SoA; `upload_region` OOMs before any tile is written → clean rebuild). The
/// ONE place this two-trigger routing lives, shared by the batch (`process_region_gpu`) and stream
/// (`run_stream`) paths so the two can't drift.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_prepared_cell(
    gpu: &AirborneGpu,
    cache: &mut R4SourceCache,
    rasters: &RealRasters,
    args: &Args,
    n_days: u16,
    z: u8,
    bn: u32,
    r4: u64,
    p: PreparedCell,
    tiles: &[(u32, u32)],
) -> Result<BuiltCell> {
    if p.too_big {
        return gpu_build_cell_chunked(gpu, cache, rasters, args, n_days, z, bn, r4, tiles);
    }
    match gpu_build_cell_one_pass(gpu, args, n_days, p) {
        Err(e) if is_cell_unbuildable(&e) => {
            gpu_build_cell_chunked(gpu, cache, rasters, args, n_days, z, bn, r4, tiles)
        }
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn process_region_gpu(
    gpu: &AirborneGpu,
    cache: &mut R4SourceCache,
    rasters: &RealRasters,
    args: &Args,
    z: u8,
    bn: u32,
    n_days: u16,
    r4: u64,
    tiles: &[(u32, u32)],
) -> Result<(usize, usize)> {
    let p = prep_cell(rasters, cache, &args.h3r4_dir, z, bn, r4, tiles)?;
    let built = build_prepared_cell(gpu, cache, rasters, args, n_days, z, bn, r4, p, tiles)?;
    Ok((built.written, built.skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::envelope::EnvelopeClass;
    use raster_reader::fused_tile_z13::TILE_PX;
    use tile_painter::wire_hm3::{dequantise_lden, quantise_lden};

    /// A tile writer that forgets the interior stamp paints the airborne layer 20-35 dB louder
    /// than every other layer inside every building footprint — the defect this file carried
    /// until 2026-08-28. Uniform energy over the whole tile makes the façade donor's value
    /// independent of which outdoor pixel the distance transform picks, so the assertion tests
    /// the stamp and nothing else.
    #[test]
    fn the_sealed_tile_carries_the_building_interior_stamp() {
        const ENCLOSED: usize = 5 * TILE_PX + 7;
        let n_days = 12u16;
        let mut accum = TileAccumulator::new();
        for py in 0..TILE_PX as u32 {
            for px in 0..TILE_PX as u32 {
                accum.add_energy_at(py, px, 0, 1.0e12);
            }
        }
        let mut classes = vec![EnvelopeClass::Outdoor as u8; TILE_PX * TILE_PX];
        classes[ENCLOSED] = EnvelopeClass::Default as u8;
        let interior = InteriorEstimate::from_classes(classes);

        let bare = aircraft_tile_cells(&accum, n_days, None);
        let stamped = aircraft_tile_cells(&accum, n_days, Some(&interior));

        let facade = dequantise_lden(bare[ENCLOSED]);
        assert!(
            facade > 30.0,
            "fixture must sit above the render floor, got {facade} dB"
        );
        let delta = EnvelopeClass::Default
            .delta_db()
            .expect("enclosed class has a delta");
        assert_eq!(stamped[ENCLOSED], quantise_lden(facade - delta));
        assert_ne!(
            stamped[ENCLOSED], bare[ENCLOSED],
            "the stamp must change the enclosed pixel"
        );
        for (i, (&sealed, &raw)) in stamped.iter().zip(bare.iter()).enumerate() {
            if i != ENCLOSED {
                assert_eq!(sealed, raw, "outdoor pixel {i} must keep its façade value");
            }
        }
    }
}
