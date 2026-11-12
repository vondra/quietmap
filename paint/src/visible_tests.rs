//! Regression checks for the visible tolerance in loudness and in decibels.

use super::*;
use physics::bands::energy;

/// The total agrees within half a phon from 0.8 sone and a tenth of a sone under it; a layer
/// within a phon from 1 sone and a fifth of a sone under it; an undefined loudness never.
#[test]
fn a_probe_agrees_in_loudness_as_far_as_a_visitor_sees() {
    let phon = |sone: f64, phon: f64| sone * 2.0f64.powf(phon / 10.0);
    let heard = [2.0; LAYERS + 1];
    let with = |layer: usize, value: f64| {
        let mut blended = heard;
        blended[layer] = value;
        blended
    };
    assert!(loudness_visibly_agrees(
        &heard,
        &with(LAYERS, phon(2.0, 0.4))
    ));
    assert!(!loudness_visibly_agrees(
        &heard,
        &with(LAYERS, phon(2.0, 0.6))
    ));
    assert!(loudness_visibly_agrees(&heard, &with(0, phon(2.0, 0.9))));
    assert!(!loudness_visibly_agrees(&heard, &with(0, phon(2.0, 1.1))));
    let quiet = [0.3; LAYERS + 1];
    let with = |layer: usize, value: f64| {
        let mut blended = quiet;
        blended[layer] = value;
        blended
    };
    assert!(loudness_visibly_agrees(&quiet, &with(LAYERS, 0.38)));
    assert!(!loudness_visibly_agrees(&quiet, &with(LAYERS, 0.45)));
    assert!(loudness_visibly_agrees(&quiet, &with(1, 0.45)));
    assert!(!loudness_visibly_agrees(&quiet, &with(1, 0.55)));
    assert!(!loudness_visibly_agrees(
        &[f64::NAN; LAYERS + 1],
        &[0.0; LAYERS + 1]
    ));
}

/// The total's Lden agrees within half a decibel from 32 dB and two under it; a layer's within
/// one from 35 dB, three from 20 dB and anyhow under it. A quiet intermittent source may pass the
/// loudness check and still change Lden.
#[test]
fn a_probe_agrees_in_decibels_as_far_as_a_visitor_sees() {
    let energies = |layer_db: f64| [energy(layer_db); LAYERS];
    let total_db = |layer_db: f64| layer_db + 10.0 * (LAYERS as f64).log10();
    let loudness = [0.01; LAYERS + 1];
    let agrees = |heard: [f64; LAYERS], blended: [f64; LAYERS]| {
        visibly_agrees(&(heard, loudness), &(blended, loudness))
    };
    // Six equal layers: the total 7.8 dB over each.
    assert!(total_db(30.0) > 32.0 && total_db(22.0) < 32.0);
    assert!(agrees(energies(30.0), energies(30.4)));
    assert!(!agrees(energies(30.0), energies(30.6)));
    assert!(agrees(energies(22.0), energies(23.5)));
    assert!(!agrees(energies(22.0), energies(24.5)));
    let one = |layer_db: f64| {
        let mut layers = energies(40.0);
        layers[2] = energy(layer_db);
        layers
    };
    assert!(agrees(one(40.0), one(40.9)));
    assert!(!agrees(one(40.0), one(41.1)));
    assert!(agrees(one(25.0), one(27.5)));
    assert!(!agrees(one(25.0), one(28.5)));
    assert!(agrees(one(5.0), one(15.0)));
    assert!(!agrees(one(40.0), one(f64::NAN)));
    assert!(agrees([0.0; LAYERS], [0.0; LAYERS]));
}
