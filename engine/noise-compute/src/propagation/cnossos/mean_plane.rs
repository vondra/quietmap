//! Least-squares mean ground plane of a terrain polyline (CNOSSOS-EU 2.5.2–2.5.4) and the
//! equivalent heights, projected distance and mirror images measured against it.

use super::VerticalProfile;

/// A mean ground line `z = slope·x + intercept` in the vertical propagation plane, `x` the
/// horizontal distance from the source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeanPlane {
    pub slope: f64,
    pub intercept_m: f64,
}

/// Directive (2.5.2)–(2.5.4) fit the polyline `z(x)` continuously: the line minimises
/// `∫(z(x) − a·x − b)² dx` over `[start_m, end_m]`, every stretch weighted by its length.
/// Integrating relative to `start_m` and to the first ground altitude keeps the normal
/// equations well conditioned on long rays. A range shorter than a millimetre is a point
/// and gets a level plane through its ground.
pub fn fit_mean_plane(profile: &VerticalProfile<'_>, start_m: f64, end_m: f64) -> MeanPlane {
    let reference_z = profile.ground_altitude_at(start_m);
    let length = end_m - start_m;
    if length < 1e-3 {
        return MeanPlane {
            slope: 0.0,
            intercept_m: reference_z,
        };
    }
    // ∫ z dx and ∫ x z dx over the range, x and z relative to the range start.
    let (mut integral_z, mut integral_xz) = (0.0_f64, 0.0_f64);
    let x = profile.distance_m;
    let z = profile.altitude_m;
    let mut accumulate = |x0: f64, z0: f64, x1: f64, z1: f64| {
        let (u0, u1) = (x0 - start_m, x1 - start_m);
        let (w0, w1) = (z0 - reference_z, z1 - reference_z);
        let width = u1 - u0;
        if width <= 0.0 {
            return;
        }
        integral_z += 0.5 * (w0 + w1) * width;
        // Exact ∫ u·w du for w linear between the two ends.
        integral_xz += width * (u0 * (2.0 * w0 + w1) + u1 * (w0 + 2.0 * w1)) / 6.0;
    };
    let first = x.partition_point(|&value| value <= start_m);
    let last = x.partition_point(|&value| value < end_m);
    let mut previous = (start_m, profile.ground_altitude_at(start_m));
    for index in first..last {
        accumulate(previous.0, previous.1, x[index], z[index]);
        previous = (x[index], z[index]);
    }
    accumulate(previous.0, previous.1, end_m, profile.ground_altitude_before(end_m));
    // Normal equations of the continuous fit with u ∈ [0, L]:
    //   a·L³/3 + b·L²/2 = ∫uw,   a·L²/2 + b·L = ∫w.
    let slope = 12.0 * (integral_xz - 0.5 * length * integral_z) / (length * length * length);
    let intercept_relative = integral_z / length - 0.5 * slope * length;
    MeanPlane {
        slope,
        intercept_m: reference_z + intercept_relative - slope * start_m,
    }
}

impl MeanPlane {
    #[inline]
    pub fn altitude_at(&self, x_m: f64) -> f64 {
        self.slope * x_m + self.intercept_m
    }

    /// Signed orthogonal height of a point above the plane (negative below it).
    #[inline]
    pub fn signed_height(&self, x_m: f64, z_m: f64) -> f64 {
        (z_m - self.altitude_at(x_m)) / (1.0 + self.slope * self.slope).sqrt()
    }

    /// Distance between the orthogonal projections of two points on the plane — the `dp`
    /// of (2.5.3), not the horizontal distance.
    #[inline]
    pub fn projected_distance(&self, from: (f64, f64), to: (f64, f64)) -> f64 {
        ((to.0 - from.0) + self.slope * (to.1 - from.1)).abs() / (1.0 + self.slope * self.slope).sqrt()
    }

    /// Mirror image of a point in the plane (S′, R′ of 2.5.31–2.5.32 and S*, R* of the
    /// Rayleigh criterion).
    #[inline]
    pub fn mirror(&self, point: (f64, f64)) -> (f64, f64) {
        let norm_sq = 1.0 + self.slope * self.slope;
        let offset = (point.1 - self.altitude_at(point.0)) / norm_sq;
        (point.0 + 2.0 * offset * self.slope, point.1 - 2.0 * offset)
    }
}

