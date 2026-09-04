//! Cell-anchored terrain halos: one rectangle normally, two compact strips at the dateline.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use noise_compute::emission::aircraft::RECEIVER_HORIZON_MAX_M;
use raster_reader::fused_grid::FusedGrid;
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBbox};
use raster_reader::RealRasters;
use tile_painter::grid::tile_bbox;
use tile_painter::region_runner::region_tiles;

pub(super) fn tiles_bbox(z: u8, tiles: &[(u32, u32)]) -> Result<TileBbox> {
    if tiles.is_empty() {
        bail!("no tile at z{z} to bound");
    }
    let mut bounds = tile_bbox(z, tiles[0].0, tiles[0].1);
    for &(x, y) in &tiles[1..] {
        let tile = tile_bbox(z, x, y);
        bounds.south_lat = bounds.south_lat.min(tile.south_lat);
        bounds.north_lat = bounds.north_lat.max(tile.north_lat);
        bounds.west_lon = bounds.west_lon.min(tile.west_lon);
        bounds.east_lon = bounds.east_lon.max(tile.east_lon);
    }
    Ok(TileBbox {
        south_lat: bounds.south_lat,
        north_lat: bounds.north_lat,
        west_lon: bounds.west_lon,
        east_lon: bounds.east_lon,
    })
}

pub(super) fn cell_horizon_rectangles(z: u8, r4: u64) -> Result<Vec<TileBbox>> {
    let tiles = region_tiles(r4, z);
    let whole = tiles_bbox(z, &tiles).with_context(|| format!("horizon halo of R4 {r4:015x}"))?;
    if whole.east_lon - whole.west_lon <= 180.0 {
        return Ok(vec![whole]);
    }
    let (west, east): (Vec<_>, Vec<_>) = tiles.into_iter().partition(|&(x, _)| x < (1 << z) / 2);
    Ok(vec![tiles_bbox(z, &west)?, tiles_bbox(z, &east)?])
}

/// Anchor on all owned tiles, never a requested window; this preserves the
/// elevation lattice and its samples between whole-cell and windowed painting.
pub(super) fn cell_horizon_halos(
    rasters: &RealRasters,
    z: u8,
    r4: u64,
) -> Result<Vec<Arc<FusedGrid>>> {
    Ok(cell_horizon_rectangles(z, r4)?
        .iter()
        .map(|receivers| {
            rasters.preload_dem_bbox(
                receivers.south_lat,
                receivers.north_lat,
                receivers.west_lon,
                receivers.east_lon,
            );
            FusedTileZ13::build_elevation_halo(receivers, RECEIVER_HORIZON_MAX_M, rasters)
        })
        .collect())
}
