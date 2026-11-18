//! Regression checks for lattice coverage, exact sample reuse, the own sources' free field and carried loudness.

use super::*;
use crate::lattice_blend::{ENERGY_FLOOR, SONE_FLOOR};

/// A paint whose every energy is `energy` and every loudness `sone`.
fn even(energy: f64, sone: f64) -> Painted {
    ([energy; LAYERS], [sone; LAYERS + 1])
}

/// An exact point of a block without own sources.
fn anchor(paint: Painted) -> Option<Anchor> {
    Some(Anchor {
        paint,
        own: [0.0; LAYERS],
        free: [0.0; LAYERS],
    })
}

/// No own source: no free field at any pixel.
fn no_own(_: usize, _: usize) -> Option<[f64; LAYERS]> {
    Some([0.0; LAYERS])
}

/// A paint following one law from its energy (in the first layer alone) to its loudness: the
/// loudness the lattice's slope carries exactly.
fn on_the_law(energy: f64) -> Painted {
    let mut energies = [0.0; LAYERS];
    energies[0] = energy;
    let sone = (energy + ENERGY_FLOOR).powf(0.4) - SONE_FLOOR;
    let mut ndens = [0.0; LAYERS + 1];
    (ndens[0], ndens[LAYERS]) = (sone, sone);
    (energies, ndens)
}

/// A field linear in the blend's domain (the logarithm over the floors) within every patch, its
/// slope changing at every lattice line, is blended exactly at every pixel of a block, at blocks
/// of 8 and 16: a pixel blended from a neighbouring patch's points would miss it.
#[test]
fn the_sample_grid_covers_every_pixel_without_extrapolation() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        assert_eq!(positions.len(), 41);
        // A slope of its own between each two lattice lines.
        let bent = |at: usize| -> f64 {
            (0..4)
                .map(|k| {
                    [0.1, 0.25, 0.05, 0.3][k] * (at.clamp(axis[k], axis[k + 1]) - axis[k]) as f64
                })
                .sum()
        };
        let level = |x: usize, y: usize| (0.5 + bent(x) + 2.0 * bent(y)).exp();
        let painted = |x: usize, y: usize| {
            let mut paint = even(0.0, level(x, y) - SONE_FLOOR);
            paint.0[0] = level(x, y) - ENERGY_FLOOR;
            paint
        };
        let anchors: Vec<Option<Anchor>> = (axis.into_iter())
            .flat_map(|y| axis.into_iter().map(move |x| anchor(painted(x, y))))
            .collect();
        let lines = axis.map(|at| at as f64);
        let lattice = Lattice::new(&lines, &anchors);
        for y in 0..b {
            for x in 0..b {
                let blended = lattice.blend_at(x, y, &[0.0; LAYERS]).unwrap();
                assert!((blended.1[LAYERS] / (level(x, y) - SONE_FLOOR) - 1.0).abs() < 1e-9);
                assert!((blended.0[0] / (level(x, y) - ENERGY_FLOOR) - 1.0).abs() < 1e-9);
            }
        }
    }
}

/// A patch whose probe hears a phon more than the lattice blends keeps its samples and leaves its
/// other pixels to a pass; so does one a screening edge crosses though its probe agrees, and that
/// one carries its lattice's loudness. The patches beside it blend.
#[test]
fn a_refused_patch_keeps_its_samples_and_leaves_its_other_pixels_to_a_pass() {
    let (_, positions) = sample_positions(16);
    let heard = even(1000.0, 2.0);
    let mut samples = vec![anchor(heard); positions.len()];
    let louder = even(1000.0, 2.0 * 2.0f64.powf(0.1));
    samples[25 + 5] = anchor(louder);
    let block = QuietSamples::patched(16, &samples, &|_| true, &no_own);
    assert!(!block.complete() && block.paints_a_pixel());
    assert_eq!(block.patches(), (15, 0));
    assert_eq!(block.known_at(4, 4), Some(heard));
    assert_eq!(block.known_at(6, 6), Some(louder));
    assert_eq!(block.known_at(5, 5), None);
    assert_eq!(block.known_at(7, 7), None);
    for (x, y) in [(3, 4), (8, 4), (5, 9), (15, 15)] {
        let blended = block.known_at(x, y).unwrap();
        assert!((blended.0[0] - 1000.0).abs() < 1e-9 && (blended.1[0] - 2.0).abs() < 1e-12);
    }
    let even_ground = vec![anchor(heard); positions.len()];
    let edged = QuietSamples::patched(16, &even_ground, &|cell| cell != 5, &no_own);
    assert_eq!(edged.patches(), (15, 1));
    assert_eq!(edged.known_at(5, 5), None);
    let probe = edged.known_at(6, 6).unwrap();
    assert!(probe.0 == heard.0 && (probe.1[LAYERS] - 2.0).abs() < 1e-12);
    assert!(edged.loudness_from(5, 5, &heard.0).is_some());
    assert!(QuietSamples::patched(16, &even_ground, &|_| true, &no_own).complete());
}

