//! Tile-pyramid downsample for HM3 layers in the tile store.
//!
//! Each pyramid level merges a `2×2` block of source cells into one
//! destination cell via **linear-energy mean**: convert the u8 × 0.5 dB
//! value back to linear energy, average across the 4 cells, convert
//! back to dB. A simple arithmetic mean of dB values would understate
//! the dominant-cell energy (a 60 dB and a 40 dB cell averaged
//! arithmetically gives 50, but the true energy mean is ~57 dB).
//!
//! Spatial layout: at zoom z each tile covers the same bbox as four
//! z+1 tiles at `(2x, 2y)`, `(2x+1, 2y)`, `(2x, 2y+1)`, `(2x+1, 2y+1)`.
//! A z-level tile's `tile_px × tile_px` pixels each come from a 2×2 block
//! of source pixels inside exactly one of those four z+1 tiles — no
//! cross-source-tile averaging needed.
//!
//! Silence handling: a cell carrying [`crate::wire_hm3::NO_DATA`]
//! contributes 0 energy and the divisor stays 4. This preserves the
//! area-mean interpretation Lden pyramids traditionally use — an
//! all-silent 2×2 block stays silent in the destination, so missing
//! child tiles propagate up cleanly.
//!
//! Store-to-store: level `z_dst` is written into the layer's
//! `z{dst}.qtsi/.qtsd` pair from the `z_src` pair via
//! [`crate::tile_store::TileStore::put_cells`] — the ship-out `BrotliHm3`
//! codec directly, since 2026-07-16 (was the `ZstdCells` working codec,
//! deferring the Brotli-q9 encode to the pmtiles pack; that made every
//! publish redo the encode for every pyramid tile, ~63 min for one 580k-tile
//! layer — see `TileCodec::ZstdCells`'s doc). Encoding once here, amortized
//! over the days between publishes, is what let `tile-store-pack`'s ship-out
//! drop back to a verbatim copy. The old mtime skip-gate + column-dir +
//! TOCTOU machinery is GONE: the store index is the presence truth, deletes
//! are tombstones, and cross-process write exclusivity is the store's OWN
//! per-(layer,zoom) flock (`tile_store::store::acquire_write_lock`), taken on
//! every writable open — the build/master flocks are just coarser outer locks
//! now, as refined after the 2026-07 storage redesign.
//!
//! Rebuild scope (2026-07-15): a bbox restricts WHICH z_dst tiles get
//! recomputed, but still recomputes every one of them at every level in
//! range — fine for a geographically tight scope, ruinous for a scattered
//! one (a world-spanning combine-loop.sh bucket forced full rewrites of
//! unrelated tiles dozens of times as unrelated cells inside it trickled
//! in — the store is append-only, so every such rewrite left the prior
//! version's bytes dead forever; measured live: rail's z11 level reached
//! 605 GB on disk for ~9-31 GB of actually-live data). `RebuildScope::Tiles`
//! instead threads an EXACT destination-tile list down through the cascade:
//! each level's `dst_vec` (already fully known before its parallel pass
//! runs) becomes the seed for the next level's tile list via `(x/2, y/2)`
//! — so a tile is only ever touched when one of its own children actually
//! changed, not because it shares a bucket with something that did.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::grid::tile_range;
use crate::tile_store::{StoreRebuildFence, TileStore};
use crate::wire_hm3::NO_DATA;

/// How `build_pyramid` selects which destination tiles to (re)roll at every level. Owned
/// throughout (no lifetime): `Tiles` doubles as both the caller-facing "starting list" and the
/// per-level cascade state — `build_pyramid` reassigns its own `scope` binding after each level
/// instead of threading a second, parallel "carry" variable that has to be kept in sync with it.
#[derive(Debug)]
pub enum RebuildScope {
    /// Every level store recreated from scratch (stale ancestors die
    /// wholesale) — the emergency/whole-layer-compaction path.
    Full,
    /// Same geographic bbox reapplied at every level — incremental, but
    /// still touches every tile the bbox covers whether or not it changed.
    Bbox([f64; 4]),
    /// Exact tile list. At `build_pyramid`'s own parameter, base-zoom
    /// coordinates (one level ABOVE the first destination); every level
    /// below cascades from the PARENTS of the tiles the level above
    /// actually rolled, not a fixed area — bounds write amplification to
    /// the pyramid's own branching factor (4×) regardless of how the dirty
    /// tiles are scattered geographically.
    Tiles(Vec<(u32, u32)>),
}

