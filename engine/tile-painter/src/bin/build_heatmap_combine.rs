//! Combine the per-layer tile stores into the precomputed `total/` layer — the
//! default all-layers-on view 80 % of visitors load as a single tile fetch.
//!
//! Per base-zoom tile (the union of every layer's present tiles, optionally
//! bbox-clipped), per cell: `total = 10·log10(Σ_layers 10^(layer_Lden/10))`,
//! skipping `NO_DATA` cells. This energy-sum is EXACT, not an approximation:
//! `Lden` is a fixed linear blend of the period mean-square pressures (12/24
//! day + 4/24·10^0.5 evening + 8/24·10^1 night), so summing `10^(Lden/10)`
//! across layers equals the blend of the per-layer period-energy sums. The
//! only error is the ±0.25 dB each layer already carries from `u8 × 0.5 dB`
//! quantisation.
//!
//! Store-to-store: reads the 7 layer stores, writes the `total` store via the
//! ship-out `BrotliHm3` codec directly (`TileStore::put_cells`, since
//! 2026-07-16 — was the `ZstdCells` working codec, deferring the Brotli-q9
//! encode to the pmtiles pack; see `TileCodec::ZstdCells`'s doc for why that
//! made every publish redo it), then rolls the `total` pyramid. The old
//! 15-stat skip-gate, column-dir-mtime hack, TOCTOU re-probe and
//! `.manifest.json` are GONE (2026-07 storage redesign): the store index is
//! the presence truth, deletes are tombstones, and cross-process write
//! exclusivity is enforced by the store ITSELF (each writable open takes a
//! per-(layer,zoom) flock — `tile_store::store::acquire_write_lock`), with the
//! `_combine` master flock as the coarser outer serialiser.
//!
//!   build-heatmap-combine --store-root data/tiles/2026/store
//!   build-heatmap-combine --store-root … --bbox 49.9,14.3,50.1,14.6
//!
//! The combine zoom is DERIVED from the stores (finest common level) — no
//! CLI flag for a value the data carries (project rule).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{bail, Result};
use clap::Parser;
use rayon::prelude::*;

use tile_painter::grid::tile_range;
use tile_painter::pyramid::{
    build_pyramid_with_existing_rebuild_fence, get_cells_lenient, rebuild_scope_fingerprint,
    require_incremental_pyramid_levels, scope_from_args, RebuildScope,
};
use tile_painter::tile_store::{StoreRebuildFence, TileStore, TOTAL_INPUT_LAYERS};
use tile_painter::wire_hm3::{dequantise_lden, quantise_lden, NO_DATA, SOURCE_ID_TOTAL};

#[derive(Parser, Debug)]
struct Args {
    /// Store root holding the per-layer store dirs; `total/` is written
    /// alongside them.
    #[arg(long)]
    store_root: PathBuf,
    /// Lowest pyramid level to build for `total/`. Defaults to z2 — the zoom
    /// the serving stack starts `total` at (server MIN_ZOOM in
    /// heatmap-shared.ts; combine-loop.sh passes the same), so a defaulted
    /// full combine can never leave the packer a hole at the world view.
    #[arg(long, default_value_t = 2)]
    dst_zoom: u8,
    /// `south_lat,west_lon,north_lat,east_lon` — limit the combine to this
    /// bbox; default is every tile present in any layer. Mutually exclusive
    /// with `--tiles-from`.
    #[arg(long, value_parser = parse_bbox)]
    bbox: Option<[f64; 4]>,
    /// Path(s) to a file of exact `x y` base-zoom tile pairs (one per line) —
    /// repeatable, one per layer whose tiles changed (total/'s tile at (x,y)
    /// can be dirty because ANY of them changed there — this flag's own
    /// union, not each file separately, is what gets combined). Skips the
    /// whole-store scan entirely, and cascades `total/`'s own pyramid from
    /// that same union, not a bbox. Mutually exclusive with `--bbox`. See
    /// pyramid::RebuildScope for the rationale.
    #[arg(long)]
    tiles_from: Vec<PathBuf>,
}

fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.parse())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bbox: {e}"))?;
    if v.len() != 4 {
        return Err("expected south,west,north,east".into());
    }
    if v[0] >= v[2] || v[1] >= v[3] {
        return Err("need south<north and west<east".into());
    }
    Ok([v[0], v[1], v[2], v[3]])
}

fn main() -> Result<()> {
    let args = Args::parse();
    let scope = scope_from_args(args.bbox, &args.tiles_from)?;
    let incremental = !matches!(&scope, RebuildScope::Full);
    let total_dir = args.store_root.join("total");
    let zoom = finest_common_zoom(&args.store_root)?;
    let clip = args.bbox.map(|b| tile_range(zoom, b[0], b[1], b[2], b[3]));

    // Open every layer store at the derived base zoom (a layer not yet built
    // is simply absent from the sum — finest_common_zoom already skipped it).
    let layer_stores: Vec<TileStore> = TOTAL_INPUT_LAYERS
        .iter()
        .filter_map(|l| {
            let dir = args.store_root.join(l);
            dir.join(format!("z{zoom}.qtsi"))
                .exists()
                .then(|| TileStore::open(&dir, zoom, false))
        })
        .collect::<Result<_>>()?;

    // Size-agnostic: the layer stores' header carries the tile side (256
    // pre-shift, 512 post) — every store in one combine must agree, and the
    // total store inherits it.
    let px = layer_stores[0].tile_px();
    if let Some(bad) = layer_stores.iter().find(|s| s.tile_px() != px) {
        bail!("mixed tile_px in layer stores: {} vs {px}", bad.tile_px());
    }
    let full_rebuild = matches!(&scope, RebuildScope::Full);
    if incremental {
        // An incremental total is an update, never a hidden first-world bootstrap. Validate the
        // whole derived band before changing its base tile so a missing ancestor fails with zero
        // mutations and an explicit full-rebuild recovery.
        require_incremental_pyramid_levels(&total_dir, zoom, args.dst_zoom)?;
    }
    // Base total + every ancestor are one transaction for every scope. Without the durable fence,
    // a crash after z12 or a direct pack between incremental levels can expose a mixed cascade.
    let operation = if full_rebuild {
        "combine-total".to_string() // preserves adoption of a legacy full-combine crash marker
    } else {
        format!("combine-total:{}", rebuild_scope_fingerprint(&scope))
    };
    let rebuild_fence = StoreRebuildFence::begin(&total_dir, &operation)?;

    // The `total` destination store. Full combine: recreate from scratch —
    // total/ is fully derived, so stale tiles die wholesale. Bbox/tiles-from
    // combine: overwrite the existing store in place (everything outside the
    // scope stays valid).
    let total = if incremental {
        TileStore::open(&total_dir, zoom, true)?
    } else {
        TileStore::create(&total_dir, zoom, SOURCE_ID_TOTAL, px)?
    };

    // Tiles to combine. Tiles scope: exactly the (already-unioned, see
    // scope_from_args) caller list — no store scan at all, the entire point
    // being that the caller already knows precisely which base-zoom tiles
    // changed (see pyramid::RebuildScope). Otherwise, the union of every
    // layer's present tiles (clipped). A bbox combine also visits EXISTING
    // total tiles in range: an (x,y) that vanished from every layer is
    // absent from the layer union, and its now-orphaned total tile must be
    // tombstoned or the incremental pyramid would read phantom energy —
    // Tiles scope doesn't need this: the caller's list already covers every
    // (x,y) whose CONSTITUENT layer data changed, orphaning included.
    let mut tiles: BTreeSet<(u32, u32)> = BTreeSet::new();
    if let RebuildScope::Tiles(list) = &scope {
        tiles.extend(list.iter().copied());
    } else {
        let mut scan = |store: &TileStore| -> Result<()> {
            match &clip {
                // Guard the degenerate empty range: `end.saturating_sub(1)` on
                // `0..0` would otherwise scan the whole x=0 column (review find).
                Some((xr, _)) if xr.is_empty() => Ok(()),
                Some((xr, yr)) => {
                    store.for_each_present_in_x_range(xr.start, xr.end - 1, |x, y, _| {
                        if yr.contains(&y) {
                            tiles.insert((x, y));
                        }
                        Ok(())
                    })
                }
                None => store.for_each_present(|x, y, _| {
                    tiles.insert((x, y));
                    Ok(())
                }),
            }
        };
        for store in &layer_stores {
            scan(store)?;
        }
        if args.bbox.is_some() {
            scan(&total)?;
        }
    }
    eprintln!("{} tile(s) to combine at z={}", tiles.len(), zoom);

    // Each (x,y) reads its own layer cells and writes its own disjoint store
    // slot — embarrassingly parallel (atomic tail reservation, disjoint index
    // entries). The Brotli-q9 encode (put_cells) happens once per tile
    // here, at combine time, amortized over the days between publishes —
    // never redone at ship-out (see the module doc).
    let t = Instant::now();
    let n_cells = usize::from(px) * usize::from(px);
    let written = AtomicUsize::new(0);
    let empty = AtomicUsize::new(0);
    let corrupt = AtomicUsize::new(0);
    let tile_vec: Vec<(u32, u32)> = tiles.into_iter().collect();
    tile_vec.par_iter().try_for_each_init(
        // Per-worker scratch — the combine loop reruns this forever, so the
        // 256 KiB output buffer must not be re-malloc'd per tile. (The decoded
        // layer cells are still fresh Vecs from get_cells — the store API
        // allocates per read.)
        || (Vec::with_capacity(layer_stores.len()), vec![0u8; n_cells]),
        |(layer_tiles, out_cells): &mut (Vec<Vec<u8>>, Vec<u8>), &(x, y)| -> Result<()> {
            layer_tiles.clear();
            for store in &layer_stores {
                let cells = get_cells_lenient(store, x, y, &corrupt)?;
                if let Some(cells) = cells {
                    if cells.len() != n_cells {
                        bail!(
                            "layer tile {x}/{y}: {} cells, expected {n_cells}",
                            cells.len()
                        );
                    }
                    layer_tiles.push(cells);
                }
            }
            if layer_tiles.is_empty() {
                // Every layer vanished here — tombstone any stale total tile so
                // the incremental pyramid can't propagate phantom energy upward.
                total.delete(x, y)?;
                empty.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            for (i, out) in out_cells.iter_mut().enumerate() {
                *out = combine_cell(layer_tiles.iter().map(|t| t[i]));
            }
            // put_cells tombstones an all-silent result itself (present ⟺ audible).
            if total.put_cells(x, y, out_cells)? > 0 {
                written.fetch_add(1, Ordering::Relaxed);
            } else {
                empty.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        },
    )?;
    total.sync_all()?;
    // Release total/z{base}'s per-store write lock BEFORE the pyramid runs:
    // build_pyramid reopens this base level read-only as its own source and
    // writes the LOWER levels, so keeping the writable handle here would nest two
    // per-store write locks in one process. Dropping it keeps the store's
    // one-writable-lock-at-a-time invariant true (store.rs::acquire_write_lock)
    // and removes any latent lock-ordering surface (dual /gg, 2026-07-15).
    drop(total);
    let corrupt = corrupt.load(Ordering::Relaxed);
    let corrupt_note = if corrupt > 0 {
        format!(", {corrupt} UNREADABLE (skipped — self-heals on re-ingest)")
    } else {
        String::new()
    };
    eprintln!(
        "combined: {} written, {} empty{corrupt_note}, {:.1} s",
        written.load(Ordering::Relaxed),
        empty.load(Ordering::Relaxed),
        t.elapsed().as_secs_f64()
    );

    let tag = match &scope {
        RebuildScope::Tiles(_) => " (tiles)",
        RebuildScope::Bbox(_) => " (bbox)",
        RebuildScope::Full => "",
    };
    let n = build_pyramid_with_existing_rebuild_fence(&total_dir, zoom, args.dst_zoom, scope)?;
    eprintln!(
        "total/ pyramid z{}→z{}{tag}: {n} tiles",
        zoom, args.dst_zoom
    );
    rebuild_fence.finish()?;
    Ok(())
}

/// The combine base zoom, derived from the data: the finest `z*.qtsi` level of
/// every present layer store, which must AGREE across layers — a mixed finest
/// (e.g. one layer still 256@z13 in a 512@z12 root) means a half-shifted store
/// root, and summing only the odd layer out would silently drop the rest.
fn finest_common_zoom(store_root: &std::path::Path) -> Result<u8> {
    let mut finest: Option<(u8, &str)> = None;
    for layer in TOTAL_INPUT_LAYERS {
        let Some(z) = finest_level(&store_root.join(layer))? else {
            continue; // layer not built yet — absent from the sum by design
        };
        match finest {
            None => finest = Some((z, layer)),
            Some((z0, l0)) if z0 != z => bail!(
                "mixed finest levels under {}: {l0} has z{z0}, {layer} has z{z} — \
                 half-shifted store root, refusing to combine",
                store_root.display()
            ),
            Some(_) => {}
        }
    }
    finest
        .map(|(z, _)| z)
        .ok_or_else(|| anyhow::anyhow!("no layer stores under {}", store_root.display()))
}

/// Highest zoom among a layer dir's `z{N}.qtsi` files (base > pyramid levels),
/// or `None` when the dir is missing or holds no store.
fn finest_level(dir: &std::path::Path) -> Result<Option<u8>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut max: Option<u8> = None;
    for e in entries {
        let name = e?.file_name();
        if let Some(z) = name
            .to_string_lossy()
            .strip_prefix('z')
            .and_then(|s| s.strip_suffix(".qtsi"))
            .and_then(|s| s.parse::<u8>().ok())
        {
            max = Some(max.map_or(z, |m| m.max(z)));
        }
    }
    Ok(max)
}

/// Energy-sum the quantised per-layer `Lden` bytes for one cell into the total
/// `Lden` byte: `10·log10(Σ 10^(Lden_i/10))` over the cells that carry data.
/// All `NO_DATA` → `NO_DATA` (no layer is audible here).
fn combine_cell(layer_bytes: impl Iterator<Item = u8>) -> u8 {
    let mut energy = 0.0f64;
    let mut has = false;
    for b in layer_bytes {
        if b != NO_DATA {
            energy += 10f64.powf(dequantise_lden(b) / 10.0);
            has = true;
        }
    }
    if has {
        quantise_lden(10.0 * energy.log10())
    } else {
        NO_DATA
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate: road 60 dB + rail 55 dB energy-sum to ~61.2 dB (the plan's check).
    #[test]
    fn energy_sum_two_layers() {
        let total = dequantise_lden(combine_cell(
            [quantise_lden(60.0), quantise_lden(55.0)].into_iter(),
        ));
        assert!(
            (total - 61.19).abs() < 0.5,
            "60 + 55 → {total:.2}, expected ~61.2"
        );
    }

    /// NO_DATA layers are skipped; a lone audible layer passes through; all
    /// silent → NO_DATA.
    #[test]
    fn no_data_handling() {
        assert_eq!(combine_cell([NO_DATA, NO_DATA].into_iter()), NO_DATA);
        let lone = dequantise_lden(combine_cell([quantise_lden(48.0), NO_DATA].into_iter()));
        assert!(
            (lone - 48.0).abs() <= 0.25,
            "lone layer ≈ itself (±quant), got {lone:.2}"
        );
    }

    /// Summing N equal layers adds 10·log10(N): four 50 dB layers → ~56 dB.
    #[test]
    fn equal_layers_add_ten_log_n() {
        let four = dequantise_lden(combine_cell([quantise_lden(50.0); 4].into_iter()));
        assert!((four - 56.02).abs() < 0.5, "4×50 → {four:.2}, expected ~56");
    }
}