/// A probe paints as its patch paints the pixels around it: the lattice's blend in a blended
/// patch; in a refused patch that carries, its own energies with the lattice's loudness moved to
/// them along the block's slope; its own paint in any other.
#[test]
fn a_probe_paints_as_its_patch_paints_the_pixels_around_it() {
    let (_, positions) = sample_positions(16);
    let mut samples: Vec<Option<Anchor>> = (positions.iter())
        .map(|&(x, y)| anchor(on_the_law(1000.0 * (1.0 + x as f64 + 2.0 * y as f64))))
        .collect();
    // The first patch's lattice points hear 1,000 to 13,000 and blend 4,919 at its probe, which
    // hears 11,000 on their law: refused, and carried.
    let mut probe = on_the_law(11_000.0);
    probe.1[LAYERS] *= 1.01;
    samples[25] = anchor(probe);
    let block = QuietSamples::patched(16, &samples, &|_| true, &no_own);
    let painted = block.known_at(2, 2).unwrap();
    assert_eq!(painted.0, probe.0);
    assert!((painted.1[LAYERS] / on_the_law(11_000.0).1[LAYERS] - 1.0).abs() < 1e-6);
    assert_eq!(block.known_at(1, 1), None);
    // Half a phon and more off the law it is not carried, and keeps its own paint.
    probe.1[LAYERS] *= 2.0f64.powf(0.1);
    samples[25] = anchor(probe);
    let block = QuietSamples::patched(16, &samples, &|_| true, &no_own);
    assert_eq!(block.known_at(2, 2), Some(probe));
    assert_eq!(block.loudness_from(1, 1, &probe.0), None);
    // A probe that agrees with the blend takes the blend.
    samples[25] = anchor(on_the_law(5_000.0));
    let block = QuietSamples::patched(16, &samples, &|_| true, &no_own);
    let blended = block.known_at(2, 2).unwrap();
    assert!((blended.0[0] / 4_918.7 - 1.0).abs() < 1e-4, "{blended:?}");
}

/// A block of 8 pixels' last interval is one pixel wide: its probe stands on a lattice line, and
/// neither blends nor carries its patch.
#[test]
fn an_edge_interval_without_an_interior_probe_keeps_its_unsampled_pixels() {
    let (_, positions) = sample_positions(8);
    let painted = even(1.0, 1.0);
    let samples = vec![anchor(painted); positions.len()];
    let block = QuietSamples::patched(8, &samples, &|_| true, &no_own);
    assert_eq!(block.patches(), (9, 0));
    assert!(block.known_at(5, 5).is_some() && block.known_at(3, 5).is_some());
    assert_eq!(block.known_at(6, 5), Some(painted));
    assert_eq!(block.known_at(7, 5), None);
    assert_eq!(block.loudness_from(7, 5, &painted.0), None);
}

/// A block whose loudness follows its energy to the power 0.4 from one lattice point to another
/// finds that slope, and a pixel of another energy within its refused patch's points' takes the
/// loudness of that law at its own; one over half a decibel beyond them is not carried, nor one
/// of a patch that blends.
#[test]
fn a_pixels_loudness_follows_its_energy_along_the_lattices_slope() {
    let (_, positions) = sample_positions(16);
    let mut samples: Vec<Option<Anchor>> = (positions.iter())
        .map(|&(x, y)| anchor(on_the_law(1000.0 * (1.0 + x as f64 + 2.0 * y as f64))))
        .collect();
    samples[25] = anchor(on_the_law(11_000.0));
    let block = QuietSamples::patched(16, &samples, &|_| true, &no_own);
    assert!(block.patches().1 >= 1);
    // The first patch's lattice points hear 1,000 to 13,000.
    let within = on_the_law(11_000.0);
    let carried = block.loudness_from(1, 3, &within.0).unwrap();
    for (carried, expected) in carried.iter().zip(within.1) {
        assert!(
            (carried - expected).abs() <= 1e-6 * expected,
            "{carried} {expected}"
        );
    }
    assert!(block.loudness_from(1, 3, &on_the_law(14_000.0).0).is_some());
    assert_eq!(block.loudness_from(1, 3, &on_the_law(15_000.0).0), None);
    assert_eq!(block.loudness_from(1, 3, &on_the_law(880.0).0), None);
    assert_eq!(block.loudness_from(9, 9, &on_the_law(40_000.0).0), None);
}

