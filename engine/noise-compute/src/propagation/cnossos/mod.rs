//! CNOSSOS-EU boundary attenuation of one source→receiver path in the vertical plane (Directive
//! 2015/996 Annex II §2.5 as corrected and amended by 2021/1226; ISO/TR 17534-4:2020): ground
//! and multiple-edge diffraction per meteorological state, never mixed before the boundary term
//! is complete.
//!
//! Submodules: [`mean_plane`] (2.5.2–2.5.4 fits, heights, mirrors), [`ground`] (2.5.14–2.5.20),
//! [`rubber_band`] (diffraction points per state), [`diffraction`] (2.5.21–2.5.32 and the
//! Rayleigh criterion).

pub mod diffraction;
pub mod ground;
pub mod mean_plane;
pub mod rubber_band;

pub use ground::MeteorologicalState;

use crate::types::NUM_BANDS;
use rubber_band::{DiffractionPath, PlanePoint};

/// The ground of one path in the vertical plane: vertices at ascending horizontal distance from
/// the source (first 0, last the path length) carrying altitude and ground factor G, both linear
/// between vertices; two vertices at one distance make a step. Building roofs are part of it
/// (raised hard ground, as the ISO/TR 17534-4 reference profiles have them); walls are not.
#[derive(Debug, Clone, Copy)]
pub struct VerticalProfile<'a> {
    pub distance_m: &'a [f64],
    pub altitude_m: &'a [f64],
    pub ground_factor: &'a [f64],
}

impl VerticalProfile<'_> {
    /// Ground altitude at `x`, the upper side of a vertical step at exactly `x`.
    pub fn ground_altitude_at(&self, x: f64) -> f64 {
        interpolate_with_steps(self.distance_m, self.altitude_m, x, false)
    }

    /// Ground altitude at `x`, the lower (source-side) side of a vertical step at exactly `x`.
    pub fn ground_altitude_before(&self, x: f64) -> f64 {
        interpolate_with_steps(self.distance_m, self.altitude_m, x, true)
    }

    /// Mean ground factor over `[start_m, end_m]` in horizontal projection (2021/1226 point
    /// (9)(a)), G linear between vertices; G at `start_m` for a range shorter than a millimetre.
    pub fn mean_ground_factor(&self, start_m: f64, end_m: f64) -> f64 {
        let (d, g) = (self.distance_m, self.ground_factor);
        let at = |x: f64, before: bool| interpolate_with_steps(d, g, x, before);
        if end_m - start_m < 1e-3 {
            return at(start_m, false);
        }
        let first = d.partition_point(|&v| v <= start_m);
        let last = d.partition_point(|&v| v < end_m);
        let mut previous = (start_m, at(start_m, false));
        let mut total = 0.0;
        for index in first..last {
            total += 0.5 * (previous.1 + g[index]) * (d[index] - previous.0);
            previous = (d[index], g[index]);
        }
        total += 0.5 * (previous.1 + at(end_m, true)) * (end_m - previous.0);
        total / (end_m - start_m)
    }
}

/// `y` at `x` on the polyline `(xs, ys)`, linear between vertices; at a step exactly at `x` the
/// upper (later) vertex, or with `before` the lower (earlier) one.
fn interpolate_with_steps(xs: &[f64], ys: &[f64], x: f64, before: bool) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[xs.len() - 1] {
        return ys[ys.len() - 1];
    }
    let upper = if before {
        xs.partition_point(|&v| v < x)
    } else {
        xs.partition_point(|&v| v <= x)
    };
    if before && xs[upper] == x {
        return ys[upper];
    }
    let (x0, x1) = (xs[upper - 1], xs[upper]);
    if x1 == x0 {
        return ys[upper];
    }
    ys[upper - 1] + (x - x0) / (x1 - x0) * (ys[upper] - ys[upper - 1])
}

/// One path in the vertical plane: source S at distance 0, receiver R at the path length.
#[derive(Debug, Clone, Copy)]
pub struct VerticalPath<'a> {
    pub profile: VerticalProfile<'a>,
    pub source: PlanePoint,
    pub receiver: PlanePoint,
    /// Gs of (2.5.14): the ground the source stands on.
    pub source_ground_factor: f64,
    /// Terrain points that may diffract, sorted by distance: the bare-earth samples, roofs
    /// aside (empty for the popup's "no terrain" hypothesis).
    pub terrain_candidates: &'a [PlanePoint],
    /// Obstacle tops (building walls and barriers) as candidates, sorted by distance.
    pub obstacle_tops: &'a [PlanePoint],
}

/// The boundary term of one state per band.
#[derive(Debug, Clone, Default)]
pub struct StateBoundary {
    /// A_boundary (2.5.5 / 2.5.7): A_dif in diffracted bands, A_ground otherwise.
    pub attenuation_db: [f64; NUM_BANDS],
    /// The same with every ground term removed (the popup's "no ground" hypothesis).
    pub without_ground_db: [f64; NUM_BANDS],
    /// A_ground of the whole path, diffraction ignored (the popup's free-field hypothesis).
    pub whole_path_ground_db: [f64; NUM_BANDS],
    pub diffracted: [bool; NUM_BANDS],
    pub path: DiffractionPath,
    /// Signed path difference of `path` (0 without a path).
    pub path_difference_m: f64,
}

/// Reusable per-thread buffers.
#[derive(Default)]
pub struct VerticalPathScratch {
    candidates: Vec<PlanePoint>,
    lowered: Vec<(f64, f64, usize)>,
}

/// A_boundary of `path` in `state`.
pub fn state_boundary(
    path: &VerticalPath<'_>,
    state: MeteorologicalState,
    scratch: &mut VerticalPathScratch,
) -> StateBoundary {
    let candidates = &mut scratch.candidates;
    candidates.clear();
    let length = path.receiver.0;
    let inside = |point: &&PlanePoint| point.0 > 0.0 && point.0 < length;
    candidates.extend(path.terrain_candidates.iter().filter(inside));
    candidates.extend(path.obstacle_tops.iter().filter(inside));
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    diffraction::boundary_for_candidates(path, state, candidates, &mut scratch.lowered)
}

#[cfg(test)]
#[path = "iso_tr_17534_4_tests.rs"]
mod iso_tr_17534_4_tests;

#[cfg(test)]
#[path = "boundary_gain_tests.rs"]
mod boundary_gain_tests;
