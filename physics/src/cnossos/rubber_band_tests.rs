//! Path differences against the textbook formulas (arcs by asin, lengths summed directly) on random
//! geometries, image points beyond the ends included.

use super::*;

/// The state's ray length by asin and the path difference by direct sums: (2.5.26) and (2.5.27)
/// as printed.
fn reference(ray: &StateRay, from: PlanePoint, points: &[PlanePoint], to: PlanePoint) -> f64 {
    let arc = |a: PlanePoint, b: PlanePoint| {
        let chord = distance(a, b);
        match ray.state {
            MeteorologicalState::Homogeneous => chord,
            MeteorologicalState::Favourable => {
                2.0 * ray.radius_m * (chord / (2.0 * ray.radius_m)).min(1.0).asin()
            }
        }
    };
    let (first, last) = (points[0], points[points.len() - 1]);
    let mut along = arc(from, first) + arc(last, to);
    for pair in points.windows(2) {
        along += arc(pair[0], pair[1]);
    }
    let excess = along - arc(from, to);
    if points.len() > 1 {
        return excess;
    }
    let chord = chord_altitude(from, to, first.0);
    match ray.state {
        MeteorologicalState::Homogeneous if first.1 >= chord => excess,
        MeteorologicalState::Homogeneous => -excess,
        MeteorologicalState::Favourable if first.1 >= chord => excess,
        MeteorologicalState::Favourable => {
            let a = (first.0, chord);
            2.0 * arc(from, a) + 2.0 * arc(a, to)
                - arc(from, first)
                - arc(first, to)
                - arc(from, to)
        }
    }
}

#[test]
fn path_differences_match_the_printed_formulas_for_images_beyond_the_ends() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut unit = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for case in 0..20_000 {
        let length = 5.0 + 3_000.0 * unit();
        let (source, receiver) = ((0.0, 50.0 * unit()), (length, 50.0 * unit()));
        // An image of S or R mirrored in a tilted plane: shifted along x, possibly beyond O.
        let (shift, depth) = ((unit() - 0.5) * 0.8 * length, -20.0 * unit());
        let from = if case % 3 == 0 {
            (shift, depth)
        } else {
            source
        };
        let to = if case % 3 == 1 {
            (length + shift, depth)
        } else {
            receiver
        };
        if to.0 - from.0 < 1.0 {
            continue;
        }
        let count = 1 + (case % 3);
        // The diffraction points stand on the path between the real S and R; only the image moved.
        let points: Vec<PlanePoint> = (0..count)
            .map(|k| {
                let x = length * (k as f64 + unit()) / count as f64;
                (x, chord_altitude(from, to, x) + (unit() - 0.4) * 10.0)
            })
            .collect();
        for state in [
            MeteorologicalState::Homogeneous,
            MeteorologicalState::Favourable,
        ] {
            let ray = StateRay::between(state, source, receiver);
            let (got, want) = (
                ray.path_difference(from, &points, to),
                reference(&ray, from, &points, to),
            );
            assert!(
                (got - want).abs() < 1e-7 * (1.0 + want.abs()),
                "{case} {state:?}: {got} against {want}"
            );
        }
    }
}