/// Stable, bounded identity for one rebuild scope. It is persisted in the layer transaction
/// fence, so a crashed incremental cascade can only be cleared by retrying the exact same work.
pub fn rebuild_scope_fingerprint(scope: &RebuildScope) -> String {
    match scope {
        RebuildScope::Full => "full".to_string(),
        RebuildScope::Bbox(bounds) => format!(
            "bbox-{:016x}-{:016x}-{:016x}-{:016x}",
            bounds[0].to_bits(),
            bounds[1].to_bits(),
            bounds[2].to_bits(),
            bounds[3].to_bits()
        ),
        RebuildScope::Tiles(tiles) => {
            let mut canonical = tiles.clone();
            canonical.sort_unstable();
            canonical.dedup();
            let mut hasher = Sha256::new();
            for &(x, y) in &canonical {
                hasher.update(x.to_le_bytes());
                hasher.update(y.to_le_bytes());
            }
            let mut digest = String::with_capacity(64);
            for byte in hasher.finalize() {
                write!(digest, "{byte:02x}").expect("write to String");
            }
            format!("tiles-{}-{digest}", canonical.len())
        }
    }
}

/// Resolves `--bbox`/`--tiles-from` CLI args into a `RebuildScope` — shared by both binaries
/// that accept either (`build-pyramid`, `build-heatmap-combine`), so the exclusivity check and
/// the cross-file union live in exactly one place. `tiles_from` is a slice, not a single
/// `Option`, because `build-heatmap-combine`'s `total/` tile at (x,y) can be dirty because ANY
/// of several layers changed there — its CLI makes `--tiles-from` repeatable (one file per
/// dirty layer) and this function unions them; `build-pyramid` (one layer at a time) just
/// passes a 0-or-1-element slice.
pub fn scope_from_args(bbox: Option<[f64; 4]>, tiles_from: &[PathBuf]) -> Result<RebuildScope> {
    if bbox.is_some() && !tiles_from.is_empty() {
        anyhow::bail!("--bbox and --tiles-from are mutually exclusive");
    }
    if tiles_from.is_empty() {
        return Ok(bbox.map_or(RebuildScope::Full, RebuildScope::Bbox));
    }
    let mut set = std::collections::BTreeSet::new();
    for p in tiles_from {
        set.extend(read_tile_list(p)?);
    }
    Ok(RebuildScope::Tiles(set.into_iter().collect()))
}

/// Parses a `--tiles-from` file: one `x y` pair per line (whitespace-
/// separated), blank lines skipped. Error-message idiom matches this crate's
/// other line-oriented list-file reader, `region_runner::read_r4_file`.
pub fn read_tile_list(path: &Path) -> Result<Vec<(u32, u32)>> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read tile list {}", path.display()))?;
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let x: u32 = it
                .next()
                .with_context(|| format!("tile list {}: empty line", path.display()))?
                .parse()
                .with_context(|| format!("tile list {}: bad x in {l:?}", path.display()))?;
            let y: u32 = it
                .next()
                .with_context(|| format!("tile list {}: line {l:?} missing y", path.display()))?
                .parse()
                .with_context(|| format!("tile list {}: bad y in {l:?}", path.display()))?;
            Ok((x, y))
        })
        .collect()
}

/// Unique `(x/2, y/2)` parents of `tiles` — the destination-tile set one
/// pyramid level up. Used by `build_pyramid`'s cascade both to translate the
/// caller's base-zoom list into the first level's tile set and, after each
/// level, to derive the next level's from the one just rolled.
fn halve_dedup(tiles: impl IntoIterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
    let mut set = std::collections::BTreeSet::new();
    for (x, y) in tiles {
        set.insert((x / 2, y / 2));
    }
    set.into_iter().collect()
}

/// Build every pyramid level from `base_zoom` down to `dst_zoom` (e.g.
/// 12 → 2) inside one layer's store dir. The HM3 `source_id` of any newly
/// created level store is DERIVED from the source level's header — the layer
/// itself is the truth, no caller flag to get wrong. Returns tiles written.
///
/// See [`RebuildScope`] for what each variant restricts. `Full` also removes
/// any leftover level stores BELOW `dst_zoom` — the pmtiles packer publishes
/// every `z*.qtsi` it finds, so a stale deeper history (e.g. an old z3-z5)
/// must not survive a full rebuild to z6.
pub fn build_pyramid(
    layer_dir: &Path,
    base_zoom: u8,
    dst_zoom: u8,
    scope: RebuildScope,
) -> Result<usize> {
    build_pyramid_managing_fence(layer_dir, base_zoom, dst_zoom, scope, &mut |_| Ok(()))
}

/// Build a pyramid inside an outer operation that fenced the layer before replacing its base
/// level. The outer owner remains responsible for removing the fence only after its own
/// post-pyramid checks and directory fsyncs complete.
pub fn build_pyramid_with_existing_rebuild_fence(
    layer_dir: &Path,
    base_zoom: u8,
    dst_zoom: u8,
    scope: RebuildScope,
) -> Result<usize> {
    validate_zoom_range(base_zoom, dst_zoom)?;
    if !matches!(scope, RebuildScope::Full) {
        require_incremental_pyramid_levels(layer_dir, base_zoom, dst_zoom)?;
    }
    StoreRebuildFence::require_present(layer_dir)?;
    build_pyramid_levels(layer_dir, base_zoom, dst_zoom, scope, &mut |_| Ok(()))
}

