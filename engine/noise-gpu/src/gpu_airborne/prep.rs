//! CPU prep for gpu-airborne: pack candidates and build receiver/terrain inputs.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use h3o::CellIndex;
use noise_compute::emission::aircraft::SegmentPrepared;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_gpu::airborne::region_candidates;
use noise_gpu::pack_airborne_segs;
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBatch};
use raster_reader::RealRasters;
use rayon::prelude::*;
use tile_painter::r4_source_cache::R4SourceCache;
use tile_painter::region_runner::group_tiles_into_batches;
use tile_painter::source_loader_structure::{InteriorEstimate, StructureData};

#[path = "horizon_halo.rs"]
mod horizon_halo;
use horizon_halo::cell_horizon_halos;

/// One grid-aligned tile-block, CPU-prepped: its NW corner `(bx,by)`, the owned tiles in it,
/// the receiver-altitude `TileBatch` with its 8 km DEM halo, and one building-interior
/// estimate per owned tile.
pub(crate) struct PrepBlock {
    pub(crate) bx: u32,
    pub(crate) by: u32,
    pub(crate) btiles: Vec<(u32, u32)>,
    pub(crate) batch: TileBatch,
    /// Per owned tile, in `btiles` order: the façade-donor map `write_tile_accumulator` stamps
    /// onto the collapsed tile. Baked HERE so the work overlaps GPU scatter, the same placement
    /// the surface runner uses.
    pub(crate) interiors: Vec<InteriorEstimate>,
}

impl PrepBlock {
    /// The `&FusedTileZ13` receiver grids of this block's owned tiles, in `btiles` order. The
    /// `((ty-by)*bn + (tx-bx))` index math lives ONCE here — the scatter and the interior bake
    /// must address the same tile, and a divergence would silently pair a tile with another
    /// tile's receivers.
    pub(crate) fn tile_refs(&self) -> Vec<&FusedTileZ13> {
        let bn = self.batch.batch_n;
        self.btiles
            .iter()
            .map(|&(tx, ty)| &self.batch.tiles[((ty - self.by) * bn + (tx - self.bx)) as usize])
            .collect()
    }
}

/// One cell's CPU prep output, handed across the A2 channel to the GPU thread. Holds the
/// pre-packed region SoA (`sll`/`sf`/`si`, ready for `upload_region` — `region` itself is
/// dropped after packing; the stream path never reads it) and the prepped tile-blocks.
/// `Send` because every field is a `Vec`/`usize`/`Instant` and `TileBatch` is `Vec`s of
/// primitives + `Arc<FusedGrid>` (`FusedGrid: Send+Sync`). `t_start` is stamped at the START
/// of prep so the reported ms = prep+build wall time per cell, matching the serial `done` line.
/// (The cell's `r4` is NOT a field: the stream channel carries it alongside as `(u64, Result<_>)`
/// so the GPU thread can report `fail`/FATAL even when prep itself errored and produced no cell.)
pub(crate) struct PreparedCell {
    pub(crate) sll: Vec<f64>,
    pub(crate) sf: Vec<f32>,
    pub(crate) si: Vec<i32>,
    pub(crate) nreg: usize,
    pub(crate) blocks: Vec<PrepBlock>,
    pub(crate) obstacles: Option<Arc<ObstacleSet>>,
    pub(crate) t_start: Instant,
    pub(crate) timings: PrepTimings,
    /// M2: the region's full candidate Vec wouldn't fit one host/VRAM pass, so prep produced NO SoA
    /// — the GPU stage rebuilds + builds this cell CHUNKED (`gpu_build_cell_chunked`) instead. Set
    /// only for the ~5 densest megahubs; every other cell stays the one-pass A2 fast path.
    pub(crate) too_big: bool,
}

/// CPU-prep wall split emitted by the persistent stream. `candidates` includes loading the seven
/// source cells because source decode/cache misses are part of the same pre-GPU bottleneck, and
/// `dem` likewise covers ALL per-tile receiver-lattice prep — the DEM grids plus the region's
/// obstacle-shard read and the per-tile interior bake, which are the same per-tile class of work.
#[derive(Clone, Copy, Default)]
pub(crate) struct PrepTimings {
    pub(crate) candidates: Duration,
    pub(crate) pack: Duration,
    pub(crate) dem: Duration,
}

