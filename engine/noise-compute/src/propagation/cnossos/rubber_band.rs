//! The diffraction path of one meteorological state: the rubber band over the candidates
//! (CNOSSOS-EU 2.5.23–2.5.29 as amended by 2021/1226; ISO/TR 17534-4 §5.10–5.12) or, on an
//! unblocked path, the candidate nearest to blocking it.
//!
//! Homogeneous rays are straight. The favourable ray is the arc of radius
//! `Γ = max(1000, 8·d)` through S and R, concave toward the ground: a candidate blocks it when it
//! stands above the arc, the band is the upper hull after lowering every candidate by the arc's
//! height above the chord, and path lengths are arcs `2Γ·asin(ℓ/2Γ)` between consecutive points.

use super::ground::MeteorologicalState;

/// Favourable-ray curvature Γ = max(1000 m, 8·d) of (2.5.24) as amended.
pub const FAVOURABLE_RAY_RADIUS_MINIMUM_M: f64 = 1000.0;
pub const FAVOURABLE_RAY_RADIUS_PER_DISTANCE: f64 = 8.0;

/// A point in the vertical propagation plane: horizontal distance from the source, altitude.
pub type PlanePoint = (f64, f64);

/// The diffraction points of one state, source side first.
#[derive(Debug, Clone, Default)]
pub struct DiffractionPath {
    /// O₁…Oₙ; empty when there is no candidate at all.
    pub points: Vec<PlanePoint>,
    /// Some candidate stands above the state's line of sight (the band is taut); otherwise
    /// `points` is the single candidate nearest to blocking, subject to Rayleigh.
    pub blocked: bool,
}

/// The ray geometry of one state between S and R.
#[derive(Debug, Clone, Copy)]
pub struct StateRay {
    pub state: MeteorologicalState,
    /// Γ from the direct S–R distance; unused in the homogeneous state.
    pub radius_m: f64,
}

impl StateRay {
    pub fn between(state: MeteorologicalState, source: PlanePoint, receiver: PlanePoint) -> Self {
        let direct = distance(source, receiver);
        Self {
            state,
            radius_m: FAVOURABLE_RAY_RADIUS_MINIMUM_M.max(FAVOURABLE_RAY_RADIUS_PER_DISTANCE * direct),
        }
    }

    /// Length of the state's ray between two points: the chord or its arc.
    #[inline]
    pub fn length(&self, from: PlanePoint, to: PlanePoint) -> f64 {
        let chord = distance(from, to);
        match self.state {
            MeteorologicalState::Homogeneous => chord,
            MeteorologicalState::Favourable => {
                2.0 * self.radius_m * (chord / (2.0 * self.radius_m)).min(1.0).asin()
            }
        }
    }

    /// Path difference `Σ|S O₁ … Oₙ R| − |SR|` along the state's rays. A single point below the
    /// state's line of sight gives a negative value: homogeneous `−(SO + OR − SR)`; favourable
    /// (2.5.26) signed by the arcs themselves when the point is above the straight chord, else
    /// (2.5.27) with A where the straight chord meets the vertical through the point.
    pub fn path_difference(&self, from: PlanePoint, points: &[PlanePoint], to: PlanePoint) -> f64 {
        let Some((&first, _)) = points.split_first() else {
            return 0.0;
        };
        let last = points[points.len() - 1];
        let mut along = self.length(from, first) + self.length(last, to);
        for pair in points.windows(2) {
            along += self.length(pair[0], pair[1]);
        }
        let excess = along - self.length(from, to);
        if points.len() > 1 {
            return excess;
        }
        let chord = chord_altitude(from, to, first.0);
        match self.state {
            MeteorologicalState::Homogeneous => {
                if first.1 >= chord {
                    excess
                } else {
                    -excess
                }
            }
            MeteorologicalState::Favourable => {
                if first.1 >= chord {
                    excess
                } else {
                    let on_chord = (first.0, chord);
                    2.0 * self.length(from, on_chord) + 2.0 * self.length(on_chord, to)
                        - self.length(from, first)
                        - self.length(first, to)
                        - self.length(from, to)
                }
            }
        }
    }

    /// Height the state's ray stands above the S–R chord at horizontal distance `x`.
    fn ray_height_above_chord(&self, source: PlanePoint, receiver: PlanePoint, x: f64) -> f64 {
        match self.state {
            MeteorologicalState::Homogeneous => 0.0,
            MeteorologicalState::Favourable => {
                let horizontal = receiver.0 - source.0;
                let chord = distance(source, receiver);
                let along = (x - source.0) * chord / horizontal;
                let gamma = self.radius_m;
                let offset = along - 0.5 * chord;
                let perpendicular = along * (chord - along)
                    / ((gamma * gamma - offset * offset).max(0.0).sqrt()
                        + (gamma * gamma - 0.25 * chord * chord).sqrt());
                perpendicular * chord / horizontal
            }
        }
    }
}

#[inline]
pub fn distance(a: PlanePoint, b: PlanePoint) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

#[inline]
fn chord_altitude(from: PlanePoint, to: PlanePoint, x: f64) -> f64 {
    from.1 + (to.1 - from.1) * (x - from.0) / (to.0 - from.0)
}

/// The state's diffraction path over `candidates` (sorted by distance, strictly between S and
/// R). `lowered` is scratch.
pub fn diffraction_path(
    ray: &StateRay,
    source: PlanePoint,
    receiver: PlanePoint,
    candidates: &[PlanePoint],
    lowered: &mut Vec<(f64, f64, usize)>,
    path: &mut DiffractionPath,
) {
    path.points.clear();
    path.blocked = false;
    lowered.clear();
    for (index, &(x, z)) in candidates.iter().enumerate() {
        let z_lowered = z - ray.ray_height_above_chord(source, receiver, x);
        if z_lowered > chord_altitude(source, receiver, x) {
            lowered.push((x, z_lowered, index));
        }
    }
    if !lowered.is_empty() {
        // Upper hull of S, the blocking candidates and R (monotone chain on sorted x).
        path.blocked = true;
        let mut hull: Vec<(f64, f64, usize)> = Vec::with_capacity(lowered.len() + 2);
        let ends = [(source.0, source.1, usize::MAX), (receiver.0, receiver.1, usize::MAX)];
        for point in std::iter::once(ends[0]).chain(lowered.iter().copied()).chain(std::iter::once(ends[1])) {
            while hull.len() >= 2 {
                let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
                if (b.0 - a.0) * (point.1 - a.1) - (b.1 - a.1) * (point.0 - a.0) >= 0.0 {
                    hull.pop();
                } else {
                    break;
                }
            }
            hull.push(point);
        }
        path.points.extend(hull[1..hull.len() - 1].iter().map(|&(_, _, index)| candidates[index]));
        return;
    }
    // Unblocked: the candidate with the largest (least negative) path difference.
    let mut best: Option<(f64, PlanePoint)> = None;
    for &candidate in candidates {
        let delta = ray.path_difference(source, &[candidate], receiver);
        if best.is_none_or(|(value, _)| delta > value) {
            best = Some((delta, candidate));
        }
    }
    if let Some((_, candidate)) = best {
        path.points.push(candidate);
    }
}
