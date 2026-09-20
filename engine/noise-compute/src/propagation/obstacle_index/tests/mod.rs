//! Shared metric fixtures for obstacle-index regression tests.

use super::geometry::{ray_cell_aabb, ray_cell_aabb_may_overlap};
use super::grid_build::obstacle_grid_cell_m;
use super::*;
use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};

mod construction;
mod containment;
mod crossings;
mod pruning;
mod ray_cell_bounds;
mod reflection;
mod set;
mod skyline;
mod slab_reject;

const OLAT: f64 = 50.0;
const OLON: f64 = 14.0;

fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
    (
        OLAT + y_m / M_PER_DEG_LAT,
        OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
    )
}

fn square(cx: f64, cy: f64, half: f64) -> Vec<(f64, f64)> {
    vec![
        ll(cx - half, cy - half),
        ll(cx + half, cy - half),
        ll(cx + half, cy + half),
        ll(cx - half, cy + half),
    ]
}

fn run(idx: &ObstacleIndex, from: (f64, f64), to: (f64, f64)) -> Vec<CrossingCandidate> {
    let mut out = Vec::new();
    idx.crossings(from.0, from.1, to.0, to.1, &mut out);
    out
}
