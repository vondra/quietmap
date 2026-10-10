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

/// Codex Astra's example (2026-10-10): the Γ-arc from O1 to R passes 62.319 m above x = 600, so
/// O2 at 62.2 m does not diffract downwind; the straight hull of lowered points kept it.
#[test]
fn the_downwind_band_is_made_of_arcs() {
    let (source, receiver) = ((0.0, 0.0), (1_000.0, 0.0));
    let ray = StateRay {
        state: MeteorologicalState::Favourable,
        radius_m: 8_000.0,
    };
    let candidates = [(250.0, 100.0), (600.0, 62.2)];
    let (mut blocking, mut hull, mut path) = (Vec::new(), Vec::new(), DiffractionPath::default());
    diffraction_path(
        &ray,
        source,
        receiver,
        &candidates,
        &mut blocking,
        &mut hull,
        &mut path,
    );
    assert!(path.blocked);
    assert_eq!(path.points, vec![(250.0, 100.0)]);
    // 0.3 m higher, O2 stands above that arc and joins the band.
    let candidates = [(250.0, 100.0), (600.0, 62.5)];
    diffraction_path(
        &ray,
        source,
        receiver,
        &candidates,
        &mut blocking,
        &mut hull,
        &mut path,
    );
    assert_eq!(path.points, vec![(250.0, 100.0), (600.0, 62.5)]);
}

/// On random profiles each point of the band stands above the state's ray between its
/// neighbours on the band, and every blocking candidate left out stands on or under the ray
/// between the band's points around it.
#[test]
fn the_band_is_the_upper_envelope_of_the_states_rays() {
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut unit = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let under = |ray: &StateRay, a: PlanePoint, b: PlanePoint, p: PlanePoint| {
        p.1 <= chord_altitude(a, b, p.0) + ray.sag(a, b).at(p.0 - a.0) + 1e-9
    };
    for case in 0..3_000 {
        let length = 50.0 + 4_000.0 * unit();
        let (source, receiver) = ((0.0, 30.0 * unit()), (length, 30.0 * unit()));
        let count = 1 + case % 12;
        let mut candidates: Vec<PlanePoint> = (0..count)
            .map(|_| {
                (
                    length * (0.01 + 0.98 * unit()),
                    60.0 * unit() + 0.02 * length * unit(),
                )
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for state in [
            MeteorologicalState::Homogeneous,
            MeteorologicalState::Favourable,
        ] {
            let ray = StateRay::between(state, source, receiver);
            let (mut blocking, mut hull, mut path) =
                (Vec::new(), Vec::new(), DiffractionPath::default());
            diffraction_path(
                &ray,
                source,
                receiver,
                &candidates,
                &mut blocking,
                &mut hull,
                &mut path,
            );
            if !path.blocked {
                continue;
            }
            let band: Vec<PlanePoint> = std::iter::once(source)
                .chain(path.points.iter().copied())
                .chain(std::iter::once(receiver))
                .collect();
            for triple in band.windows(3) {
                assert!(
                    !under(&ray, triple[0], triple[2], triple[1]),
                    "{case} {state:?}: a band point under its neighbours' ray"
                );
            }
            for &(x, z, _) in &blocking {
                if band.contains(&(x, z)) {
                    continue;
                }
                let next = band.iter().position(|p| p.0 > x).unwrap();
                assert!(
                    under(&ray, band[next - 1], band[next], (x, z)),
                    "{case} {state:?}: a blocking point above the band"
                );
            }
        }
    }
}