fn validate_zoom_range(base_zoom: u8, dst_zoom: u8) -> Result<()> {
    // Equality is a legitimate NO-OP, not an error: a zoom-tier root builds
    // its total at the tier's own zoom and has no pyramid below it
    // (city-z13 plan §A — combine runs with --dst-zoom == the tier zoom).
    // build_pyramid_levels over an empty level range writes nothing.
    if base_zoom < dst_zoom {
        anyhow::bail!("base_zoom {} must be >= dst_zoom {}", base_zoom, dst_zoom);
    }
    Ok(())
}

/// An incremental cascade is an update, never a bootstrap: every source and destination level
/// must already exist before the first ancestor is touched. A caller that needs first-time stores
/// must run the explicit full rebuild path.
pub fn require_incremental_pyramid_levels(
    layer_dir: &Path,
    base_zoom: u8,
    dst_zoom: u8,
) -> Result<()> {
    validate_zoom_range(base_zoom, dst_zoom)?;
    for zoom in dst_zoom..=base_zoom {
        for suffix in ["qtsi", "qtsd"] {
            let path = layer_dir.join(format!("z{zoom}.{suffix}"));
            if !path.try_exists()? {
                anyhow::bail!(
                    "incremental pyramid requires existing z{zoom}.{suffix} at {}; run one full pyramid bootstrap before scoped updates",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn build_pyramid_managing_fence(
    layer_dir: &Path,
    base_zoom: u8,
    dst_zoom: u8,
    scope: RebuildScope,
    after_level: &mut dyn FnMut(u8) -> Result<()>,
) -> Result<usize> {
    validate_zoom_range(base_zoom, dst_zoom)?;
    // The base level must exist even when the level range is EMPTY
    // (dst == base, the zoom-tier no-op): the per-level "requires source"
    // check never runs then, and a silent success over a missing store would
    // hide a broken tier root.
    let base_path = layer_dir.join(format!("z{base_zoom}.qtsi"));
    if !base_path.exists() {
        anyhow::bail!(
            "pyramid requires base z{base_zoom}, but {} is missing",
            base_path.display()
        );
    }
    if !matches!(scope, RebuildScope::Full) {
        require_incremental_pyramid_levels(layer_dir, base_zoom, dst_zoom)?;
    }
    let operation = format!("pyramid:{}", rebuild_scope_fingerprint(&scope));
    let fence = StoreRebuildFence::begin(layer_dir, &operation)?;
    let written = build_pyramid_levels(layer_dir, base_zoom, dst_zoom, scope, after_level)?;
    fence.finish()?;
    Ok(written)
}

fn build_pyramid_levels(
    layer_dir: &Path,
    base_zoom: u8,
    dst_zoom: u8,
    mut scope: RebuildScope,
    after_level: &mut dyn FnMut(u8) -> Result<()>,
) -> Result<usize> {
    // The caller's Tiles list is at base_zoom (z_src of the first fold), one level ABOVE the
    // first z_dst — halve it once up front so `scope` uniformly holds "this level's own tile
    // set" from the first iteration on, same as every iteration after it.
    if let RebuildScope::Tiles(t) = &scope {
        scope = RebuildScope::Tiles(halve_dedup(t.iter().copied()));
    }
    let mut total_written = 0;
    let mut total_corrupt = 0;
    for z_dst in (dst_zoom..base_zoom).rev() {
        let z_src = z_dst + 1;
        if matches!(&scope, RebuildScope::Tiles(l) if l.is_empty()) {
            break; // Tiles mode, nothing left to cascade — the walk is naturally exhausted
        }
        let (written, empty, corrupt, dst_vec) = build_one_level(layer_dir, z_src, z_dst, &scope)?;
        after_level(z_dst)?;
        eprintln!("  z={z_dst} ← z={z_src}: {written} written, {empty} empty");
        total_written += written;
        total_corrupt += corrupt;
        if matches!(scope, RebuildScope::Tiles(_)) {
            scope = RebuildScope::Tiles(halve_dedup(dst_vec));
        }
    }
    // The ONE line that reports corruption — deliberately the only place this
    // pass says "unreadable" so a caller (combine-loop.sh's step()) can grep
    // for this exact line and get the true cross-level total, not just
    // whichever level's per-level line it happened to match first.
    if total_corrupt > 0 {
        eprintln!(
            "pyramid: {total_corrupt} source tile(s) were unreadable and skipped this pass \
             — they self-heal once their owning cell is re-ingested; re-run pyramid after \
             the current build catches up to confirm 0"
        );
    }
    if matches!(scope, RebuildScope::Full) {
        for z in 0..dst_zoom {
            for suffix in ["qtsi", "qtsd"] {
                let p = layer_dir.join(format!("z{z}.{suffix}"));
                if p.exists() {
                    std::fs::remove_file(&p)?;
                    eprintln!("  removed stale below-dst level file {}", p.display());
                }
            }
        }
    }
    Ok(total_written)
}

/// (written, empty, corrupt, dst_vec) — `dst_vec` is the exact destination-
/// tile list this level attempted (already fully known before the parallel
/// pass below), handed back so `build_pyramid`'s Tiles-mode cascade can
/// derive the next level's list from it at zero extra cost.
type LevelResult = (usize, usize, usize, Vec<(u32, u32)>);

fn build_one_level(
    layer_dir: &Path,
    z_src: u8,
    z_dst: u8,
    scope: &RebuildScope,
) -> Result<LevelResult> {
    if !layer_dir.join(format!("z{z_src}.qtsi")).exists() {
        anyhow::bail!(
            "{} pyramid requires source z{z_src}, but {} is missing",
            if matches!(scope, RebuildScope::Full) {
                "full"
            } else {
                "incremental"
            },
            layer_dir.join(format!("z{z_src}.qtsi")).display()
        );
    }
    let src = TileStore::open(layer_dir, z_src, false)?;
    let source_id = src.source_id(); // the layer's own header is the truth
    let px = usize::from(src.tile_px()); // size-agnostic: 256 pre-shift, 512 post

    // Destination tiles to (re)build. Full: every parent of a present source
    // tile (a scan); Bbox/Tiles: exactly the z_dst tiles covered/listed.
    let dst_vec: Vec<(u32, u32)> = match scope {
        RebuildScope::Bbox([s, w, n, e]) => {
            let (xr, yr) = tile_range(z_dst, *s, *w, *n, *e);
            yr.flat_map(|y| xr.clone().map(move |x| (x, y))).collect()
        }
        RebuildScope::Tiles(list) => list.clone(),
        RebuildScope::Full => {
            let mut set = std::collections::BTreeSet::new();
            src.for_each_present(|x, y, _| {
                set.insert((x / 2, y / 2));
                Ok(())
            })?;
            set.into_iter().collect()
        }
    };
    // Full recreates the destination store from scratch (stale ancestors die wholesale);
    // Bbox/Tiles may only overwrite a preflighted existing level.
    let dst = if matches!(scope, RebuildScope::Full) {
        TileStore::create(layer_dir, z_dst, source_id, src.tile_px())?
    } else {
        TileStore::open(layer_dir, z_dst, true)?
    };
    if dst.source_id() != source_id {
        anyhow::bail!(
            "z{z_dst}: destination source_id {} differs from source z{z_src} source_id {source_id}",
            dst.source_id()
        );
    }
    if dst.tile_px() != src.tile_px() {
        anyhow::bail!(
            "z{z_dst}: destination tile_px {} differs from source z{z_src} tile_px {}",
            dst.tile_px(),
            src.tile_px()
        );
    }

    // Each destination tile reads its own 2×2 children and writes its own
    // disjoint store slot — embarrassingly parallel (atomic tail reservation,
    // disjoint index entries), exactly like the combine's base-level sum.
    let written = AtomicUsize::new(0);
    let empty = AtomicUsize::new(0);
    let corrupt = AtomicUsize::new(0);
    dst_vec.par_iter().try_for_each_init(
        // Per-worker buffer: the combine loop reruns this forever, so the
        // 256 KiB scratch must not be re-malloc'd per tile. The per-tile
        // fill(NO_DATA) stays — downsample only writes cells with energy.
        || vec![NO_DATA; px * px],
        |dst_cells, &(dx, dy)| -> Result<()> {
            dst_cells.fill(NO_DATA);
            let mut any_data = false;
            for (qx, qy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
                let src_cells = get_cells_lenient(&src, dx * 2 + qx, dy * 2 + qy, &corrupt)?;
                if let Some(src_cells) = src_cells {
                    downsample_2x2_into_quadrant(
                        &src_cells,
                        dst_cells,
                        qx as usize,
                        qy as usize,
                        px,
                    );
                    any_data = true;
                }
            }
            // No surviving child → tombstone any stale ancestor; an all-silent
            // pool tombstones inside put_cells (present ⟺ audible invariant).
            let n = if any_data {
                dst.put_cells(dx, dy, dst_cells)?
            } else {
                dst.delete(dx, dy)?;
                0
            };
            if n > 0 {
                written.fetch_add(1, Ordering::Relaxed);
            } else {
                empty.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        },
    )?;
    dst.sync_all()?;
    Ok((
        written.load(Ordering::Relaxed),
        empty.load(Ordering::Relaxed),
        corrupt.load(Ordering::Relaxed),
        dst_vec,
    ))
}

/// Read cells while treating a decode failure after a successful fetch as a
/// self-healing absent tile. Fetch/index I/O remains fatal, and mandatory fsck
/// blocks a corrupt store from publication. Shared by pyramid and combine,
/// never by the packer's ship-out path.
///
/// Deliberately does not log per tile: this runs inside a `par_iter` hot loop,
/// and under mass corruption one `eprintln!` per bad tile would serialize
/// every worker on stderr's lock. `corrupt` tallies hits for one aggregate
/// summary line after the parallel pass instead.
pub fn get_cells_lenient(
    store: &TileStore,
    x: u32,
    y: u32,
    corrupt: &AtomicUsize,
) -> Result<Option<Vec<u8>>> {
    let Some((codec, blob)) = store.get_blob(x, y)? else {
        return Ok(None);
    };
    match store.decode_cells(codec, &blob) {
        Ok(cells) => Ok(Some(cells)),
        Err(_) => {
            corrupt.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }
}

/// 2×2 → 1 pool of `src` placed into the (qx, qy) quadrant of `dst`.
/// Quadrants are arranged top-left, top-right, bottom-left, bottom-right
/// matching Mercator XYZ tile coordinates (Y axis grows downward). `px` is the
/// tile side (from the store header) — the arithmetic is size-agnostic; for
/// px=256 the operation order is byte-identical to the pre-shift code.
fn downsample_2x2_into_quadrant(src: &[u8], dst: &mut [u8], qx: usize, qy: usize, px: usize) {
    debug_assert_eq!(src.len(), px * px);
    debug_assert_eq!(dst.len(), px * px);
    let half = px / 2;
    for dy in 0..half {
        for dx in 0..half {
            let dst_px = qx * half + dx;
            let dst_py = qy * half + dy;
            let dst_idx = dst_py * px + dst_px;
            // The 2×2 source block at (2dx, 2dy) within src tile.
            let src_px0 = dx * 2;
            let src_py0 = dy * 2;
            let mut e_sum = 0.0f64;
            for dyi in 0..2 {
                for dxi in 0..2 {
                    let idx = (src_py0 + dyi) * px + (src_px0 + dxi);
                    let v = src[idx];
                    if v != NO_DATA {
                        // u8 × 0.5 dB encoding: dB = v / 2. The exp(ln10·…)
                        // form is kept VERBATIM (≡ 10^(db/10)) — a stylistic
                        // powf rewrite could flip the last ULP and with it a
                        // quantised byte at a rounding boundary (byte parity).
                        let db = v as f64 * 0.5;
                        e_sum += (db / 10.0 * std::f64::consts::LN_10).exp();
                    }
                }
            }
            // Area-mean: silence cells contribute 0 energy, divisor is 4
            // (block size). Dividing by `n_valid` instead would let
            // isolated loud cells stay loud at every zoom out ("pixel
            // bloat") and overstate energy density at flight-envelope
            // edges. An all-silent block keeps NO_DATA (initial fill).
            if e_sum > 0.0 {
                let db = 10.0 * (e_sum / 4.0).log10();
                let q = (db * 2.0).round();
                dst[dst_idx] = if q >= 254.0 {
                    254
                } else if q <= 0.0 {
                    0
                } else {
                    q as u8
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_legacy_zstd_cells(store: &TileStore, x: u32, y: u32, cells: &[u8]) {
        let blob = zstd::encode_all(std::io::Cursor::new(cells), 1).unwrap();
        store.put_blob(x, y, TileCodec::ZstdCells, &blob).unwrap();
    }
    use crate::grid::TILE_PX;
    use crate::tile_store::{TileCodec, REBUILD_INCOMPLETE_MARKER};
    use crate::wire_hm3::{quantise_lden, SOURCE_ID_AIRCRAFT, SOURCE_ID_RAIL, SOURCE_ID_ROAD};

    /// scope_from_args: the exclusivity check, the three no-file/no-bbox
    /// shapes, and — the reason it takes a slice at all — that repeated
    /// `--tiles-from` files union and dedupe rather than only using the
    /// first/last one.
    #[test]
    fn scope_from_args_rejects_both_bbox_and_tiles_from() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.tiles");
        std::fs::write(&f, "1 2\n").unwrap();
        assert!(scope_from_args(Some([0.0, 0.0, 1.0, 1.0]), &[f]).is_err());
    }

    #[test]
    fn scope_from_args_neither_given_is_full() {
        assert!(matches!(
            scope_from_args(None, &[]).unwrap(),
            RebuildScope::Full
        ));
    }

    #[test]
    fn scope_from_args_bbox_only_is_bbox() {
        let b = [1.0, 2.0, 3.0, 4.0];
        assert!(
            matches!(scope_from_args(Some(b), &[]).unwrap(), RebuildScope::Bbox(got) if got == b)
        );
    }

    #[test]
    fn scope_from_args_unions_and_dedupes_multiple_tiles_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.tiles");
        let b = dir.path().join("b.tiles");
        std::fs::write(&a, "1 2\n3 4\n").unwrap();
        std::fs::write(&b, "3 4\n5 6\n").unwrap(); // 3,4 shared with a.tiles
        let RebuildScope::Tiles(list) = scope_from_args(None, &[a, b]).unwrap() else {
            panic!("expected Tiles scope");
        };
        assert_eq!(
            list,
            vec![(1, 2), (3, 4), (5, 6)],
            "deduped union, BTreeSet order"
        );
    }

    /// `dst_zoom == base_zoom` is a legal NO-OP (a zoom-tier root has no
    /// pyramid below its own zoom, city-z13 plan §A): with the base present
    /// nothing is written and nothing errors; a missing base still fails via
    /// the source-level requirement, exactly like any Full rebuild.
    #[test]
    fn equal_base_and_dst_zoom_is_a_noop_with_base_present_and_fails_without() {
        let dir = tempfile::tempdir().unwrap();
        let base = TileStore::create(dir.path(), 13, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let cells = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
        base.put_cells(4424, 2774, &cells).unwrap();
        base.sync_all().unwrap();
        drop(base);
        let written = build_pyramid(dir.path(), 13, 13, RebuildScope::Full).unwrap();
        assert_eq!(written, 0, "no levels below the tier zoom to build");

        let missing = tempfile::tempdir().unwrap();
        assert!(
            build_pyramid(missing.path(), 13, 13, RebuildScope::Full).is_err(),
            "a missing base store must still fail loudly at equality"
        );
    }

    #[test]
    fn full_rebuild_fails_when_a_required_source_level_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let error = build_pyramid(dir.path(), 6, 2, RebuildScope::Full).unwrap_err();
        assert!(
            error.to_string().contains("requires base z6"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn injected_full_rebuild_failure_leaves_fence_until_complete_retry() {
        let dir = tempfile::tempdir().unwrap();
        let base = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let cells = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
        base.put_cells(2, 2, &cells).unwrap();
        base.sync_all().unwrap();
        drop(base);

        let mut completed_levels = 0;
        let error = build_pyramid_managing_fence(dir.path(), 6, 2, RebuildScope::Full, &mut |_| {
            completed_levels += 1;
            anyhow::bail!("injected failure after first durable level")
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected failure"));
        assert_eq!(completed_levels, 1);
        assert!(dir.path().join(REBUILD_INCOMPLETE_MARKER).exists());
        assert!(
            dir.path().join("z5.qtsi").exists(),
            "the failure was post-mutation"
        );

        build_pyramid(dir.path(), 6, 2, RebuildScope::Full).unwrap();
        assert!(!dir.path().join(REBUILD_INCOMPLETE_MARKER).exists());
    }

    #[test]
    fn incremental_rebuild_rejects_a_destination_source_id_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        TileStore::create(dir.path(), 5, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();

        let error = build_pyramid(dir.path(), 6, 5, RebuildScope::Tiles(vec![(2, 2)])).unwrap_err();
        assert!(
            error.to_string().contains("destination source_id"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn incremental_rebuild_rejects_a_destination_tile_size_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        TileStore::create(dir.path(), 5, SOURCE_ID_ROAD, 256).unwrap();

        let error = build_pyramid(dir.path(), 6, 5, RebuildScope::Tiles(vec![(2, 2)])).unwrap_err();
        assert!(
            error.to_string().contains("destination tile_px"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn incremental_rebuild_refuses_to_bootstrap_a_missing_destination_level() {
        let dir = tempfile::tempdir().unwrap();
        let base = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        base.put_cells(2, 2, &vec![quantise_lden(60.0); TILE_PX * TILE_PX])
            .unwrap();
        base.sync_all().unwrap();
        drop(base);

        let error = build_pyramid(dir.path(), 6, 5, RebuildScope::Tiles(vec![(2, 2)])).unwrap_err();
        assert!(error
            .to_string()
            .contains("incremental pyramid requires existing"));
        assert!(!dir.path().join("z5.qtsi").exists());
        assert!(!dir.path().join(REBUILD_INCOMPLETE_MARKER).exists());
    }

    #[test]
    fn injected_incremental_failure_keeps_fence_until_the_exact_retry_completes() {
        let dir = tempfile::tempdir().unwrap();
        let base = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        base.put_cells(2, 2, &vec![quantise_lden(60.0); TILE_PX * TILE_PX])
            .unwrap();
        base.sync_all().unwrap();
        drop(base);
        build_pyramid(dir.path(), 6, 4, RebuildScope::Full).unwrap();

        let scope = RebuildScope::Tiles(vec![(2, 2)]);
        let error = build_pyramid_managing_fence(dir.path(), 6, 4, scope, &mut |_| {
            anyhow::bail!("injected incremental failure")
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected incremental failure"));
        assert!(dir.path().join(REBUILD_INCOMPLETE_MARKER).exists());

        build_pyramid(dir.path(), 6, 4, RebuildScope::Tiles(vec![(2, 2)])).unwrap();
        assert!(!dir.path().join(REBUILD_INCOMPLETE_MARKER).exists());
    }

    /// Synthetic z=13 → z=11 smoke over STORES: a 2×2 block of
    /// identical-energy tiles at z=13 pools to one z=12 tile, then to one
    /// z=11 tile. Verifies the store pipeline preserves 60 dB → area-mean
    /// 60 dB at each level (within the 0.5 dB quant step).
    #[test]
    fn pyramid_from_z13_writes_each_level() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let body = vec![quantise_lden(60.0); TILE_PX * TILE_PX];

        // Feed the live read-only legacy codec through the current writer. The remaining store
        // census still contains Zstd entries, while every new pyramid level must be BrotliHm3.
        let z13 = TileStore::create(base, 13, SOURCE_ID_AIRCRAFT, TILE_PX as u16).unwrap();
        for (sx, sy) in [(4420, 2772), (4421, 2772), (4420, 2773), (4421, 2773)] {
            put_legacy_zstd_cells(&z13, sx, sy, &body);
        }
        drop(z13);

        let written = build_pyramid(base, 13, 11, RebuildScope::Full).unwrap();
        assert!(
            written >= 2,
            "expected ≥ 2 tiles (one z=12 + one z=11), got {written}"
        );

        let z12 = TileStore::open(base, 12, false).unwrap();
        let z11 = TileStore::open(base, 11, false).unwrap();
        let cells12 = z12.get_cells(2210, 1386).unwrap().expect("z=12 tile");
        let cells11 = z11.get_cells(1105, 693).unwrap().expect("z=11 tile");
        let expected = quantise_lden(60.0);
        assert_eq!(cells12[0], expected, "z=12 first cell must stay 60 dB");
        assert_eq!(cells11[0], expected, "z=11 first cell must stay 60 dB");
        // 2026-07-16: pyramid levels write the ship-out codec directly
        // (put_cells) instead of the legacy ZstdCells working codec —
        // publish no longer has to re-encode these tiles at all.
        assert_eq!(
            z12.get_blob(2210, 1386).unwrap().unwrap().0,
            TileCodec::BrotliHm3
        );
        assert_eq!(
            z11.get_blob(1105, 693).unwrap().unwrap().0,
            TileCodec::BrotliHm3
        );
    }

    /// `RebuildScope::Tiles` cascades from BASE-ZOOM coordinates (z=13 here),
    /// one level ABOVE the first destination — this is the exact off-by-one
    /// the level seeding must get right, or the first fold reads the wrong
    /// quadrant's children entirely (direct regression for a bug caught
    /// in review before this ever ran, 2026-07-15). Two 2×2 blocks far apart
    /// (disjoint z12 AND z11 ancestors) prove both halves of the design: the
    /// dirty block's new value reaches z12 AND z11, and the OTHER block's
    /// ancestors are untouched by a Tiles-mode pass that never named them —
    /// the entire point of the redesign (no bucket-wide rewrite of unrelated
    /// tiles).
    #[test]
    fn pyramid_tiles_scope_cascades_from_base_zoom_and_ignores_untouched_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let a13 = [(4420u32, 2772u32), (4421, 2772), (4420, 2773), (4421, 2773)];
        let b13 = [(3000u32, 1000u32), (3001, 1000), (3000, 1001), (3001, 1001)];
        let (a12, a11) = ((2210u32, 1386u32), (1105u32, 693u32));
        let (b12, b11) = ((1500u32, 500u32), (750u32, 250u32));

        let z13 = TileStore::create(base, 13, SOURCE_ID_AIRCRAFT, TILE_PX as u16).unwrap();
        for &(x, y) in a13.iter().chain(b13.iter()) {
            z13.put_cells(x, y, &vec![quantise_lden(50.0); TILE_PX * TILE_PX])
                .unwrap();
        }
        drop(z13);
        build_pyramid(base, 13, 11, RebuildScope::Full).unwrap();
        let baseline_b11 = TileStore::open(base, 11, false)
            .unwrap()
            .get_cells(b11.0, b11.1)
            .unwrap()
            .expect("b's z=11 baseline");
        let baseline_b12 = TileStore::open(base, 12, false)
            .unwrap()
            .get_cells(b12.0, b12.1)
            .unwrap()
            .expect("b's z=12 baseline");

        // Only block A changes this "cycle" — B's z13 tiles are left as-is.
        let z13 = TileStore::open(base, 13, true).unwrap();
        for &(x, y) in &a13 {
            z13.put_cells(x, y, &vec![quantise_lden(70.0); TILE_PX * TILE_PX])
                .unwrap();
        }
        drop(z13);
        build_pyramid(base, 13, 11, RebuildScope::Tiles(a13.to_vec())).unwrap();

        let z12 = TileStore::open(base, 12, false).unwrap();
        let z11 = TileStore::open(base, 11, false).unwrap();
        let expected70 = quantise_lden(70.0);
        assert_eq!(
            z12.get_cells(a12.0, a12.1).unwrap().expect("a's z=12")[0],
            expected70,
            "the dirty block's new value must reach z=12"
        );
        assert_eq!(
            z11.get_cells(a11.0, a11.1).unwrap().expect("a's z=11")[0],
            expected70,
            "…and cascade all the way to z=11 — this is the off-by-one this test guards"
        );
        assert_eq!(
            z12.get_cells(b12.0, b12.1).unwrap().expect("b's z=12"),
            baseline_b12,
            "a Tiles-mode pass over A alone must not rewrite B's z=12 ancestor at all"
        );
        assert_eq!(
            z11.get_cells(b11.0, b11.1)
                .unwrap()
                .expect("b's z=11 untouched"),
            baseline_b11,
            "…nor B's z=11 ancestor"
        );
    }

    /// Quadrant placement at a given tile size: four children each filled with
    /// a DISTINCT byte must land as four uniform quadrants of the parent (a
    /// 2×2 pool of equal bytes re-quantises to the same byte). Distinct values
    /// catch qx/qy swaps and offset bugs that uniform fills can't see.
    fn assert_pyramid_quadrants(base_zoom: u8) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let px = TILE_PX as u16;
        let n = usize::from(px) * usize::from(px);
        let vals = [100u8, 110, 120, 130];
        // Coordinates valid at both z12 (max 4095) and z13 fixtures.
        let children = [(2420u32, 1772u32), (2421, 1772), (2420, 1773), (2421, 1773)];
        let store = TileStore::create(base, base_zoom, SOURCE_ID_AIRCRAFT, px).unwrap();
        for (i, &(sx, sy)) in children.iter().enumerate() {
            store.put_cells(sx, sy, &vec![vals[i]; n]).unwrap();
        }
        drop(store);

        build_pyramid(base, base_zoom, base_zoom - 1, RebuildScope::Full).unwrap();
        let parent = TileStore::open(base, base_zoom - 1, false).unwrap();
        let cells = parent.get_cells(1210, 886).unwrap().expect("parent tile");
        let (px, half) = (usize::from(px), usize::from(px) / 2);
        for (i, &(sx, sy)) in children.iter().enumerate() {
            let (qx, qy) = ((sx % 2) as usize, (sy % 2) as usize);
            for dy in 0..half {
                for dx in 0..half {
                    let idx = (qy * half + dy) * px + qx * half + dx;
                    assert_eq!(
                        cells[idx], vals[i],
                        "child ({sx},{sy}) quadrant ({qx},{qy}) at ({dx},{dy}), px={px}"
                    );
                }
            }
        }
    }

    #[test]
    fn pyramid_quadrants_512() {
        assert_pyramid_quadrants(12);
    }

    /// A pre-existing corrupt blob (2026-07-13 concurrent-writer bug, fixed at
    /// the write path but not retroactively) among an otherwise-good 2×2 block
    /// must not abort the whole pyramid pass — get_cells_lenient treats it as
    /// absent, `build_pyramid` still succeeds, `corrupt` counts exactly the one
    /// bad tile, and the surviving 3 children still pool correctly (the 4th
    /// quadrant is left at NO_DATA, same as a legitimately-missing child).
    #[test]
    fn corrupt_blob_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let body = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
        let children = [(4420u32, 2772u32), (4421, 2772), (4420, 2773), (4421, 2773)];

        let z13 = TileStore::create(base, 13, SOURCE_ID_AIRCRAFT, TILE_PX as u16).unwrap();
        for &(sx, sy) in &children[..3] {
            z13.put_cells(sx, sy, &body).unwrap();
        }
        z13.put_blob(
            children[3].0,
            children[3].1,
            TileCodec::BrotliHm3,
            b"corrupt",
        )
        .unwrap();
        drop(z13);

        let corrupt = AtomicUsize::new(0);
        let z13 = TileStore::open(base, 13, false).unwrap();
        let cells = get_cells_lenient(&z13, children[3].0, children[3].1, &corrupt)
            .expect("a decode failure must not propagate as Err");
        assert!(cells.is_none(), "corrupt tile reads as absent");
        assert_eq!(corrupt.load(Ordering::Relaxed), 1);

        // The full pyramid build over this store must still succeed end to end.
        let written = build_pyramid(base, 13, 12, RebuildScope::Full).unwrap();
        assert!(written >= 1);
        let z12 = TileStore::open(base, 12, false).unwrap();
        let parent = z12.get_cells(2210, 1386).unwrap().expect("z=12 tile");
        let expected = quantise_lden(60.0);
        assert_eq!(parent[0], expected, "surviving good child still pools");
    }
}
