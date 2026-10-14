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
    assert!(samples.agrees_with_probe([2, 2], positions[25 + 10], &heard));
    assert!(!samples.agrees_with_probe([3, 3], positions[25 + 15], &heard));
    assert_eq!(samples.known_at(7, 5), None);
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
