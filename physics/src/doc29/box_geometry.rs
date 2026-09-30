//! The size of an aircraft box (PLAN section 5): a web-map cell horizontally and a slab of the
//! same edge vertically, counted as clearance above the highest terrain within one edge. The edge
//! grows with clearance so that one edge spans at most `BOX_EDGE_LEVEL_STEP_DB` of the steepest
//! NPD curve at that clearance (the slant to a receiver right below), never finer than the first
//! layer of about 50 m: nearer the ground the altitude error of the data dominates anyway.

use super::npd::{METRES_PER_FOOT, NPD_DISTANCES_FT, steepest_sel_slope_db_per_m};

/// D: the level change one box edge may span along the steepest NPD curve (dB).
pub const BOX_EDGE_LEVEL_STEP_DB: f64 = 3.0;
/// D of the far boxes, read from the second ring of z12 tiles on: at least a tile edge away the
/// steepest slope is a fraction of the one at the boxes' clearance, so four times the edges hold.
pub const FAR_BOX_EDGE_LEVEL_STEP_DB: f64 = 12.0;
/// The first layer's edge (m): the web-map zoom whose cell edge is nearest to it is the finest.
pub const FIRST_LAYER_EDGE_M: f64 = 50.0;

/// The largest box edge (m) allowed at `clearance_m` above the terrain for a level step D of
/// `level_step_db`.
pub fn box_edge_limit_m(clearance_m: f64, level_step_db: f64) -> f64 {
    level_step_db / steepest_sel_slope_db_per_m(clearance_m.max(0.0))
}

/// The smallest limit over the clearances from `low_m` to `high_m`: the steepest slope jumps up
/// at some NPD distances, so the floor alone may overstate what a band may span.
fn band_edge_limit_m(low_m: f64, high_m: f64, level_step_db: f64) -> f64 {
    let npd_distances_m = NPD_DISTANCES_FT.map(|feet| feet * METRES_PER_FOOT);
    npd_distances_m
        .iter()
        .filter(|&&distance| low_m < distance && distance < high_m)
        .chain([low_m, high_m].iter())
        .map(|&clearance| box_edge_limit_m(clearance, level_step_db))
        .fold(f64::INFINITY, f64::min)
}

/// One clearance band: boxes of cell edge `edge_m` (web-map zoom `zoom`) from `clearance_m` to
/// `clearance_m + edge_m` above the terrain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearanceBand {
    pub zoom: u8,
    pub edge_m: f64,
    pub clearance_m: f64,
}

/// The clearance bands up to `top_m` for web-map cells whose edge at zoom z is
/// `cell_edge_m(z)` (the equatorial edge times the cosine of the tile's latitude): from the
/// ground up, each band takes the coarsest zoom whose edge fits the limit over its height, never
/// finer than the first layer's zoom nor coarser than `coarsest_zoom`. A band spans at most D
/// (`level_step_db`) over its whole height; a band above may take a finer zoom where the steepest
/// slope jumps up.
pub fn clearance_bands(
    cell_edge_m: impl Fn(u8) -> f64,
    coarsest_zoom: u8,
    top_m: f64,
    level_step_db: f64,
) -> Vec<ClearanceBand> {
    let finest_zoom = (coarsest_zoom..=FINEST_ZOOM)
        .min_by(|&a, &b| {
            (cell_edge_m(a) / FIRST_LAYER_EDGE_M)
                .ln()
                .abs()
                .total_cmp(&(cell_edge_m(b) / FIRST_LAYER_EDGE_M).ln().abs())
        })
        .expect("a zoom range");
    let mut bands = Vec::new();
    let mut clearance_m = 0.0;
    while clearance_m < top_m {
        let zoom = (coarsest_zoom..finest_zoom)
            .find(|&zoom| {
                let edge_m = cell_edge_m(zoom);
                edge_m <= band_edge_limit_m(clearance_m, clearance_m + edge_m, level_step_db)
            })
            .unwrap_or(finest_zoom);
        let edge_m = cell_edge_m(zoom);
        bands.push(ClearanceBand {
            zoom,
            edge_m,
            clearance_m,
        });
        clearance_m += edge_m;
    }
    bands
}

/// The finest zoom a box may take (a cell under 20 m anywhere).
pub const FINEST_ZOOM: u8 = 22;

#[cfg(test)]
#[path = "box_geometry_tests.rs"]
mod tests;
