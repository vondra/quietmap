//! Regression checks for lattice coverage, exact sample reuse and carried loudness.

use super::*;
use crate::lattice_blend::{ENERGY_FLOOR, SONE_FLOOR, loudness_slopes};

fn samples(axis: [usize; 5], values: Vec<Option<Painted>>, accepted: [bool; 16]) -> QuietSamples {
    QuietSamples {
        axis,
        values,
        accepted,
        carried: [false; 16],
        slopes: [0.0; LAYERS + 1],
    }
}

/// A paint whose every energy is `energy` and every loudness `sone`.
fn even(energy: f64, sone: f64) -> Painted {
    ([energy; LAYERS], [sone; LAYERS + 1])
}

/// A field linear in the blend's domain (the logarithm over the floors) within every patch, its
/// slope changing at the block's middle lines, is blended exactly at every pixel of a block, at
/// blocks of 8 and 16: a pixel blended from a neighbouring patch's points would miss it.
#[test]
fn the_sample_grid_covers_every_pixel_without_extrapolation() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        assert_eq!(positions.len(), 41);
        let bent = |at: usize| at as f64 + 1.5 * at.saturating_sub(b / 2) as f64;
        let level = |x: usize, y: usize| (0.5 + 0.1 * bent(x) + 0.2 * bent(y)).exp();
        let values = axis
            .into_iter()
            .flat_map(|y| {
                axis.into_iter().map(move |x| {
                    let value = level(x, y);
                    Some(even(value - ENERGY_FLOOR, value - SONE_FLOOR))
                })
            })
            .collect();
        let samples = samples(axis, values, [true; 16]);
        for y in 0..b {
            for x in 0..b {
                let blended = samples.at(x, y);
                assert!((blended.1[LAYERS] / (level(x, y) - SONE_FLOOR) - 1.0).abs() < 1e-9);
                assert!((blended.0[0] / (level(x, y) - ENERGY_FLOOR) - 1.0).abs() < 1e-9);
            }
        }
    }
}

#[test]
fn rejected_patches_keep_their_unsampled_pixels() {
    let accepted = std::array::from_fn(|cell| cell == 5);
    let samples = samples([0, 4, 8, 12, 15], vec![None; 25], accepted);
    assert!(samples.accepts(4, 4));
    assert!(samples.accepts(7, 7));
    assert!(!samples.accepts(3, 4));
    assert!(!samples.accepts(8, 4));
    assert!(!samples.accepts(15, 15));
    assert!(!samples.complete());
}

#[test]
fn rejected_patches_reuse_exact_lattice_and_probe_values() {
    let mut samples = samples([0, 4, 8, 12, 15], vec![None; 41], [false; 16]);
    let (lattice, probe) = (even(1.0, 2.0), even(3.0, 4.0));
    samples.values[0] = Some(lattice);
    samples.values[25] = Some(probe);
    assert_eq!(samples.known_at(0, 0), Some(lattice));
    assert_eq!(samples.known_at(2, 2), Some(probe));
    assert_eq!(samples.known_at(1, 1), None);
    assert_eq!(samples.known_at(4, 4), None);
}

/// A probe paints as its patch paints the pixels around it: the lattice's blend in an accepted
/// patch, its own energies with the carried loudness in a patch that carries, its own paint in
/// any other.
#[test]
fn a_probe_paints_as_its_patch_paints_the_pixels_around_it() {
    let (axis, positions) = sample_positions(16);
    let mut samples = samples(
        axis,
        vec![Some(even(1000.0, 2.0)); positions.len()],
        [false; 16],
    );
    let probe = even(1100.0, 2.2);
    samples.values[25] = Some(probe);
    assert_eq!(samples.known_at(2, 2), Some(probe));
    samples.carried[0] = true;
    let carried = samples.known_at(2, 2).unwrap();
    assert_eq!(carried.0, probe.0);
    assert!((carried.1[0] - 2.0).abs() < 1e-12, "{carried:?}");
    samples.accepted[0] = true;
    let blended = samples.known_at(2, 2).unwrap();
    assert!((blended.0[0] - 1000.0).abs() < 1e-9 && (blended.1[0] - 2.0).abs() < 1e-12);
}

/// A block of 8 pixels' last interval is one pixel wide: its probe stands on a lattice line, and
/// neither blends nor carries its patch.
#[test]
fn an_edge_interval_without_an_interior_probe_keeps_its_unsampled_pixels() {
    let (axis, positions) = sample_positions(8);
    let painted = even(1.0, 1.0);
    let samples = samples(axis, vec![Some(painted); positions.len()], [false; 16]);
    assert!(samples.agrees_with_probe([2, 2], positions[25 + 10], &painted));
    assert!(samples.carries_probe([2, 2], positions[25 + 10], &painted));
    assert!(!samples.agrees_with_probe([3, 3], positions[25 + 15], &painted));
    assert!(!samples.carries_probe([3, 3], positions[25 + 15], &painted));
    assert_eq!(samples.known_at(7, 5), None);
}

/// A block whose loudness follows its energy to the power 0.4 from one lattice point to another
/// finds that slope, and a pixel of another energy within its patch's points' takes the loudness
/// of that law at its own; one over half a decibel beyond them is not carried. A probe that hears
/// the law is carried, one a phon off is not. A lattice of one energy takes Stevens' slope.
#[test]
fn a_pixels_loudness_follows_its_energy_along_the_lattices_slope() {
    let (axis, positions) = sample_positions(16);
    let law = |energy: f64| (energy + ENERGY_FLOOR).powf(0.4) - SONE_FLOOR;
    let point = |energy: f64| {
        // Six equal layers: the total's energy six times a layer's, on the same law.
        let mut paint = even(energy, law(energy));
        paint.1[LAYERS] = law(6.0 * energy);
        paint
    };
    let values: Vec<Option<Painted>> = positions
        .iter()
        .map(|&(x, y)| Some(point(1000.0 * (1.0 + x as f64 + 2.0 * y as f64))))
        .collect();
    let mut samples = samples(axis, values, [false; 16]);
    samples.slopes = loudness_slopes(&samples.values[..25]);
    assert!(
        (samples.slopes.iter()).all(|slope| (slope - 0.4).abs() < 1e-6),
        "{:?}",
        samples.slopes
    );
    samples.carried[0] = true;
    // The first patch's lattice points hear 1,000 to 13,000.
    let within = point(11_000.0);
    let carried = samples.loudness_from(2, 2, &within.0).unwrap();
    for (carried, expected) in carried.iter().zip(within.1) {
        assert!(
            (carried / expected - 1.0).abs() < 1e-6,
            "{carried} {expected}"
        );
    }
    assert!(samples.loudness_from(2, 2, &point(14_000.0).0).is_some());
    assert_eq!(samples.loudness_from(2, 2, &point(15_000.0).0), None);
    assert_eq!(samples.loudness_from(2, 2, &point(880.0).0), None);
    assert_eq!(samples.loudness_from(6, 2, &within.0), None);
    assert!(samples.carries_probe([0, 0], (2, 2), &within));
    let mut louder = within;
    louder.1[LAYERS] *= 2.0f64.powf(0.1);
    assert!(!samples.carries_probe([0, 0], (2, 2), &louder));
    let flat = vec![Some(even(1000.0, 2.0)); 25];
    assert_eq!(loudness_slopes(&flat), [0.3; LAYERS + 1]);
}
