//! Regression checks for lattice coverage, exact sample reuse and per-layer accuracy probes.

use super::*;

#[test]
fn the_sample_grid_covers_every_pixel_without_extrapolation() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        assert_eq!(positions.len(), 41);
        let values = axis
            .into_iter()
            .flat_map(|y| {
                axis.into_iter().map(move |x| {
                    let value = 1.0 + x as f64 + 2.0 * y as f64;
                    Some(([value; LAYERS], [value; LAYERS + 1]))
                })
            })
            .collect();
        let samples = QuietSamples {
            axis,
            values,
            accepted: [true; 16],
        };
        for y in 0..b {
            for x in 0..b {
                assert!(
                    (samples.at(x, y).1[LAYERS] - (1.0 + x as f64 + 2.0 * y as f64)).abs() < 1e-12
                );
            }
        }
    }
}

#[test]
fn rejected_patches_keep_their_unsampled_pixels() {
    let samples = QuietSamples {
        axis: [0, 4, 8, 12, 15],
        values: vec![None; 25],
        accepted: std::array::from_fn(|cell| cell == 5),
    };
    assert!(samples.accepts(4, 4));
    assert!(samples.accepts(7, 7));
    assert!(!samples.accepts(3, 4));
    assert!(!samples.accepts(8, 4));
    assert!(!samples.accepts(15, 15));
    assert!(!samples.complete());
}

#[test]
fn rejected_patches_reuse_exact_lattice_and_probe_values() {
    let mut samples = QuietSamples {
        axis: [0, 4, 8, 12, 15],
        values: vec![None; 41],
        accepted: [false; 16],
    };
    let lattice = ([1.0; LAYERS], [2.0; LAYERS + 1]);
    let probe = ([3.0; LAYERS], [4.0; LAYERS + 1]);
    samples.values[0] = Some(lattice);
    samples.values[25] = Some(probe);
    assert_eq!(samples.known_at(0, 0), Some(lattice));
    assert_eq!(samples.known_at(2, 2), Some(probe));
    assert_eq!(samples.known_at(1, 1), None);
    assert_eq!(samples.known_at(4, 4), None);
}

#[test]
fn an_edge_interval_without_an_interior_probe_keeps_its_unsampled_pixels() {
    let (axis, positions) = sample_positions(8);
    let heard = [1.0; LAYERS + 1];
    let samples = QuietSamples {
        axis,
        values: vec![Some(([1.0; LAYERS], heard)); positions.len()],
        accepted: [false; 16],
    };
    let painted = ([1.0; LAYERS], heard);
    assert!(samples.agrees_with_probe([2, 2], positions[25 + 10], &painted));
    assert!(!samples.agrees_with_probe([3, 3], positions[25 + 15], &painted));
    assert_eq!(samples.known_at(7, 5), None);
}

#[test]
fn a_quiet_intermittent_source_must_also_agree_in_decibels() {
    let loudness = [0.01; LAYERS + 1];
    let energies = [1000.0; LAYERS];
    let samples = QuietSamples {
        axis: [0, 4, 8, 12, 15],
        values: vec![Some((energies, loudness)); 41],
        accepted: [false; 16],
    };
    let mut heard = (energies, loudness);
    assert!(samples.agrees_with_probe([0, 0], (2, 2), &heard));
    heard.0[0] *= 10.0f64.powf(0.6 / 10.0);
    assert!(!samples.agrees_with_probe([0, 0], (2, 2), &heard));
    heard.0[0] = f64::NAN;
    assert!(!samples.agrees_with_probe([0, 0], (2, 2), &heard));
    assert!(energies_agree_within_half_decibel(
        &[0.0; LAYERS],
        &[0.0; LAYERS]
    ));
    assert!(!energies_agree_within_half_decibel(
        &[0.2; LAYERS],
        &[0.8; LAYERS]
    ));
}

#[test]
fn a_probe_checks_each_layer_and_rejects_undefined_loudness() {
    let heard = [1.0; LAYERS + 1];
    let mut blended = heard;
    blended[0] *= 2.0f64.powf(0.6 / 10.0);
    assert!(!agrees_with_interpolation(&heard, &blended));
    blended[0] = 2.0f64.powf(0.4 / 10.0);
    assert!(agrees_with_interpolation(&heard, &blended));
    assert!(!agrees_with_interpolation(
        &[f64::NAN; LAYERS + 1],
        &[0.0; LAYERS + 1]
    ));
}

#[test]
fn a_layer_under_half_a_sone_agrees_within_a_tenth_of_a_sone_but_the_total_by_half_a_phon() {
    let mut heard = [1.0; LAYERS + 1];
    heard[1] = 0.3;
    let mut blended = heard;
    blended[1] = 0.38;
    assert!(agrees_with_interpolation(&heard, &blended));
    blended[1] = 0.45;
    assert!(!agrees_with_interpolation(&heard, &blended));
    blended[1] = 0.6;
    assert!(!agrees_with_interpolation(&heard, &blended));
    let (quiet_total, mut blended_total) = ([0.3; LAYERS + 1], [0.3; LAYERS + 1]);
    blended_total[LAYERS] = 0.45;
    assert!(!agrees_with_interpolation(&quiet_total, &blended_total));
}

/// A field bilinear in each quarter passes the coarse probes, at blocks of 8 and 16, and the block
/// then blends every pixel from the coarse lattice, a probe within the tolerance taking the blend;
/// a probe over half a phon or half a decibel off, or a coarse sample unknown, sends the block to
/// the fine lattice.
#[test]
fn a_coarse_lattice_blends_a_block_only_where_its_quadrant_probes_agree() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        let middle = axis[2] as f64;
        let field = |x: usize, y: usize| {
            let (x, y) = (x as f64, y as f64);
            1.0 + 0.05 * x
                + 0.02 * y
                + if x > middle {
                    0.03 * (x - middle) * y
                } else {
                    0.0
                }
        };
        let painted = |value: f64| Some(([value; LAYERS], [value; LAYERS + 1]));
        let mut values: Vec<Option<Painted>> = vec![None; positions.len()];
        for &k in &COARSE_FIRST {
            let (x, y) = positions[k];
            values[k] = painted(field(x, y));
        }
        let (px, py) = positions[18];
        values[18] = painted(field(px, py) * 1.02);
        let samples = QuietSamples {
            axis,
            values: coarse_lattice(axis, &values).expect("a field within tolerance agrees"),
            accepted: [true; 16],
        };
        for y in 0..b {
            for x in 0..b {
                let known = samples.known_at(x, y).unwrap();
                assert!(
                    (known.1[LAYERS] - field(x, y)).abs() < 1e-12,
                    "block {b} pixel {x},{y}"
                );
            }
        }
        let mut loud = values.clone();
        loud[18] = painted(field(px, py) * 1.05);
        assert!(coarse_lattice(axis, &loud).is_none());
        let mut energetic = values.clone();
        energetic[18] = Some(([field(px, py) * 1.2; LAYERS], [field(px, py); LAYERS + 1]));
        assert!(coarse_lattice(axis, &energetic).is_none());
        let mut unknown = values;
        unknown[24] = None;
        assert!(coarse_lattice(axis, &unknown).is_none());
    }
}
