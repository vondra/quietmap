//! Region-by-region tile builder — the world-scale outer loop. One
//! region = one output R4: it loads its `grid_disk(1)` sources through
//! the LRU, merges their views, and runs the existing batch / kernel /
//! HM3 pipeline over the region's own base-zoom tiles. Memory is bounded to
//! one region's ring + one `TileBatch` at a time, so the whole globe
//! runs region after region. Sequential in v1 (M5 adds outer rayon with
//! per-thread caches).
//!
//! Equivalence to the legacy whole-bbox build: a tile is assigned to the
//! region of its CENTRE R4 (matching the old per-tile centre-R4 source
//! pick), and the kernels sum commutatively over the concatenated
//! per-R4 views, so both paths give a tile the identical source set.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use h3o::{CellIndex, LatLng, Resolution};
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::RealRasters;

use crate::accumulator::TileAccumulator;
use crate::airborne::{scatter_tile as airborne_scatter_tile, AirborneStats};
use crate::cruise::ScatterStats as CruiseStats;
use crate::cruise_field::CruiseField;
use crate::grid::{tile_bbox, tile_range};
use crate::r4_source_cache::{R4SourceCache, SourceSel};
use crate::wire_hm3;

/// Immutable per-build settings shared across every region.
pub struct RegionCtx<'a> {
    pub zoom: u8,
    pub sel: SourceSel,
    pub n_days: u16,
    /// GA full-year hybrid per-class weight LUT, resolved once build-wide
    /// from the source arrows' `sample_days_by_class` metadata
    /// (`worklist::resolve_class_weights`). Threaded into the airborne
    /// scatter.
    pub class_weights: noise_compute::emission::aircraft::ClassWeights,
    pub batch_n: u32,
    pub output: &'a Path,
    pub h3r4_dir: &'a Path,
    pub write_empty: bool,
    pub rasters: &'a RealRasters,
}

#[derive(Default)]
pub struct RegionStats {
    pub tiles_written: usize,
    pub tiles_skipped: usize,
    pub bytes_written: usize,
    /// Airborne scatter telemetry, summed over every tile (near/coarse/prune
    /// split). Zero unless `sel.airborne`. Logged once at build end.
    pub airborne: AirborneStats,
    /// Cruise scatter telemetry, summed over every tile. Zero unless
    /// `sel.cruise`; logged once at build end.
    pub cruise: CruiseStats,
    /// Wall-time per phase, summed. Across regions these sum (CPU-like, since
    /// regions run on outer rayon); within one region they are that region's
    /// serial wall. `t_load` = grid_disk(1) source load + view build, `t_raster`
    /// = `TileBatch::build` DEM/raster fill, `t_scatter` = all `scatter_tile`,
    /// `t_write` = HM3 collapse + write. Lets us see where the build's time goes.
    pub t_load: Duration,
    pub t_raster: Duration,
    pub t_scatter: Duration,
    pub t_cruise_scatter: Duration,
    pub t_airborne_scatter: Duration,
    pub t_write: Duration,
}

/// North-west tile of the `batch_n × batch_n` [`TileBatch`] that paints a
/// grid-aligned block. Normally the block's own grid origin; at the east /
/// south world edge (a zoom width not divisible by `batch_n`, e.g. the L3
/// default 3 against 4096 columns) the origin slides back so no tile at or
/// beyond `2^zoom` is ever built. Every owned tile of the block stays inside
/// the batch; only world-edge blocks see a shifted shared halo lattice.
pub fn block_batch_origin(block_x: u32, block_y: u32, batch_n: u32, zoom: u8) -> (u32, u32) {
    let limit = 1_u32 << zoom;
    assert!(
        batch_n >= 1 && batch_n <= limit,
        "batch {batch_n} exceeds the z{zoom} world"
    );
    (block_x.min(limit - batch_n), block_y.min(limit - batch_n))
}

/// Locate a tile in a contiguous [`TileBatch`] without duplicating its
/// row-major indexing arithmetic in the CPU, GPU, and aircraft writers.
pub fn batch_slot(batch: &TileBatch, x: u32, y: u32) -> usize {
    assert!(x >= batch.base_x && x < batch.base_x + batch.batch_n);
    assert!(y >= batch.base_y && y < batch.base_y + batch.batch_n);
    ((y - batch.base_y) * batch.batch_n + (x - batch.base_x)) as usize
}