/// Equivalent geometry of one (sub-)path over its own mean plane: the heights of its two
/// ends (negative → 0 per 2.5.3, the sign kept for 2021/1226 point (9)(h)) and `dp`.
#[derive(Debug, Clone, Copy)]
pub struct EquivalentGeometry {
    pub plane: MeanPlane,
    pub source_side_height_m: f64,
    pub receiver_side_height_m: f64,
    pub source_side_below_plane: bool,
    pub receiver_side_below_plane: bool,
    pub projected_distance_m: f64,
}

impl EquivalentGeometry {
    pub fn over(plane: MeanPlane, from: (f64, f64), to: (f64, f64)) -> Self {
        let source_height = plane.signed_height(from.0, from.1);
        let receiver_height = plane.signed_height(to.0, to.1);
        Self {
            plane,
            source_side_height_m: source_height.max(0.0),
            receiver_side_height_m: receiver_height.max(0.0),
            source_side_below_plane: source_height < 0.0,
            receiver_side_below_plane: receiver_height < 0.0,
            projected_distance_m: plane.projected_distance(from, to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile<'a>(x: &'a [f64], z: &'a [f64], g: &'a [f64]) -> VerticalProfile<'a> {
        VerticalProfile {
            distance_m: x,
            altitude_m: z,
            ground_factor: g,
        }
    }

    /// A straight ramp is its own mean plane, over any sub-range and with any vertex split.
    #[test]
    fn a_straight_ramp_is_its_own_plane() {
        let x = [0.0, 3.0, 40.0, 100.0];
        let z = [2.0, 2.3, 6.0, 12.0];
        let g = [0.5; 4];
        for (start, end) in [(0.0, 100.0), (1.5, 70.0), (40.0, 100.0), (10.0, 10.5)] {
            let plane = fit_mean_plane(&profile(&x, &z, &g), start, end);
            assert!((plane.slope - 0.1).abs() < 1e-12, "{start}..{end}: {plane:?}");
            assert!((plane.intercept_m - 2.0).abs() < 1e-9, "{start}..{end}: {plane:?}");
        }
    }

    /// Length weighting: a long flat stretch dominates a short step even though the step
    /// carries more vertices (the unweighted fit this replaces tilted toward the vertices).
    #[test]
    fn the_fit_weights_ground_by_length_not_by_vertex() {
        let x = [0.0, 1.0, 2.0, 3.0, 4.0, 100.0];
        let z = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let g = [0.0; 6];
        let flat = fit_mean_plane(&profile(&x, &z, &g), 0.0, 100.0);
        assert_eq!(flat.slope, 0.0);
        // Closed form for a step of height h over [c, L]: ∫z = h(L − c), ∫xz = h(L² − c²)/2.
        let x = [0.0, 50.0, 50.0, 100.0];
        let z = [0.0, 0.0, 10.0, 10.0];
        let g = [0.0; 4];
        let step = fit_mean_plane(&profile(&x, &z, &g), 0.0, 100.0);
        let (l, c, h) = (100.0_f64, 50.0_f64, 10.0_f64);
        let (iz, ixz) = (h * (l - c), h * (l * l - c * c) / 2.0);
        let slope = 12.0 * (ixz - 0.5 * l * iz) / (l * l * l);
        assert!((step.slope - slope).abs() < 1e-12);
        assert!((step.intercept_m - (iz / l - 0.5 * slope * l)).abs() < 1e-9);
    }

    #[test]
    fn heights_projection_and_mirror_on_a_tilted_plane() {
        let plane = MeanPlane {
            slope: 0.5,
            intercept_m: 1.0,
        };
        let point = (2.0, 5.0);
        let height = plane.signed_height(point.0, point.1);
        assert!((height - 3.0 / 1.25_f64.sqrt()).abs() < 1e-12);
        let image = plane.mirror(point);
        assert!((plane.signed_height(image.0, image.1) + height).abs() < 1e-12);
        let midpoint = ((point.0 + image.0) / 2.0, (point.1 + image.1) / 2.0);
        assert!(plane.signed_height(midpoint.0, midpoint.1).abs() < 1e-12);
        let dp = plane.projected_distance((0.0, 1.0), (4.0, 3.0));
        assert!((dp - 20.0_f64.sqrt()).abs() < 1e-12, "{dp}");
    }
}
