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

/// Γ of the favourable ray for the direct source–receiver distance `direct_m`.
#[inline]
pub fn favourable_ray_radius_m(direct_m: f64) -> f64 {
    FAVOURABLE_RAY_RADIUS_MINIMUM_M.max(FAVOURABLE_RAY_RADIUS_PER_DISTANCE * direct_m)
}

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
            radius_m: favourable_ray_radius_m(direct),
        }
    }

    /// Length of the state's ray between two points: the chord or its arc.
    #[inline]
    pub fn length(&self, from: PlanePoint, to: PlanePoint) -> f64 {
        let chord = distance(from, to);
        chord + self.arc_excess(chord)
    }

    /// `2Γ·asin(ℓ/2Γ) − ℓ`, the arc's excess over its chord: by its series in `x = ℓ/2Γ` up to
    /// x = 1/16 (Γ ≥ 8d holds the direct chord there; `x³/6 + 3x⁵/40 + 5x⁷/112 + 35x⁹/1152` times
    /// 2Γ, the next term below 1e-15 of the arc), by asin beyond (a leg to an image or through A
    /// can be longer than d).
    #[inline]
    fn arc_excess(&self, chord: f64) -> f64 {
        match self.state {
            MeteorologicalState::Homogeneous => 0.0,
            MeteorologicalState::Favourable => {
                let x = chord / (2.0 * self.radius_m);
                if x > 1.0 / 16.0 {
                    return 2.0 * self.radius_m * x.min(1.0).asin() - chord;
                }
                let x2 = x * x;
                2.0 * self.radius_m
                    * x
                    * x2
                    * (1.0 / 6.0 + x2 * (3.0 / 40.0 + x2 * (5.0 / 112.0 + x2 * (35.0 / 1152.0))))
            }
        }
    }

    /// Path difference `Σ|S O₁ … Oₙ R| − |SR|` along the state's rays. A single point below the
    /// state's line of sight gives a negative value: homogeneous `−(SO + OR − SR)`; favourable
    /// (2.5.26) signed by the arcs themselves when the point is above the straight chord, else
    /// (2.5.27) with A where the straight chord meets the vertical through the point.
    pub fn path_difference(&self, from: PlanePoint, points: &[PlanePoint], to: PlanePoint) -> f64 {
        let Some(&first) = points.first() else {
            return 0.0;
        };
        // The chords' excess as a sum of triangle excesses (S O1 R) + (O1 O2 R) + ..., each free of
        // the cancellation of three long lengths; the arcs add their own small excesses.
        let (mut chords, mut arcs, mut start) = (0.0, 0.0, from);
        for &point in points {
            chords += triangle_excess(start, point, to);
            arcs += self.arc_excess(distance(start, point));
            start = point;
        }
        arcs += self.arc_excess(distance(start, to)) - self.arc_excess(distance(from, to));
        let excess = chords + arcs;
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
                    // (2.5.27): 2 SA + 2 AR - SO - OR - SR with A on the chord under O, that is
                    // 2 (SA + AR - SR) - (SO + OR - SR) and the arcs' excesses. SA + AR = SR only
                    // while A lies between S and R; an image point mirrored in a tilted plane
                    // can stand beyond O, and then A lies outside.
                    let on_chord = (first.0, chord);
                    2.0 * triangle_excess(from, on_chord, to) - triangle_excess(from, first, to)
                        + 2.0 * self.arc_excess(distance(from, on_chord))
                        + 2.0 * self.arc_excess(distance(on_chord, to))
                        - self.arc_excess(distance(from, first))
                        - self.arc_excess(distance(first, to))
                        - self.arc_excess(distance(from, to))
                }
            }
        }
    }

    /// The state's ray height above the S–R chord as a function of the horizontal distance from
    /// S, its per-path constants taken once.
    fn sag(&self, source: PlanePoint, receiver: PlanePoint) -> Sag {
        let horizontal = receiver.0 - source.0;
        let chord = distance(source, receiver);
        let gamma2 = self.radius_m * self.radius_m;
        Sag {
            favourable: self.state == MeteorologicalState::Favourable,
            along_per_x: chord / horizontal,
            chord,
            gamma2,
            centre_term: (gamma2 - 0.25 * chord * chord).sqrt(),
        }
    }
}

/// The favourable ray's height above the S–R chord, its per-path constants taken once.
struct Sag {
    favourable: bool,
    along_per_x: f64,
    chord: f64,
    gamma2: f64,
    centre_term: f64,
}

impl Sag {
    #[inline]
    fn at(&self, x_from_source: f64) -> f64 {
        if !self.favourable {
            return 0.0;
        }
        let along = x_from_source * self.along_per_x;
        let offset = along - 0.5 * self.chord;
        let perpendicular = along * (self.chord - along)
            / ((self.gamma2 - offset * offset).max(0.0).sqrt() + self.centre_term);
        perpendicular * self.along_per_x
    }
}

#[inline]
pub fn distance(a: PlanePoint, b: PlanePoint) -> f64 {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    (dx * dx + dz * dz).sqrt()
}

/// `|SO| + |OR| − |SR|` without the cancellation of the three lengths: with u = O − S, v = R − O
/// and w = u + v, `(|u| + |v|)² − |w|² = 2(|u||v| − u·v)` and `|u||v| − u·v = (u×v)²/(|u||v| + u·v)`;
/// exact when O lies ahead of S toward R (u·v > 0), the direct difference otherwise (large, no
/// cancellation); the kernel's `triangle_excess`.
#[inline]
fn triangle_excess(s: PlanePoint, o: PlanePoint, r: PlanePoint) -> f64 {
    let (ux, uz, vx, vz) = (o.0 - s.0, o.1 - s.1, r.0 - o.0, r.1 - o.1);
    let (lu, lv) = ((ux * ux + uz * uz).sqrt(), (vx * vx + vz * vz).sqrt());
    let (wx, wz) = (ux + vx, uz + vz);
    let lw = (wx * wx + wz * wz).sqrt();
    let dot = ux * vx + uz * vz;
    if dot <= 0.0 {
        return lu + lv - lw;
    }
    let cross = ux * vz - uz * vx;
    2.0 * cross * cross / ((lu * lv + dot) * (lu + lv + lw))
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
    hull: &mut Vec<(f64, f64, usize)>,
    path: &mut DiffractionPath,
) {
    path.points.clear();
    path.blocked = false;
    lowered.clear();
    let sag = ray.sag(source, receiver);
    let slope = (receiver.1 - source.1) / (receiver.0 - source.0);
    for (index, &(x, z)) in candidates.iter().enumerate() {
        let z_lowered = z - sag.at(x - source.0);
        if z_lowered > source.1 + slope * (x - source.0) {
            lowered.push((x, z_lowered, index));
        }
    }
    if !lowered.is_empty() {
        // Upper hull of S, the blocking candidates and R (monotone chain on sorted x).
        path.blocked = true;
        hull.clear();
        let ends = [
            (source.0, source.1, usize::MAX),
            (receiver.0, receiver.1, usize::MAX),
        ];
        for point in std::iter::once(ends[0])
            .chain(lowered.iter().copied())
            .chain(std::iter::once(ends[1]))
        {
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
        path.points.extend(
            hull[1..hull.len() - 1]
                .iter()
                .map(|&(_, _, index)| candidates[index]),
        );
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

#[cfg(test)]
#[path = "rubber_band_tests.rs"]
mod tests;
