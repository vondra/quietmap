//! Skip the tiles no source can reach — the quiet-cell work-skip, host side only.
//!
//! A tile no source segment can reach renders all-NO_DATA and `write_tile` returns
//! 0 bytes, so both its kernel launch and its halo crop are pure waste. Most of the
//! world is such a tile: quiet cells are the common case in a world repaint, not the
//! exception. Nothing here is acoustics — the reach box comes from the audited
//! `reach_box_half_extents_deg`, and the per-pixel gate in the kernel stays the only
//! thing that decides audibility.

use std::collections::HashSet;

use anyhow::{Context, Result};
use noise_compute::propagation::geo::reach_box_half_extents_deg;
use std::path::Path;
use tile_painter::grid::tile_range;
use tile_painter::source_line::LineRow;

use crate::LineLayer;

/// Web Mercator's own latitude limit. Clamping to a round 85.0 instead would put
/// the top and bottom tile rows outside every census box, so a high-latitude
/// source would have tiles dropped that the kernel still paints.
const WEB_MERCATOR_MAX_ABS_LAT_DEG: f64 = 85.051_128_779_806_6;

/// A layer with more rows than this keeps the full sweep. Not a tuned optimum: a
/// dense region has almost no silent tiles to find (measured, Dobris z13 road:
/// 168 of 168 owned tiles painted, none silent), so the census cannot pay for the
/// single-threaded host work it costs on the region-load path.
pub const REACH_CENSUS_MAX_ROWS: usize = 200_000;

/// Per layer, the tiles at least one source segment can reach.
///
/// Each row's box is its segment bbox grown by `reach_box_half_extents_deg` at the
/// row's own `max_distance_m` — the same cutoff the kernel culls with, which
/// `pack_sources` writes to `sp[s * 12 + 1]`. That helper is conservative by
/// construction (its box always contains the reach disk, including the polar band
/// where the retired `cos().max(0.2)` clamp under-covered), so the census can only
/// over-cover: a tile admitted in error merely computes a silent tile, while one
/// omitted in error would drop real energy. A layer over `REACH_CENSUS_MAX_ROWS`
/// gets no entry at all, which means nothing is skipped for it.
pub fn build_reach_census(
    z: u8,
    region_tiles: &[(u32, u32)],
    region_rows: &[(LineLayer, Vec<LineRow>)],
) -> Vec<(LineLayer, HashSet<(u32, u32)>)> {
    // Clip every stamp to the region's own tile bbox: only tiles this region owns
    // are ever looked up, and at the row cap an unclipped stamp is millions of
    // inserts for tiles nobody asks about.
    let (Some(bx0), Some(bx1), Some(by0), Some(by1)) = (
        region_tiles.iter().map(|t| t.0).min(),
        region_tiles.iter().map(|t| t.0).max(),
        region_tiles.iter().map(|t| t.1).min(),
        region_tiles.iter().map(|t| t.1).max(),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (layer, rows) in region_rows {
        if rows.len() > REACH_CENSUS_MAX_ROWS {
            continue;
        }
        let mut set = HashSet::new();
        for row in rows {
            let (lat_lo, lat_hi) = (
                row.start_lat.min(row.end_lat),
                row.start_lat.max(row.end_lat),
            );
            let (lon_lo, lon_hi) = (
                row.start_lon.min(row.end_lon),
                row.start_lon.max(row.end_lon),
            );
            let (reach_lat_deg, reach_lon_deg) = reach_box_half_extents_deg(
                lat_hi.abs().max(lat_lo.abs()),
                row.max_distance_m.max(0.0),
            );
            let (xs, ys) = tile_range(
                z,
                (lat_lo - reach_lat_deg).max(-WEB_MERCATOR_MAX_ABS_LAT_DEG),
                lon_lo - reach_lon_deg,
                (lat_hi + reach_lat_deg).min(WEB_MERCATOR_MAX_ABS_LAT_DEG),
                lon_hi + reach_lon_deg,
            );
            let ys: Vec<u32> = ys.filter(|y| (by0..=by1).contains(y)).collect();
            for x in xs.filter(|x| (bx0..=bx1).contains(x)) {
                for &y in &ys {
                    set.insert((x, y));
                }
            }
        }
        out.push((*layer, set));
    }
    out
}

/// Drop the region's tiles NO layer can reach, before any halo is cropped.
///
/// This is the union across layers, and it is deliberately coarser than the
/// per-(tile, layer) filter the block path runs: a tile road reaches but rail does
/// not survives here and has only its rail pair dropped later. Both are needed —
/// this one saves the halo crop, which on a quiet cell outweighs the kernel
/// (measured, three quiet z13 cells, road, card exclusive: 21.5 s of GPU pipeline
/// against 9.8 s of raster), the finer one saves the launches this one keeps.
///
/// The stale-output unlink still has to happen for every tile dropped here, exactly
/// as the `write_tile` == 0 path does it, or a rebuild leaves a prior build's tile.
pub fn drop_unreachable_tiles(
    region_tiles: &[(u32, u32)],
    reach: &[(LineLayer, HashSet<(u32, u32)>)],
    layers: &[LineLayer],
    output: Option<&String>,
    z: u8,
) -> Result<(Vec<(u32, u32)>, usize)> {
    // A layer with no census entry (over the row cap) may reach anywhere, so the
    // union is unknown and nothing may be dropped region-wide.
    if layers.iter().any(|l| !reach.iter().any(|(rl, _)| rl == l)) {
        return Ok((region_tiles.to_vec(), 0));
    }
    let mut kept = Vec::with_capacity(region_tiles.len());
    let mut removed = 0usize;
    for &(tx, ty) in region_tiles {
        if reach.iter().any(|(_, set)| set.contains(&(tx, ty))) {
            kept.push((tx, ty));
            continue;
        }
        if let Some(root) = output {
            for layer in layers {
                removed += usize::from(unlink_stale_tile(root, *layer, z, tx, ty)?);
            }
        }
    }
    Ok((kept, removed))
}

/// Unlink the tile a previous build may have left where this build writes nothing.
/// Without it `combine` keeps summing that tile's energy (mirrors
/// build_heatmap_surface). Shared by the all-silent write path and by both census
/// early-outs, which reach the same state without running the kernel.
/// Returns whether a tile was actually removed.
pub fn unlink_stale_tile(root: &str, layer: LineLayer, z: u8, tx: u32, ty: u32) -> Result<bool> {
    let out = Path::new(root)
        .join(layer.dir())
        .join(z.to_string())
        .join(tx.to_string())
        .join(format!("{ty}.bin"));
    match std::fs::remove_file(&out) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("rm stale {}", out.display())),
    }
}
