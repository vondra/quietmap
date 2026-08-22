//! CPU prep stage for the gpu-airborne bin: load + pack a cell's candidate SoA, build its
//! DEM tile-blocks (no GPU/device touch). The output (`PreparedCell`) crosses the A2 channel
//! to the `build` stage; `build_dem_blocks` is shared with the M2 chunked build there.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use h3o::CellIndex;
use noise_compute::emission::aircraft::SegmentPrepared;
use noise_gpu::airborne::region_candidates;
use noise_gpu::pack_airborne_segs;
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;
use tile_painter::grid::tile_bbox;
use tile_painter::r4_source_cache::R4SourceCache;

/// One grid-aligned tile-block, CPU-prepped: its NW corner `(bx,by)`, the owned tiles in it,
/// and the DEM-only `TileBatch`. `gpu_build_cell_one_pass` rebuilds the `&FusedTileZ13` refs from
/// `batch.tiles` by the same `((ty-by)*bn + (tx-bx))` index the serial path used.
pub(crate) struct PrepBlock {
    pub(crate) bx: u32,
    pub(crate) by: u32,
    pub(crate) btiles: Vec<(u32, u32)>,
    pub(crate) batch: TileBatch,
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
    pub(crate) t_start: Instant,
    pub(crate) timings: PrepTimings,
    /// M2: the region's full candidate Vec wouldn't fit one host/VRAM pass, so prep produced NO SoA
    /// — the GPU stage rebuilds + builds this cell CHUNKED (`gpu_build_cell_chunked`) instead. Set
    /// only for the ~5 densest megahubs; every other cell stays the one-pass A2 fast path.
    pub(crate) too_big: bool,
}

/// CPU-prep wall split emitted by the persistent stream. `candidates` includes loading the seven
/// source cells because source decode/cache misses are part of the same pre-GPU bottleneck.
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

/// Pre-fault the tile DEM footprints, then batch them into grid-aligned DEM-only blocks.
/// `build_receiver_altitude_only` reads only `rx_alt_m`, skipping building/forest/IMD and
/// the halo a full build computes. Requested-zoom tile batching and the receiver-altitude grid
/// use one path for both one-pass prep (`prep_cell`) and the M2 chunked build
/// (`gpu_build_cell_chunked`); a divergence would scatter megahubs against a different receiver
/// grid than one-pass cells.
pub(crate) fn build_dem_blocks(
    rasters: &RealRasters,
    z: u8,
    bn: u32,
    tiles: &[(u32, u32)],
) -> Vec<PrepBlock> {
    let (mut ps, mut pn, mut pw, mut pe) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(tx, ty) in tiles {
        let bb = tile_bbox(z, tx, ty);
        ps = ps.min(bb.south_lat);
        pn = pn.max(bb.north_lat);
        pw = pw.min(bb.west_lon);
        pe = pe.max(bb.east_lon);
    }
    rasters.preload_dem_bbox(ps, pn, pw, pe);
    let mut batches: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(tx, ty) in tiles {
        batches
            .entry(((tx / bn) * bn, (ty / bn) * bn))
            .or_default()
            .push((tx, ty));
    }
    batches
        .into_iter()
        .map(|((bx, by), btiles)| PrepBlock {
            bx,
            by,
            batch: TileBatch::build_receiver_altitude_only(z, bx, by, bn, rasters),
            btiles,
        })
        .collect()
}

/// CPU prep stage for one cell (no GPU/device touch): load its grid_disk(1) airborne sources
/// through `cache`, `region_candidates` + pack the region SoA, then build the DEM-only tile
/// batches for `tiles` (the cell's owned tiles — `region_tiles(r4,z)` on the stream/production
/// path, a bbox/single-tile subset on the dev paths). The packed SoA is uploaded later by
/// `gpu_build_cell_one_pass` via `upload_region`. `region` is dropped right after packing — only the SoA
/// crosses the channel, so host RAM per buffered cell is the SoA + its tile blocks, not the
/// prepared-segment Vec too. (Takes no `&Args`: the output dir + write-empty flag this stage's
/// serial predecessor referenced live in the GPU build stage, now the only stage that writes tiles.)
pub(crate) fn prep_cell(
    rasters: &RealRasters,
    cache: &mut R4SourceCache,
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

    // Pre-fault the DEM footprint + batch the tiles into DEM-only blocks (shared with the M2 chunked
    // build — one source of truth for the block topology, see `build_dem_blocks`).
    let dem_start = Instant::now();
    let blocks = build_dem_blocks(rasters, z, bn, tiles);
    let dem = dem_start.elapsed();
    Ok(PreparedCell {
        sll,
        sf,
        si,
        nreg,
        blocks,
        t_start,
        timings: PrepTimings {
            candidates,
            pack,
            dem,
        },
        too_big: false,
    })
}