impl RegionStats {
    pub fn merge(&mut self, o: RegionStats) {
        self.tiles_written += o.tiles_written;
        self.tiles_skipped += o.tiles_skipped;
        self.bytes_written += o.bytes_written;
        self.airborne.merge(&o.airborne);
        self.cruise.merge(&o.cruise);
        self.t_load += o.t_load;
        self.t_raster += o.t_raster;
        self.t_scatter += o.t_scatter;
        self.t_cruise_scatter += o.t_cruise_scatter;
        self.t_airborne_scatter += o.t_airborne_scatter;
        self.t_write += o.t_write;
    }
}

/// R4 hex of a tile's centre. `None` for `x`/`y` at or beyond `2^zoom` —
/// not a tile, even though its centre still computes to a finite lat/lon
/// (the dev `--tile-x/--tile-y` and positional-block paths can ask) — or
/// when H3 rejects the centre.
pub fn tile_centre_r4(zoom: u8, x: u32, y: u32) -> Option<u64> {
    let limit = 1_u32 << zoom;
    if x >= limit || y >= limit {
        return None;
    }
    let b = tile_bbox(zoom, x, y);
    let lat = (b.north_lat + b.south_lat) * 0.5;
    let lon = (b.east_lon + b.west_lon) * 0.5;
    LatLng::new(lat, lon)
        .ok()
        .map(|ll| u64::from(ll.to_cell(Resolution::Four)))
}

/// Order regions on a Morton (Z-order) curve over their centre lat/lng,
/// so spatially-near regions stay temporally near in BOTH axes and a
/// region's grid_disk(1) ring is reused from the LRU. A latitude-band
/// boustrophedon only buys 1D locality: at global scale an R4 row is
/// hundreds of cells wide — far more than the cache holds — so it evicts
/// every N-S neighbour before the next row reaches it. The Z-curve bounds
/// jumps in both axes, keeping the working set inside the cap at any
/// scale, vs raw H3-index order (`CellIndex: Ord` = base-cell+digits)
/// which is geographically scattered — the worst case. Uses
/// `LatLng::from` only (robust at pentagons/poles); the single ±180° seam
/// touches only a handful of date-line R4s.
pub fn morton_order(r4s: &[u64]) -> Vec<u64> {
    let mut keyed: Vec<_> = r4s
        .iter()
        .map(|&r4| {
            let ll = LatLng::from(CellIndex::try_from(r4).expect("valid R4 cell"));
            let code = morton2(
                quantise(ll.lng(), -180.0, 180.0),
                quantise(ll.lat(), -90.0, 90.0),
            );
            (code, r4)
        })
        .collect();
    keyed.sort_by_key(|&(code, _)| code);
    keyed.into_iter().map(|(_, r4)| r4).collect()
}

/// Map `v ∈ [lo, hi]` onto a 21-bit lattice (≈10 m at the equator —
/// finer than an R4 cell, so distinct centres never collide).
fn quantise(v: f64, lo: f64, hi: f64) -> u32 {
    const MAX: u32 = (1 << 21) - 1;
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    (t * MAX as f64).round() as u32
}