/// A road beside a block: each exact point hears the road's free field there times an excess
/// that changes slowly, over a steady rest. A pixel between them takes the road's free field at
/// itself times the excess blended, and the rest blended: exact to the blend's floor where the
/// excess is linear in the logarithm, where the points' energies blended are over a decibel off.
#[test]
fn a_blocks_own_sources_blend_by_their_free_field() {
    let b = 16.0;
    let lines = [-0.5, b - 0.5];
    // The road runs north four pixels west of the block's first pixels.
    let free = |x: f64| 4.0e5 / (x + 4.0);
    let excess = |x: f64, y: f64| (-0.02 * x + 0.01 * y).exp();
    let (rest, railway) = (500.0, 2_000.0);
    let road = |x: f64, y: f64| free(x) * excess(x, y);
    let exact = |x: f64, y: f64| road(x, y) + rest;
    let anchors: Vec<Option<Anchor>> = [
        (-0.5, -0.5),
        (b - 0.5, -0.5),
        (-0.5, b - 0.5),
        (b - 0.5, b - 0.5),
    ]
    .into_iter()
    .map(|(x, y)| {
        let mut paint = on_the_law(exact(x, y));
        paint.0[1] = railway;
        let (mut own, mut field) = ([0.0; LAYERS], [0.0; LAYERS]);
        (own[0], field[0]) = (road(x, y), free(x));
        Some(Anchor {
            paint,
            own,
            free: field,
        })
    })
    .collect();
    let lattice = Lattice::new(&lines, &anchors);
    let field_at = |x: usize| {
        let mut field = [0.0; LAYERS];
        field[0] = free(x as f64);
        field
    };
    for (x, y) in [(0, 0), (7, 3), (8, 12), (15, 15)] {
        let blended = lattice.blend_at(x, y, &field_at(x)).unwrap();
        let expected = exact(x as f64, y as f64);
        assert!((blended.0[0] / expected - 1.0).abs() < 1e-4, "{x},{y}");
        assert!((blended.0[1] / railway - 1.0).abs() < 1e-9);
    }
    // The same points without the road told apart: their energies blended read the middle loud.
    let unaware: Vec<Option<Anchor>> = (anchors.iter().flatten())
        .map(|point| anchor(point.paint))
        .collect();
    let plain = Lattice::new(&lines, &unaware).blend_at(7, 7, &[0.0; LAYERS]);
    assert!(plain.unwrap().0[0] > 1.25 * exact(7.0, 7.0));
}

/// Loudness is carried only where each layer a visitor could see, and the total, lie within the
/// lattice points' range widened by half a decibel: a layer of 20 dB or more beyond it refuses
/// the carry though the total lies within, a fainter layer does not.
#[test]
fn loudness_is_carried_only_within_the_lattice_points_energies_layer_by_layer() {
    let (_, positions) = sample_positions(16);
    let layers = |road: f64, railway: f64| {
        let mut energies = [0.0; LAYERS];
        (energies[0], energies[1]) = (road, railway);
        energies
    };
    let point = |road: f64, railway: f64| anchor((layers(road, railway), [2.0; LAYERS + 1]));
    let samples = vec![point(10_000.0, 50.0); positions.len()];
    // A screening edge crosses the first patch: refused, and carried.
    let carrying = QuietSamples::patched(16, &samples, &|cell| cell != 0, &no_own);
    assert_eq!(carrying.patches(), (15, 1));
    assert!(
        carrying
            .loudness_from(1, 1, &layers(10_500.0, 50.0))
            .is_some()
    );
    // The railway under 20 dB may differ: no visitor sees it.
    assert!(
        carrying
            .loudness_from(1, 1, &layers(10_000.0, 90.0))
            .is_some()
    );
    // At 20 dB and more it must lie within its own range, though the total does.
    assert_eq!(carrying.loudness_from(1, 1, &layers(10_000.0, 150.0)), None);
    assert_eq!(carrying.loudness_from(1, 1, &layers(12_000.0, 50.0)), None);
    assert_eq!(carrying.loudness_from(1, 1, &layers(8_000.0, 50.0)), None);
}
