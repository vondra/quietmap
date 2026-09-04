//! CPU prep for gpu-airborne: pack candidates and build receiver/terrain inputs.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use h3o::CellIndex;
use noise_compute::emission::aircraft::{SegmentPrepared, RECEIVER_HORIZON_MAX_M};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_gpu::airborne::region_candidates;
use noise_gpu::pack_airborne_segs;
use raster_reader::fused_grid::FusedGrid;
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBatch, TileBbox};
use raster_reader::RealRasters;
use rayon::prelude::*;
use tile_painter::grid::tile_bbox;
use tile_painter::r4_source_cache::R4SourceCache;
use tile_painter::region_runner::{group_tiles_into_batches, region_tiles};
use tile_painter::source_loader_structure::{InteriorEstimate, StructureData};

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
/// from [`cell_horizon_halo`], and its receiver lattice and interior stamp from the tile alone.
fn prepare_receiver_blocks(
    rasters: &RealRasters,
    z: u8,
    bn: u32,
    r4: u64,
    tiles: &[(u32, u32)],
    obstacles: &ObstacleSet,
) -> Result<Vec<PrepBlock>> {
    let region_halo = cell_horizon_halo(rasters, z, r4)?;
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
                    Arc::clone(&region_halo),
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

/// The elevation halo every block of one cell shares: the bbox of the tiles the CELL owns plus
/// the receiver horizon reach. It takes the cell, never the tiles a single request paints, and
/// that signature is the whole invariant: a `FusedGrid` reconstructs each sample's lat/lon from
/// its own origin, so a halo narrowed to a subset of the cell would move every DEM sample by an
/// ULP, and a request that paints part of a cell could no longer be compared byte for byte
/// against the whole-cell paint. That is what a bounded release check needs, and it also makes
/// the dev `--bbox` / `--tile-x` paths read the terrain production reads.
fn cell_horizon_halo(rasters: &RealRasters, z: u8, r4: u64) -> Result<Arc<FusedGrid>> {
    let receivers = tiles_bbox(z, &region_tiles(r4, z))
        .with_context(|| format!("horizon halo of R4 {r4:015x}"))?;
    rasters.preload_dem_bbox(
        receivers.south_lat,
        receivers.north_lat,
        receivers.west_lon,
        receivers.east_lon,
    );
    Ok(FusedTileZ13::build_elevation_halo(
        &receivers,
        RECEIVER_HORIZON_MAX_M,
        rasters,
    ))
}

/// The bounding box of a tile set at `z`, as one un-wrapped Mercator rectangle.
///
/// Refuses the two tile sets that have no such rectangle, so neither reaches `FusedGrid` as a
/// silent monster:
///   * empty — a cell with no tile at this zoom (an R4 above Mercator's +-85 degree cut owns
///     none). Both callers already return before an empty tile list, so this is a named
///     impossibility rather than a live path.
///   * straddling the antimeridian — `region_tiles` scans every column for such a cell and
///     returns tiles at both seam strips (measured: R4 8422591ffffffff owns 197 tiles spanning
///     the full 360 degrees), so min/max describes the whole globe. A grid that wide is ~31 GB
///     of `FusedPixel` and would abort the process; one named `fail` parks the cell instead and
///     leaves the worker its queue. Nothing regresses: this is what the whole-cell production
///     path has always handed `build_dem_blocks`. `region_tiles` carries the matching caveat.
fn tiles_bbox(z: u8, tiles: &[(u32, u32)]) -> Result<TileBbox> {
    let (mut south, mut north, mut west, mut east) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(tx, ty) in tiles {
        let bbox = tile_bbox(z, tx, ty);
        south = south.min(bbox.south_lat);
        north = north.max(bbox.north_lat);
        west = west.min(bbox.west_lon);
        east = east.max(bbox.east_lon);
    }
    if tiles.is_empty() {
        bail!("no tile at z{z} to bound");
    }
    if east - west > 180.0 {
        bail!(
            "tiles span {:.1} degrees of longitude: this cell straddles the antimeridian and has \
             no single Mercator bounding box",
            east - west
        );
    }
    Ok(TileBbox {
        west_lon: west,
        east_lon: east,
        north_lat: north,
        south_lat: south,
    })
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
mod tests {
    use super::*;

    /// Dobris and the 4x4 z13 window the release check paints inside it.
    const DOBRIS: u64 = 0x841e309ffffffff;
    const ZOOM: u8 = 13;
    const WINDOW: [(u32, u32); 4] = [(4414, 2786), (4415, 2786), (4414, 2787), (4415, 2787)];

    /// One bug class: a tile set with no single Mercator rectangle silently becomes a monster
    /// grid. Measured over the prepared world on 2026-09-04 (121 790 R4 cells under
    /// `<prepared>/2026/h3r4`): 65 cells' boundaries span more than 180 degrees of longitude, and
    /// five of those carry an `airborne.arrow` — so this is live work, not a hypothetical. Such a
    /// cell owns z13 tiles at BOTH seam strips, its bbox is the whole globe, and the halo would
    /// be ~31 GB of `FusedPixel`; a cell above Mercator's cut owns no tile at all and the min/max
    /// sentinels would survive into the grid. Both are named refusals — one `fail` parks the cell
    /// and leaves the worker its queue, where the whole-cell production path has always handed
    /// `build_dem_blocks` the same impossible box. A high-latitude cell that IS one rectangle
    /// must keep working.
    #[test]
    fn a_cell_without_one_mercator_rectangle_is_refused_by_name() {
        // All three are cells of the prepared world. 84bb005ffffffff (-41.4, 179.8) carries
        // airborne.arrow and owns 129 z13 tiles spanning the full 360 degrees; 8403205ffffffff
        // (89.5 N) is above the Mercator cut and owns none; 8401515ffffffff (Svalbard, 78 N)
        // owns 1527 tiles inside one 2.07-degree-wide rectangle.
        const ANTIMERIDIAN: u64 = 0x84bb005ffffffff;
        const ABOVE_MERCATOR: u64 = 0x8403205ffffffff;
        const HIGH_LATITUDE: u64 = 0x8401515ffffffff;

        let seam_tiles = region_tiles(ANTIMERIDIAN, ZOOM);
        assert_eq!(seam_tiles.len(), 129);
        let straddling =
            tiles_bbox(ZOOM, &seam_tiles).expect_err("an antimeridian cell has no single bbox");
        assert!(format!("{straddling:#}").contains("antimeridian"));

        let polar = tiles_bbox(ZOOM, &region_tiles(ABOVE_MERCATOR, ZOOM))
            .expect_err("a cell above the Mercator cut owns no tile");
        assert!(format!("{polar:#}").contains("no tile"));

        let arctic = tiles_bbox(ZOOM, &region_tiles(HIGH_LATITUDE, ZOOM))
            .expect("a high-latitude cell away from the seam is one rectangle");
        assert!(arctic.north_lat > 77.0 && arctic.east_lon - arctic.west_lon < 5.0);
    }

    /// One bug class, at the CALL SITE: a windowed request prepares different receivers than the
    /// whole-cell request it was carved out of. This runs the production block builder twice for
    /// one cell — once with every tile the cell owns, once with a window of them — and demands
    /// that each kept tile come back in the same block, at the same batch origin, over the same
    /// terrain halo, with a bit-identical receiver lattice and the same interior stamp. z11 keeps
    /// the whole-cell side to nine tiles; the code path is the one z13 production runs.
    #[test]
    fn a_windowed_request_prepares_the_same_receivers_as_the_whole_cell() {
        use tile_painter::stream_tile_window::TileWindow;

        const COARSE_ZOOM: u8 = 11;
        const BATCH_N: u32 = 3;
        let rasters = RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"));
        let obstacles = ObstacleSet::empty();
        let owned = region_tiles(DOBRIS, COARSE_ZOOM);
        assert_eq!(owned.len(), 9, "the fixture cell must own a small tile set");
        let window = TileWindow {
            x: owned.iter().map(|tile| tile.0).min().unwrap(),
            y: owned.iter().map(|tile| tile.1).min().unwrap(),
            side: 2,
        };
        let windowed = window.select(owned.clone()).unwrap();
        assert!(windowed.len() < owned.len() && !windowed.is_empty());

        let whole_blocks =
            prepare_receiver_blocks(&rasters, COARSE_ZOOM, BATCH_N, DOBRIS, &owned, &obstacles)
                .unwrap();
        let windowed_blocks = prepare_receiver_blocks(
            &rasters,
            COARSE_ZOOM,
            BATCH_N,
            DOBRIS,
            &windowed,
            &obstacles,
        )
        .unwrap();
        assert!(windowed_blocks.len() < whole_blocks.len());

        let stamped = |interior: &InteriorEstimate| {
            use raster_reader::fused_tile_z13::TILE_PX;
            let mut cells = vec![200u8; TILE_PX * TILE_PX];
            interior.apply(&mut cells);
            cells
        };
        let find = |blocks: &[PrepBlock], tile: (u32, u32)| {
            let block = blocks
                .iter()
                .find(|block| block.btiles.contains(&tile))
                .expect("every requested tile is prepared exactly once");
            let slot = block.btiles.iter().position(|&t| t == tile).unwrap();
            let receivers = block.tile_refs()[slot];
            (
                (block.bx, block.by),
                (block.batch.base_x, block.batch.base_y),
                receivers.rx_lat,
                receivers.rx_lon,
                receivers.rx_alt_m.clone(),
                receivers.inner_elev_m.clone(),
                stamped(&block.interiors[slot]),
                block.batch.tiles[0].halo.packed_elevation_grid(),
            )
        };
        for &tile in &windowed {
            let whole = find(&whole_blocks, tile);
            let narrowed = find(&windowed_blocks, tile);
            assert_eq!(whole.0, narrowed.0, "tile {tile:?} changed block");
            assert_eq!(whole.1, narrowed.1, "tile {tile:?} changed batch origin");
            assert_eq!(
                whole.2, narrowed.2,
                "tile {tile:?} changed receiver latitudes"
            );
            assert_eq!(
                whole.3, narrowed.3,
                "tile {tile:?} changed receiver longitudes"
            );
            assert_eq!(
                whole.4, narrowed.4,
                "tile {tile:?} changed receiver altitudes"
            );
            assert_eq!(whole.5, narrowed.5, "tile {tile:?} changed terrain");
            assert_eq!(
                whole.6, narrowed.6,
                "tile {tile:?} changed its interior stamp"
            );
            let (whole_halo, narrowed_halo) = (&whole.7, &narrowed.7);
            assert_eq!(
                (
                    whole_halo.lat_min,
                    whole_halo.lon_min,
                    whole_halo.rows,
                    whole_halo.cols
                ),
                (
                    narrowed_halo.lat_min,
                    narrowed_halo.lon_min,
                    narrowed_halo.rows,
                    narrowed_halo.cols
                ),
                "tile {tile:?} marched its horizon over a different halo lattice"
            );
        }
        // Every tile the window dropped is still prepared by the whole-cell request, so the
        // narrowing removed receivers and nothing else.
        for tile in owned.iter().filter(|tile| !windowed.contains(tile)) {
            assert!(whole_blocks.iter().any(|block| block.btiles.contains(tile)));
        }
    }

    /// One bug class: the painted tile set leaks into ADMISSION. `region_candidates` builds its
    /// admit envelope from the CELL, so a windowed paint must still admit every source the whole
    /// cell admits — including one that sits outside the painted window entirely. If the envelope
    /// were ever re-derived from the tiles a request paints, this flight would vanish from a
    /// windowed cell and its tiles would go quiet against the whole-cell reference.
    #[test]
    fn admission_comes_from_the_cell_and_never_from_the_painted_window() {
        use noise_compute::compute::aircraft_v6::views::{BBox, SubSegmentSlice};
        use noise_compute::compute::aircraft_v6::AirborneRowView;
        use noise_gpu::airborne::region_candidates;

        // The window's own bbox, and a flight one tile-width north-east of its corner — inside
        // the cell and inside the admit reach, outside everything the window paints.
        let window = tiles_bbox(ZOOM, &WINDOW).unwrap();
        let far_lat = (window.north_lat + 0.05) as f32;
        let far_lon = (window.east_lon + 0.06) as f32;
        assert!(
            f64::from(far_lat) > window.north_lat && f64::from(far_lon) > window.east_lon,
            "the fixture flight must lie outside the painted window"
        );
        let columns: [Vec<f32>; 6] = [
            vec![far_lat],
            vec![far_lon],
            vec![900.0],
            vec![far_lat + 0.01],
            vec![far_lon + 0.01],
            vec![900.0],
        ];
        let speed = vec![220.0f32];
        let length = vec![1_500.0f32];
        let period = vec![0u8];
        let date = vec![10i16];
        let flags = vec![1u8];
        let terrain = vec![300.0f32];
        let views = vec![AirborneRowView {
            flight_id: noise_compute::flight_id::pack_synth(1),
            callsign: "TEST",
            aircraft_type: *b"A320",
            profile_idx: 0,
            source_id: 0,
            origin: 0,
            sub_segments: SubSegmentSlice {
                start_lat: &columns[0],
                start_lon: &columns[1],
                start_alt_m: &columns[2],
                end_lat: &columns[3],
                end_lon: &columns[4],
                end_alt_m: &columns[5],
                speed_kt: &speed,
                length_m: &length,
                period: &period,
                date_id: &date,
                flags: &flags,
                terrain_start_elev_m: &terrain,
                terrain_end_elev_m: &terrain,
            },
            bbox: BBox {
                min_lat: far_lat,
                max_lat: far_lat + 0.01,
                min_lon: far_lon,
                max_lon: far_lon + 0.01,
            },
        }];
        assert_eq!(
            region_candidates(&views, DOBRIS, ZOOM).len(),
            1,
            "the cell must admit a source outside the window it happens to paint"
        );
    }

    /// One bug class: a paint narrowed by a `tiles=` window differs from the whole-cell paint.
    /// Every block of a cell marches its receiver horizons over ONE shared elevation halo. A halo
    /// anchored to the tiles a request paints would start on a different lattice origin, and a
    /// `FusedGrid` reconstructs each sample's lat/lon from its own origin — so every sample of a
    /// windowed paint would move by an ULP and its tiles could no longer be compared, byte for
    /// byte, against the whole-cell reference. Pin that the cell's halo is strictly the larger
    /// grid and that a window-anchored one really would sit somewhere else.
    #[test]
    fn the_shared_horizon_halo_spans_the_cell_not_the_painted_window() {
        // No raster tree: elevations read as sea level, which leaves the grid GEOMETRY —
        // the whole subject of this test — exactly as production builds it.
        let rasters = RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"));
        let owned = region_tiles(DOBRIS, ZOOM);
        assert!(WINDOW.iter().all(|tile| owned.contains(tile)));

        let cell = cell_horizon_halo(&rasters, ZOOM, DOBRIS)
            .unwrap()
            .packed_elevation_grid();
        let window_anchored = FusedTileZ13::build_elevation_halo(
            &tiles_bbox(ZOOM, &WINDOW).unwrap(),
            RECEIVER_HORIZON_MAX_M,
            &rasters,
        )
        .packed_elevation_grid();

        assert!(
            cell.rows > window_anchored.rows && cell.cols > window_anchored.cols,
            "the cell's halo must be the larger grid: {}x{} against {}x{}",
            cell.rows,
            cell.cols,
            window_anchored.rows,
            window_anchored.cols
        );
        assert!(
            cell.lat_min <= window_anchored.lat_min && cell.lon_min <= window_anchored.lon_min,
            "the cell's halo must start south and west of any window inside it"
        );
        assert_ne!(
            (cell.lat_min, cell.lon_min),
            (window_anchored.lat_min, window_anchored.lon_min),
            "a window-anchored halo starts on a different lattice origin, which is exactly \
             the difference that would move every sample of a windowed paint"
        );
        assert_eq!(cell.inv_cell_deg, window_anchored.inv_cell_deg);
    }
}
