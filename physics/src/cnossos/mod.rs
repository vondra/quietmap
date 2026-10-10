//! CNOSSOS-EU boundary attenuation of one source-receiver path in the vertical plane (Directive
//! 2015/996 Annex II 2.5 as amended by 2021/1226; ISO/TR 17534-4:2020): ground and multiple-edge
//! diffraction per meteorological state, the states mixed only after the boundary term.
//!
//! Submodules: [`mean_plane`] (2.5.2-2.5.4 fits, heights, mirrors), [`ground`] (2.5.14-2.5.20),
//! [`rubber_band`] (diffraction points per state), [`diffraction`] (2.5.21-2.5.32, Rayleigh).

pub mod diffraction;
pub mod ground;
pub mod mean_plane;
pub mod rubber_band;

pub use ground::MeteorologicalState;
pub use rubber_band::{DiffractionPath, PlanePoint};

use crate::bands::BANDS;

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
    /// Terrain points that may diffract, sorted by distance: the bare-earth samples (empty for
    /// the "no terrain" hypothesis).
    pub terrain_candidates: &'a [PlanePoint],
    /// Obstacle tops (building walls and barriers) as candidates, sorted by distance.
    pub obstacle_tops: &'a [PlanePoint],
}

/// The boundary term of one state per band.
#[derive(Debug, Clone, Default)]
pub struct StateBoundary {
    /// A_boundary (2.5.5 / 2.5.7): A_dif in diffracted bands, A_ground otherwise.
    pub attenuation_db: [f64; BANDS],
    /// The same with every ground term removed (the "no ground" hypothesis of the display).
    pub without_ground_db: [f64; BANDS],
    /// A_ground of the whole path, diffraction ignored (the free-field hypothesis).
    pub whole_path_ground_db: [f64; BANDS],
    /// Signed path difference of the diffraction path (0 without one).
    pub path_difference_m: f64,
}

/// Reusable per-thread buffers; `path` holds the diffraction points of the last boundary,
/// `calm_path` the homogeneous state's of [`state_boundaries`].
#[derive(Default)]
pub struct VerticalPathScratch {
    candidates: Vec<PlanePoint>,
    blocking: Vec<(f64, f64, usize)>,
    hull: Vec<(f64, f64, usize)>,
    pub path: DiffractionPath,
    pub calm_path: DiffractionPath,
}

/// A_boundary of `path` in `state`.
pub fn state_boundary(
    path: &VerticalPath<'_>,
    state: MeteorologicalState,
    scratch: &mut VerticalPathScratch,
) -> StateBoundary {
    collect_candidates(path, &mut scratch.candidates);
    let direct = diffraction::DirectPath::of(path);
    boundary_on_candidates(path, state, &direct, scratch)
}

/// A_boundary of `path` in both states (homogeneous, favourable), the candidates and the direct
/// path's mean plane taken once; the homogeneous diffraction points are left in
/// `scratch.calm_path`.
pub fn state_boundaries(
    path: &VerticalPath<'_>,
    scratch: &mut VerticalPathScratch,
) -> [StateBoundary; 2] {
    collect_candidates(path, &mut scratch.candidates);
    let direct = diffraction::DirectPath::of(path);
    let calm = boundary_on_candidates(path, MeteorologicalState::Homogeneous, &direct, scratch);
    std::mem::swap(&mut scratch.calm_path, &mut scratch.path);
    let downwind = boundary_on_candidates(path, MeteorologicalState::Favourable, &direct, scratch);
    [calm, downwind]
}

fn boundary_on_candidates(
    path: &VerticalPath<'_>,
    state: MeteorologicalState,
    direct: &diffraction::DirectPath,
    scratch: &mut VerticalPathScratch,
) -> StateBoundary {
    diffraction::boundary_for_candidates(
        path,
        state,
        direct,
        &scratch.candidates,
        &mut scratch.blocking,
        &mut scratch.hull,
        &mut scratch.path,
    )
}

/// The candidates strictly between S and R in order of distance: the terrain points and the
/// obstacle tops, both already in order, merged (a terrain point first at one distance).
fn collect_candidates(path: &VerticalPath<'_>, out: &mut Vec<PlanePoint>) {
    out.clear();
    let length = path.receiver.0;
    let inside = |point: &&PlanePoint| point.0 > 0.0 && point.0 < length;
    let mut terrain = path.terrain_candidates.iter().filter(inside).peekable();
    let mut tops = path.obstacle_tops.iter().filter(inside).peekable();
    loop {
        let next = match (terrain.peek(), tops.peek()) {
            (Some(a), Some(b)) => {
                if b.0 < a.0 {
                    tops.next()
                } else {
                    terrain.next()
                }
            }
            (Some(_), None) => terrain.next(),
            (None, Some(_)) => tops.next(),
            (None, None) => break,
        };
        out.push(*next.expect("peeked"));
    }
    debug_assert!(out.windows(2).all(|pair| pair[0].0 <= pair[1].0));
}

#[cfg(test)]
mod iso_tr_17534_4_tests;

#[cfg(test)]
mod boundary_gain_tests;

#[cfg(test)]
mod ground_split_regression_tests;
