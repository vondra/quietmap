//! Regression checks for a block blended from its corners or its coarse lattice where its quadrants' probes agree.

use super::*;
use crate::lattice_blend::{ENERGY_FLOOR, SONE_FLOOR};

/// A paint whose every energy is `energy` and every loudness `sone`.
fn even(energy: f64, sone: f64) -> Painted {
    ([energy; LAYERS], [sone; LAYERS + 1])
}

/// The paint of a field's `value`: its loudness, and a thousand times it in every layer's energy,
/// over the blend's floors (a field linear in the logarithm blends exactly).
fn painted(value: f64) -> Option<Painted> {
    Some(even(1000.0 * value - ENERGY_FLOOR, value - SONE_FLOOR))
}

/// A lattice accepted whole, as a block blended from its corners or its coarse lattice is.
fn whole(axis: [usize; 5], values: Vec<Option<Painted>>) -> QuietSamples {
    QuietSamples {
        axis,
        values,
        accepted: [true; 16],
        carried: [false; 16],
        slopes: [0.0; LAYERS + 1],
    }
}

/// A field the coarse lattice blends exactly passes the coarse probes, at blocks of 8 and 16, and
/// the block then blends every pixel from the coarse lattice, a probe within the tolerance taking
/// the blend; a probe over half a phon or half a decibel off, or a coarse sample unknown, sends
/// the block to the fine lattice.
#[test]
fn a_coarse_lattice_blends_a_block_only_where_its_quadrant_probes_agree() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        let field = |x: usize, y: usize| (1.0 + 0.05 * x as f64 + 0.02 * y as f64).exp();
        let mut values: Vec<Option<Painted>> = vec![None; positions.len()];
        for &k in &COARSE_FIRST {
            let (x, y) = positions[k];
            values[k] = painted(field(x, y));
        }
        let (px, py) = positions[18];
        values[18] = painted(field(px, py) * 1.02);
        let lattice = coarse_lattice(axis, &values).expect("a field within tolerance agrees");
        let samples = whole(axis, lattice);
        for y in 0..b {
            for x in 0..b {
                let known = samples.known_at(x, y).unwrap();
                assert!(
                    (known.1[LAYERS] + SONE_FLOOR - field(x, y)).abs() < 1e-9,
                    "block {b} pixel {x},{y}"
                );
            }
        }
        let mut loud = values.clone();
        loud[18] = painted(field(px, py) * 1.05);
        assert!(coarse_lattice(axis, &loud).is_none());
        let mut energetic = values.clone();
        let value = field(px, py);
        energetic[18] = Some(even(1200.0 * value, value - SONE_FLOOR));
        assert!(coarse_lattice(axis, &energetic).is_none());
        let mut unknown = values;
        unknown[24] = None;
        assert!(coarse_lattice(axis, &unknown).is_none());
    }
}

/// A field the corners blend exactly passes the quadrants' probes, and every pixel of the block
/// then takes the corners' blend at its centre, the corners standing at the block's edges. A probe
/// over half a phon off, or one in a building, refuses the corners' blend.
#[test]
fn the_corners_blend_a_block_only_where_its_quadrant_probes_agree() {
    let b = 16;
    let (axis, positions) = sample_positions(b);
    let field = |x: f64, y: f64| (1.0 + 0.05 * x + 0.02 * y).exp();
    let side = b as f64;
    let corners = [(0.0, 0.0), (side, 0.0), (0.0, side), (side, side)]
        .map(|(x, y)| painted(field(x, y)).unwrap());
    let at_centre = |k: usize| field(positions[k].0 as f64 + 0.5, positions[k].1 as f64 + 0.5);
    let probes: Vec<Option<Painted>> = (QUADRANT_PROBES.iter())
        .map(|&k| painted(at_centre(k)))
        .collect();
    let lattice = corner_lattice(b, corners, &probes).expect("a field the corners blend agrees");
    let samples = whole(axis, lattice);
    for y in 0..b {
        for x in 0..b {
            let known = samples.known_at(x, y).unwrap();
            let expected = field(x as f64 + 0.5, y as f64 + 0.5);
            assert!(
                (known.1[LAYERS] + SONE_FLOOR - expected).abs() < 1e-9,
                "pixel {x},{y}"
            );
            assert!((known.0[2] + ENERGY_FLOOR - 1000.0 * expected).abs() < 1e-6);
        }
    }
    let mut loud = probes.clone();
    loud[3] = painted(at_centre(QUADRANT_PROBES[3]) * 1.05);
    assert!(corner_lattice(b, corners, &loud).is_none());
    let mut indoors = probes;
    indoors[0] = None;
    assert!(corner_lattice(b, corners, &indoors).is_none());
}