/// Interleave the low 21 bits of `x` and `y` into a 42-bit Z-order code
/// (standard "Part1By1" bit-spread).
fn morton2(x: u32, y: u32) -> u64 {
    fn spread(v: u32) -> u64 {
        let mut x = v as u64;
        x = (x | (x << 16)) & 0x0000_ffff_0000_ffff;
        x = (x | (x << 8)) & 0x00ff_00ff_00ff_00ff;
        x = (x | (x << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
        x = (x | (x << 2)) & 0x3333_3333_3333_3333;
        x = (x | (x << 1)) & 0x5555_5555_5555_5555;
        x
    }
    spread(x) | (spread(y) << 1)
}

/// Every base-zoom tile whose CENTRE falls in R4 `r4` — the region's own
/// output tiles. The R4 boundary gives the candidate bbox; the
/// centre-R4 test drops the neighbours' tiles the bbox also covers.
/// (An antimeridian R4 over-scans x — still correct, the centre test
/// rejects the rest — left for a later wrap fix.)
pub fn region_tiles(r4: u64, zoom: u8) -> Vec<(u32, u32)> {
    let cell = CellIndex::try_from(r4).expect("valid R4 cell");
    let (mut s, mut n, mut w, mut e) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for ll in cell.boundary().iter() {
        s = s.min(ll.lat());
        n = n.max(ll.lat());
        w = w.min(ll.lng());
        e = e.max(ll.lng());
    }
    // tile_range takes (south, west, north, east). The lat range is
    // always valid, but when the region straddles ±180° its vertices sit
    // near both −180 and +180, so [w,e] is the inverted long way round
    // and tile_range would MISS the two real date-line strips. In that
    // case scan all columns and let the centre-R4 filter keep the
    // region's own tiles (rare — only date-line R4s).
    let (xr0, yr) = tile_range(zoom, s, w, n, e);
    let xr = if e - w > 180.0 {
        0..(1u32 << zoom)
    } else {
        xr0
    };
    let mut out = Vec::new();
    for y in yr {
        for x in xr.clone() {
            if tile_centre_r4(zoom, x, y) == Some(r4) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Split one `--stream` worker's stdin line into its R4 hex token and an OPTIONAL trailing
/// `layers=a,b,c` restriction. A rail-only data-version change must not force a road repaint.
/// The second token is deliberately optional — its
/// absence means "no restriction", i.e. build every layer this process was configured with,
/// TODAY'S behavior. The box agent sends the token ONLY for a STRICT SUBSET of a multi-layer
/// group (a full set stays bare hex), so single-layer engines (`gpu-airborne`,
/// `build_heatmap_aircraft`) never see it and keep their bare-hex readers. Both MULTI-layer
/// `--stream` readers (`build_heatmap_surface`, `gpu-surface`) call this SAME function so the
/// wire tokenizer has one definition, not two hand-rolled copies. The caller still owns
/// `u64::from_str_radix` on the hex token (and its own "skip non-hex line" logging) — this
/// only tokenizes.
pub fn split_stream_line(line: &str) -> (&str, Option<Vec<&str>>) {
    let mut it = line.split_whitespace();
    let hex = it.next().unwrap_or("");
    let layers = it
        .next()
        .and_then(|tok| tok.strip_prefix("layers="))
        .map(|csv| csv.split(',').filter(|s| !s.is_empty()).collect());
    (hex, layers)
}

/// Machine-readable lifecycle event shared by every warm CPU/GPU stream runner. Call this
/// immediately before the engine starts a cell; the existing `done`/`fail` event closes it.
/// The wall-clock timestamp lets an external supervisor preserve the start across samples,
/// while its own monotonic clock remains authoritative for watchdog decisions.
pub fn stream_cell_started_line(r4: u64, started_unix_ms: u128) -> String {
    format!("start {r4:x} {started_unix_ms}")
}

/// Publish a cell start before potentially hours-long work, flushing because stdout is a pipe.
pub fn announce_stream_cell_started(r4: u64) {
    use std::io::Write;

    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", stream_cell_started_line(r4, started_unix_ms));
    let _ = out.flush();
}

/// Narrow a `--stream` worker's CONFIGURED layers down to the subset a per-cell `layers=`
/// request named. `requested = None` (the token was absent) keeps
/// everything, unchanged from before this feature shipped. Returns `(effective, skipped)`:
/// `effective` is what to actually build for this cell; `skipped` is the configured layers'
/// NAMES that were excluded, for the `done` line's trailing `skipped=<layer,…>` token. Generic
/// over each binary's own concrete layer type (`Source` for build-heatmap-surface, `LineLayer`
/// for gpu-surface) via a `name` projection, so the one filtering RULE is shared instead of
/// re-implemented per binary — only the layer enum and its name differ.
pub fn split_configured_layers<L: Copy>(
    configured: &[L],
    requested: Option<&[String]>,
    name: impl Fn(L) -> &'static str,
) -> (Vec<L>, Vec<&'static str>) {
    match requested {
        None => (configured.to_vec(), Vec::new()),
        Some(names) => {
            let mut effective = Vec::with_capacity(configured.len());
            let mut skipped = Vec::new();
            for &l in configured {
                let n = name(l);
                if names.iter().any(|r| r == n) {
                    effective.push(l);
                } else {
                    skipped.push(n);
                }
            }
            (effective, skipped)
        }
    }
}

/// Read output-R4 cells (one 15-digit hex per line; blanks and `#` comments
/// skipped) from a regions file — the per-chunk work unit the cluster
/// orchestrator hands each worker (`--regions-file`). Both builders
/// turn each R4 into its `region_tiles`, so a chunk builds exactly its R4s'
/// tiles — disjoint from every other chunk by centre-R4 ownership.
pub fn read_r4_file(path: &Path) -> Result<Vec<u64>> {
    let txt = std::fs::read_to_string(path)
        .with_context(|| format!("read regions file {}", path.display()))?;
    let mut out = Vec::new();
    for line in txt.lines() {
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        let r4 = u64::from_str_radix(s.trim_start_matches("0x"), 16)
            .with_context(|| format!("bad R4 hex {s:?} in {}", path.display()))?;
        CellIndex::try_from(r4).with_context(|| format!("not a valid R4 cell: {s}"))?;
        out.push(r4);
    }
    Ok(out)
}

/// Build every tile of one output region. Loads the region's
/// `grid_disk(1)` sources through `cache` (the `Arc`s are held for the
/// region's lifetime so the merged views stay valid), then runs the
/// halo-sharing batch pipeline.
pub fn process_region(
    ctx: &RegionCtx,
    cache: &mut R4SourceCache,
    region_r4: u64,
    tiles: &[(u32, u32)],
) -> Result<RegionStats> {
    if tiles.is_empty() {
        return Ok(RegionStats::default());
    }
    let t0 = Instant::now();
    // grid_disk(1) = centre + 6 neighbours covers the ≤16 km reach of any
    // tile in this region (R4 ≈ 26 km radius, spacing ~42 km).
    let cell = CellIndex::try_from(region_r4).expect("valid R4 cell");
    let mut arcs = Vec::with_capacity(7);
    for nbr in cell.grid_disk::<Vec<_>>(1) {
        arcs.push(cache.get_or_load(u64::from(nbr))?);
    }
    let cruise_views: Vec<_> = arcs.iter().flat_map(|a| a.cruise.views()).collect();
    let airborne_views: Vec<_> = arcs.iter().flat_map(|a| a.airborne.views()).collect();
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let obstacle_data =
        crate::source_loader_obstacle::ObstacleData::load_for_r4s(ctx.h3r4_dir, region_r4, &ring)
            .with_context(|| format!("load obstacles R4 {region_r4:015x}"))?;

    let mut stats = RegionStats {
        t_load: t0.elapsed(),
        ..Default::default()
    };

    // Cruise coarse-field: computed lazily on a shared global z10 grid and
    // bilinearly upsampled into each base-zoom tile (seam-free; the per-bucket terrain
    // sample + prefilter run ~19× fewer times → ~5-6× faster cruise — see
    // `cruise_field`). Built once per region; z10 tiles are filled on first
    // bracket during the upsample below.
    let mut cruise_field = ctx
        .sel
        .cruise
        .then(|| CruiseField::new(ctx.rasters, &cruise_views));

    let t_pre = Instant::now();
    preload_region(ctx, tiles);
    stats.t_raster += t_pre.elapsed();

    let mut batches: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(x, y) in tiles {
        let bx = (x / ctx.batch_n) * ctx.batch_n;
        let by = (y / ctx.batch_n) * ctx.batch_n;
        batches.entry((bx, by)).or_default().push((x, y));
    }

    for ((bx, by), batch_tiles) in &batches {
        // Airborne/cruise are NPD and never ray-march the tile halo (cruise reads
        // the full raster store, airborne is pre-sampled at extract), so build with
        // a 0 halo — only the inner FusedTileZ13 (receiver lattice + rx_alt) is read.
        let (base_x, base_y) = block_batch_origin(*bx, *by, ctx.batch_n, ctx.zoom);
        let t_batch = Instant::now();
        let batch = TileBatch::build_receiver_altitude_only(
            ctx.zoom,
            base_x,
            base_y,
            ctx.batch_n,
            ctx.rasters,
        );
        stats.t_raster += t_batch.elapsed();

        for &(x, y) in batch_tiles {
            let tile = &batch.tiles[batch_slot(&batch, x, y)];
            // Building interiors (vector regions only): the same per-tile class
            // raster + façade donor map the surface painters bake for this tile.
            let t_class = Instant::now();
            let interior = obstacle_data.interior_estimate(tile);
            stats.t_raster += t_class.elapsed();

            let mut accum = TileAccumulator::new();
            if let Some(field) = cruise_field.as_mut() {
                let t_cruise = Instant::now();
                field.upsample_into(tile, &mut accum);
                let dt = t_cruise.elapsed();
                stats.t_cruise_scatter += dt;
                stats.t_scatter += dt;
            }
            if ctx.sel.airborne {
                let t_airborne = Instant::now();
                stats.airborne.merge(&airborne_scatter_tile(
                    tile,
                    &airborne_views,
                    &ctx.class_weights,
                    &mut accum,
                ));
                let dt = t_airborne.elapsed();
                stats.t_airborne_scatter += dt;
                stats.t_scatter += dt;
            }

            let t_write = Instant::now();
            let mut cells = wire_hm3::collapse_lden_u8(&accum, ctx.n_days as f64);
            interior.apply(&mut cells);
            let out_path = ctx
                .output
                .join(ctx.zoom.to_string())
                .join(x.to_string())
                .join(format!("{y}.bin"));
            let written = wire_hm3::write_tile(
                &out_path,
                &cells,
                wire_hm3::SOURCE_ID_AIRCRAFT,
                !ctx.write_empty,
            )?;
            if written == 0 {
                // Re-run shrank this tile to silence — unlink any stale prior tile
                // so an incremental recombine/pyramid can't read phantom energy
                // (mirrors the surface builder).
                if out_path.exists() {
                    std::fs::remove_file(&out_path)
                        .with_context(|| format!("rm stale {}", out_path.display()))?;
                }
                stats.tiles_skipped += 1;
            } else {
                stats.tiles_written += 1;
                stats.bytes_written += written;
            }
            // Collapse, interior estimate, Brotli encoding, filesystem write and
            // stale-output removal are one production boundary; record the
            // composite, including silence.
            stats.t_write += t_write.elapsed();
        }
    }
    // Cruise telemetry was accumulated over the z10 coarse-field tiles, not the
    // base-zoom outputs; fold it in once the region's field is fully realised.
    if let Some(field) = cruise_field {
        stats.cruise.merge(&field.stats);
    }
    Ok(stats)
}

fn preload_region(ctx: &RegionCtx, tiles: &[(u32, u32)]) {
    let (mut s, mut n, mut w, mut e) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(x, y) in tiles {
        let b = tile_bbox(ctx.zoom, x, y);
        s = s.min(b.south_lat);
        n = n.max(b.north_lat);
        w = w.min(b.west_lon);
        e = e.max(b.east_lon);
    }
    ctx.rasters.preload_dem_bbox(s, n, w, e);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn center(r4: u64) -> (f64, f64) {
        let ll = LatLng::from(CellIndex::try_from(r4).unwrap());
        (ll.lat(), ll.lng())
    }

    /// Mean degrees between consecutive regions — the locality the LRU
    /// rides on. Lower = neighbours stay temporally close.
    fn mean_step(order: &[u64]) -> f64 {
        order
            .windows(2)
            .map(|w| {
                let (la, na) = center(w[0]);
                let (lb, nb) = center(w[1]);
                ((la - lb).powi(2) + (na - nb).powi(2)).sqrt()
            })
            .sum::<f64>()
            / (order.len() - 1).max(1) as f64
    }

    #[test]
    fn morton_order_is_a_local_permutation() {
        // R4 cells across a wide multi-base-cell patch, so the raw u64
        // (H3-index) order has to jump between base cells.
        let mut set = BTreeSet::new();
        let mut lat = 25.0;
        while lat < 65.0 {
            let mut lon = -20.0;
            while lon < 50.0 {
                set.insert(u64::from(
                    LatLng::new(lat, lon).unwrap().to_cell(Resolution::Four),
                ));
                lon += 0.5;
            }
            lat += 0.5;
        }
        let raw: Vec<u64> = set.iter().copied().collect(); // BTreeSet == raw u64 order
        let mortoned = morton_order(&raw);

        // Permutation: same set, no loss/dup.
        assert_eq!(mortoned.len(), raw.len());
        assert_eq!(mortoned.iter().copied().collect::<BTreeSet<_>>(), set);

        // Morton keeps consecutive regions far closer than raw index order.
        let (m, r) = (mean_step(&mortoned), mean_step(&raw));
        assert!(m < r, "morton mean step {m:.3}° must beat raw {r:.3}°");
    }

    #[test]
    fn tile_centre_r4_rejects_tiles_outside_the_world() {
        // z3 = 8 tiles per axis: (7, 7) is the last valid tile, (8, 0) and
        // (0, 8) are beyond the world even though their centres are finite.
        assert!(tile_centre_r4(3, 7, 7).is_some());
        assert!(tile_centre_r4(3, 8, 0).is_none());
        assert!(tile_centre_r4(3, 0, 8).is_none());
    }

    #[test]
    fn block_batch_origin_slides_back_at_world_edges() {
        // z3 = 8 tiles per axis; batch 3 does not divide 8: the last grid block
        // starts at 6 and would build tiles 6..=8 — the origin slides to 5.
        assert_eq!(block_batch_origin(0, 0, 3, 3), (0, 0));
        assert_eq!(block_batch_origin(3, 3, 3, 3), (3, 3));
        assert_eq!(block_batch_origin(6, 6, 3, 3), (5, 5));
        assert_eq!(block_batch_origin(6, 0, 2, 3), (6, 0));
        assert_eq!(block_batch_origin(7, 7, 1, 3), (7, 7));
    }

    // Per-layer worker builds.

    #[test]
    fn split_stream_line_bare_hex_has_no_layers_request() {
        let (hex, layers) = split_stream_line("841e309ffffffff");
        assert_eq!(hex, "841e309ffffffff");
        assert!(
            layers.is_none(),
            "a bare hex line must not request a restriction"
        );
    }

    #[test]
    fn split_stream_line_parses_the_layers_token() {
        let (hex, layers) = split_stream_line("841e309ffffffff layers=road,rail");
        assert_eq!(hex, "841e309ffffffff");
        assert_eq!(layers, Some(vec!["road", "rail"]));
    }

    #[test]
    fn split_stream_line_tolerates_extra_whitespace_and_an_empty_csv_item() {
        let (hex, layers) = split_stream_line("  841e309ffffffff   layers=road,,rail  ");
        assert_eq!(hex, "841e309ffffffff");
        assert_eq!(layers, Some(vec!["road", "rail"]));
    }

    #[test]
    fn split_stream_line_ignores_an_unrecognised_second_token() {
        // Anything that isn't `layers=...` in the second slot is simply not a layers request —
        // the caller's own hex parse (not this function) is what rejects a malformed line.
        let (hex, layers) = split_stream_line("841e309ffffffff garbage");
        assert_eq!(hex, "841e309ffffffff");
        assert!(layers.is_none());
    }

    #[test]
    fn stream_cell_started_event_has_one_stable_machine_readable_shape() {
        assert_eq!(
            stream_cell_started_line(0x841e309ffffffff, 1_721_234_567_890),
            "start 841e309ffffffff 1721234567890"
        );
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum TestLayer {
        Road,
        Rail,
    }
    fn test_layer_name(l: TestLayer) -> &'static str {
        match l {
            TestLayer::Road => "road",
            TestLayer::Rail => "rail",
        }
    }

    #[test]
    fn split_configured_layers_none_keeps_everything_and_skips_nothing() {
        let configured = [TestLayer::Road, TestLayer::Rail];
        let (effective, skipped) = split_configured_layers(&configured, None, test_layer_name);
        assert_eq!(effective, vec![TestLayer::Road, TestLayer::Rail]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn split_configured_layers_narrows_to_the_requested_subset() {
        let configured = [TestLayer::Road, TestLayer::Rail];
        let requested = vec!["rail".to_string()];
        let (effective, skipped) =
            split_configured_layers(&configured, Some(&requested), test_layer_name);
        assert_eq!(effective, vec![TestLayer::Rail]);
        assert_eq!(skipped, vec!["road"]);
    }

    #[test]
    fn split_configured_layers_can_empty_out_the_effective_set() {
        // Every configured layer is stale=0 for this cell (e.g. a "rest" worker whose cell only
        // had "aircraft-ground" go stale) — the caller must skip the whole cell, not crash.
        let configured = [TestLayer::Road, TestLayer::Rail];
        let requested = vec!["industrial".to_string()]; // matches neither configured layer
        let (effective, skipped) =
            split_configured_layers(&configured, Some(&requested), test_layer_name);
        assert!(effective.is_empty());
        assert_eq!(skipped, vec!["road", "rail"]);
    }

    /// Ties `split_configured_layers` directly to the REAL production case named in the plan
    /// ("the road-repainted-for-a-rail-fix fix"): the `line` group's worker is configured with
    /// `[Source::Road, Source::Rail]`; a cell whose lease marks only `rail` stale must build
    /// ONLY rail, never road — using the actual `surface_region::Source` + `layer_meta`, not a
    /// synthetic stand-in.
    #[test]
    fn split_configured_layers_with_the_real_line_group_source_type() {
        use crate::surface_region::{layer_meta, Source};
        let configured = [Source::Road, Source::Rail];
        let requested = vec!["rail".to_string()];
        let (effective, skipped) =
            split_configured_layers(&configured, Some(&requested), |s| layer_meta(s).2);
        assert_eq!(effective, vec![Source::Rail]);
        assert_eq!(skipped, vec!["road"]);
    }
}