impl PrepTimings {
    pub(crate) fn total(self) -> Duration {
        self.candidates + self.pack + self.dem
    }
}

/// (memory.max, memory.current) of this process's cgroup-v2 scope, in bytes — the live memcap
/// budget the memcap wrapper set. None if unreadable or unlimited ("max").
fn cgroup_mem() -> Option<(u64, u64)> {
    let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?; // "0::/user.slice/…/run-XXX.scope"
    let rel = cg.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let base = format!("/sys/fs/cgroup{rel}");
    let max = std::fs::read_to_string(format!("{base}/memory.max")).ok()?;
    let max = max.trim().parse::<u64>().ok()?; // "max" → parse fails → None (no cap)
    let cur = std::fs::read_to_string(format!("{base}/memory.current"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some((max, cur))
}

/// Read a `/proc/meminfo` "Key:   N kB" line as bytes.
fn meminfo_bytes(mi: &str, key: &str) -> Option<u64> {
    mi.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|r| r.trim().strip_suffix(" kB"))
        .and_then(|n| n.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

/// Host-memory budget (max, current) in bytes for the region-Vec guard: the cgroup memcap when the
/// engine runs under one, ELSE physical RAM from `/proc/meminfo` (`MemTotal` as the cap, `MemTotal
/// − MemAvailable` as current — `MemAvailable` already discounts reclaimable page cache, e.g. the
/// scratch arrows, so it is the genuinely-used floor). The fallback is the fix for the 2026-06-21
/// SIGABRT crash: a vast docker container's `memory.max` is often "max" (unlimited), so `cgroup_mem`
/// returned None and `prep_cell` SKIPPED the guard entirely → a megahub's tens-of-GB `region` Vec
/// exhausted physical RAM → Rust alloc abort (an UNcatchable SIGABRT that crash-loops the worker,
/// sealing nothing). Guarding against physical RAM makes a too-big cell graceful-skip
/// (`RegionTooDense` → `fail`) on ANY box, capped or not.
fn host_mem_budget() -> Option<(u64, u64)> {
    if let Some(cg) = cgroup_mem() {
        return Some(cg);
    }
    let mi = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = meminfo_bytes(&mi, "MemTotal:")?;
    let avail = meminfo_bytes(&mi, "MemAvailable:")?;
    Some((total, total.saturating_sub(avail)))
}

/// Pre-fault the tile DEM footprints, then batch them into grid-aligned receiver blocks with the
/// terrain-horizon reach and bake each tile's building-interior estimate against the region's
/// vector footprints. Requested-zoom tile batching, the receiver-altitude grid and
/// the interior bake use one path for both one-pass prep (`prep_cell`) and the M2 chunked build
/// (`gpu_build_cell_chunked`); a divergence would scatter megahubs against a different receiver
/// grid than one-pass cells, or stamp only one of the two paths. The footprints load HERE rather
/// than at the two call sites so neither can build blocks and forget the stamp.
pub(crate) fn build_dem_blocks(
    rasters: &RealRasters,
    h3r4_dir: &Path,
    z: u8,
    bn: u32,
    r4: u64,
    tiles: &[(u32, u32)],
) -> Result<(Vec<PrepBlock>, Arc<ObstacleSet>)> {
    let obstacles = load_region_structures(h3r4_dir, r4)?.shared_set();
    let blocks = prepare_receiver_blocks(rasters, z, bn, r4, tiles, &obstacles)?;
    Ok((blocks, obstacles))
}

/// The tile-dependent half of [`build_dem_blocks`], split out so it can be exercised for a
/// whole-cell and a windowed request side by side without a prepared structure table.
///
/// Everything a tile reads here is a function of the TILE and the CELL, never of the other
/// tiles in the request: its block comes from [`group_tiles_into_batches`], its terrain halo
/// from [`cell_horizon_halos`], and its receiver lattice and interior stamp from the tile alone.
fn prepare_receiver_blocks(
    rasters: &RealRasters,
    z: u8,
    bn: u32,
    r4: u64,
    tiles: &[(u32, u32)],
    obstacles: &ObstacleSet,
) -> Result<Vec<PrepBlock>> {
    let region_halos = cell_horizon_halos(rasters, z, r4)?;
    let batches = group_tiles_into_batches(tiles, bn);
    // Parallel across blocks: receiver-altitude construction is sequential by design ("the caller
    // usually parallelises across batches, not within them") and the interior bake adds a
    // point-in-footprint classify + an exact distance transform over all 512² receivers per tile.
    // Left serial, a 120-tile cell would put that whole cost on the ONE prep thread that has to
    // stay ahead of the device workers. Same shape as the surface runner's `par_iter` block prep.
    Ok(batches
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|((bx, by), btiles)| {
            let halo = usize::from(region_halos.len() == 2 && btiles[0].0 >= (1 << z) / 2);
            let mut block = PrepBlock {
                bx,
                by,
                btiles,
                batch: TileBatch::build_receiver_altitude_with_shared_halo(
                    z,
                    bx,
                    by,
                    bn,
                    rasters,
                    Arc::clone(&region_halos[halo]),
                ),
                interiors: Vec::new(),
            };
            let interiors = block
                .tile_refs()
                .into_iter()
                .map(|tile| InteriorEstimate::bake(tile, obstacles))
                .collect();
            block.interiors = interiors;
            block
        })
        .collect())
}

/// The region's vector building footprints — the SAME `grid_disk(1)` load the CPU aircraft
/// builder does (`region_runner::process_region`, which likewise re-derives the ring next to its
/// consumer so the painted cell is always inside it), so both writers stamp the identical
/// estimate. The airborne layer declares the per-cell `structures.arrow` in its read set: the
/// merged buildings ∪ walls table carries both the screening polygons and the OSM rows feeding
/// the low-profile cap. Screening only — the airborne layer never consumes the building
/// emission point stream.
fn load_region_structures(h3r4_dir: &Path, r4: u64) -> Result<StructureData> {
    let cell = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    StructureData::load_screening_for_r4s(h3r4_dir, r4, &ring)
        .with_context(|| format!("load structures R4 {r4:015x}"))
}

/// CPU prep stage for one cell (no GPU/device touch): load its grid_disk(1) airborne sources
/// through `cache`, `region_candidates` + pack the region SoA, then build receiver tiles with a
/// shared terrain halo and their building-interior estimates for `tiles` (the cell's owned tiles:
/// `region_tiles(r4,z)` on the stream/production path, a bbox/single-tile subset on the dev
/// paths). The packed SoA is uploaded later by
/// `gpu_build_cell_one_pass` via `upload_region`. `region` is dropped right after packing — only the SoA
/// crosses the channel, so host RAM per buffered cell is the SoA + its tile blocks, not the
/// prepared-segment Vec too. (Takes no `&Args`: the output dir + write-empty flag this stage's
/// serial predecessor referenced live in the GPU build stage, now the only stage that writes tiles.)
pub(crate) fn prep_cell(
    rasters: &RealRasters,
    cache: &mut R4SourceCache,
    h3r4_dir: &Path,
    z: u8,
    bn: u32,
    r4: u64,
    tiles: &[(u32, u32)],
) -> Result<PreparedCell> {
    let t_start = Instant::now();
    if tiles.is_empty() {
        // No owned tiles → nothing to upload or build; empty blocks + an empty SoA. The GPU
        // thread's `for` over `blocks` is a no-op (0 written, 0 skipped), same as the serial path.
        return Ok(PreparedCell {
            sll: Vec::new(),
            sf: Vec::new(),
            si: Vec::new(),
            nreg: 0,
            blocks: Vec::new(),
            obstacles: None,
            t_start,
            timings: PrepTimings::default(),
            too_big: false,
        });
    }
    let candidates_start = Instant::now();
    // Load the region's grid_disk(1) airborne sources (Arc'd — held only for this function's
    // lifetime so the merged views stay valid while we pack), then region-prep + pack ONCE.
    let cell = CellIndex::try_from(r4)?;
    let mut arcs = Vec::with_capacity(7);
    for nbr in cell.grid_disk::<Vec<_>>(1) {
        arcs.push(cache.get_or_load(u64::from(nbr))?);
    }
    let views: Vec<_> = arcs.iter().flat_map(|a| a.airborne.views()).collect();
    // HOST-memory guard (the host analog of `scatter_region`'s VRAM `RegionTooDense`): the ~5
    // densest megahub cells build a `region` Vec of tens of GB that spikes the engine past its
    // memory budget → an uncatchable cgroup SIGKILL (capped) or Rust alloc SIGABRT (uncapped) that
    // crash-loops the worker. Estimate the Vec's peak host bytes from the total sub-segment count
    // (its element upper bound) BEFORE allocating it; if it wouldn't fit the live budget, return
    // RegionTooDense — the SAME graceful per-cell skip path the VRAM limit uses (run_stream reports
    // `fail`, hub leaves the cell uncomputed; accepted for ~5 cells). The budget is the cgroup
    // memcap when capped, else physical RAM (`host_mem_budget` — the fix for the SIGABRT a vast
    // box's unlimited container `memory.max` used to cause). Conservative (×2 for the construction
    // peak + the packed SoA built from it, +4 GiB for the prep-ahead overlap cell): err toward
    // skipping a borderline cell, since a skipped cell is uncomputed (accepted) but an OOM crashes.
    // The tile blocks are DELIBERATELY outside this estimate: `build_dem_blocks` runs on BOTH
    // routes, so counting them could only push a cell to the chunked path that pays the same bytes
    // there. Receiver cores, shared 8 km horizon halos, interior estimates, and the depth-1 stream
    // overlap live in the +4 GiB fixed allowance instead; none scales with candidate count, so it
    // cannot decide between the one-pass and chunked candidate paths.
    if let Some((max, cur)) = host_mem_budget() {
        let n_sub: usize = views.iter().map(|v| v.sub_segments.start_lat.len()).sum();
        let est = (n_sub as u64)
            .saturating_mul(std::mem::size_of::<(SegmentPrepared, u8)>() as u64)
            .saturating_mul(2);
        if cur.saturating_add(est).saturating_add(4 << 30) > max {
            // M2: too big for ONE host pass → don't skip, build CHUNKED. Return a lightweight marker
            // (no SoA packed) so the GPU stage re-loads the region + builds it in VRAM-sized passes
            // (`gpu_build_cell_chunked`). The chunked path's host peak is the source Arcs + one
            // chunk, so it fits ANY box — this guard now routes, it no longer leaves cells uncomputed.
            eprintln!(
                "cell {r4:x}: region ~{} GiB for {} sub-segs exceeds one-pass host budget \
                 (cur {} + est > max {}) — building CHUNKED",
                est >> 30,
                n_sub,
                cur >> 30,
                max >> 30
            );
            return Ok(PreparedCell {
                sll: Vec::new(),
                sf: Vec::new(),
                si: Vec::new(),
                nreg: 0,
                blocks: Vec::new(),
                obstacles: None,
                t_start,
                timings: PrepTimings {
                    candidates: candidates_start.elapsed(),
                    ..PrepTimings::default()
                },
                too_big: true,
            });
        }
    }
    let region = region_candidates(&views, r4, z);
    let candidates = candidates_start.elapsed();
    let nreg = region.len();
    let pack_start = Instant::now();
    let (sll, sf, si) = pack_airborne_segs(&region);
    let pack = pack_start.elapsed();
    // The SoA is fully packed — the prepared-segment Vec (and the source Arcs/views it borrows)
    // are no longer needed for the stream/scatter_region path, so drop them to bound host RAM.
    drop(region);
    drop(views);
    drop(arcs);

    // Pre-fault the DEM footprint + batch the tiles with their horizon halo and bake their interior
    // estimates (shared with the M2 chunked build — one source of truth for the block topology and
    // the stamp, see `build_dem_blocks`). Timed as part of `dem`: it is per-tile receiver-lattice
    // prep, and it runs here rather than at write time so it overlaps the GPU scatter.
    let dem_start = Instant::now();
    let (blocks, obstacles) = build_dem_blocks(rasters, h3r4_dir, z, bn, r4, tiles)?;
    let dem = dem_start.elapsed();
    Ok(PreparedCell {
        sll,
        sf,
        si,
        nreg,
        blocks,
        obstacles: Some(obstacles),
        t_start,
        timings: PrepTimings {
            candidates,
            pack,
            dem,
        },
        too_big: false,
    })
}

#[cfg(test)]
#[path = "prep_tests.rs"]
mod tests;
