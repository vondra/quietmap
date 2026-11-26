//! Regression checks for the visible tolerance in loudness and in decibels.

use super::*;
use physics::bands::energy;

/// The total agrees within 0.85 phon from 1.4 sone, half a phon between 0.8 and 1.4 sone, and a
/// tenth of a sone under 0.8; a layer within 2.5 phon from 1 sone and half a sone under it; an
/// undefined loudness never.
#[test]
fn a_probe_agrees_in_loudness_as_far_as_a_visitor_sees() {
    let phon = |sone: f64, phon: f64| sone * 2.0f64.powf(phon / 10.0);
    let heard = [2.0; LAYERS + 1];
    let with = |layer: usize, value: f64| {
        let mut blended = heard;
        blended[layer] = value;
        blended
    };
    let silent = [0.0; LAYERS + 1];
    let agrees = |heard: &[f64; LAYERS + 1], blended: &[f64; LAYERS + 1]| {
        loudness_visibly_agrees(heard, blended, &silent)
    };
    assert!(agrees(&heard, &with(LAYERS, phon(2.0, 0.8))));
    assert!(!agrees(&heard, &with(LAYERS, phon(2.0, 0.9))));
    assert!(agrees(&heard, &with(0, phon(2.0, 2.4))));
    assert!(!agrees(&heard, &with(0, phon(2.0, 2.6))));
    // Near the 1-sone quiet-zone boundary the total is held to half a phon.
    let edge = [1.0; LAYERS + 1];
    let with_edge = |value: f64| {
        let mut blended = edge;
        blended[LAYERS] = value;
        blended
    };
    assert!(agrees(&edge, &with_edge(phon(1.0, 0.4))));
    assert!(!agrees(&edge, &with_edge(phon(1.0, 0.6))));
    let quiet = [0.3; LAYERS + 1];
    let with = |layer: usize, value: f64| {
        let mut blended = quiet;
        blended[layer] = value;
        blended
    };
    assert!(agrees(&quiet, &with(LAYERS, 0.38)));
    assert!(!agrees(&quiet, &with(LAYERS, 0.45)));
    assert!(agrees(&quiet, &with(1, 0.75)));
    assert!(!agrees(&quiet, &with(1, 0.85)));
    assert!(!agrees(&[f64::NAN; LAYERS + 1], &silent));
    // Where an exact point the blend is made from shows, a quiet probe agrees as a visible one
    // (held to half a phon since it is under the quiet-edge ceiling).
    let mut beside_a_road = silent;
    beside_a_road[LAYERS] = 2.0;
    assert!(!loudness_visibly_agrees(
        &quiet,
        &with(LAYERS, 0.38),
        &beside_a_road
    ));
    assert!(loudness_visibly_agrees(
        &quiet,
        &with(LAYERS, phon(0.3, 0.4)),
        &beside_a_road
    ));
    assert!(loudness_visibly_agrees(&silent, &silent, &beside_a_road));
}

/// The total's Lden agrees within 0.85 dB from 32 dB and 3 dB under it; a layer's within 2.5 dB
/// from 35 dB, 6 dB from 32 dB and anyhow under it. A quiet intermittent source may pass the
/// loudness check and still change Lden.
#[test]
fn a_probe_agrees_in_decibels_as_far_as_a_visitor_sees() {
    let energies = |layer_db: f64| [energy(layer_db); LAYERS];
    let total_db = |layer_db: f64| layer_db + 10.0 * (LAYERS as f64).log10();
    let loudness = [0.01; LAYERS + 1];
    let agrees = |heard: [f64; LAYERS], blended: [f64; LAYERS]| {
        visibly_agrees(
            &(heard, loudness),
            &(blended, loudness),
            &Loudest::default(),
        )
    };
    // Six equal layers: the total 7.8 dB over each.
    assert!(total_db(30.0) > 32.0 && total_db(23.5) < 32.0);
    assert!(agrees(energies(30.0), energies(30.8)));
    assert!(!agrees(energies(30.0), energies(30.9)));
    assert!(agrees(energies(20.0), energies(22.5)));
    assert!(!agrees(energies(20.0), energies(23.5)));
    let one = |layer_db: f64| {
        let mut layers = energies(40.0);
        layers[2] = energy(layer_db);
        layers
    };
    assert!(agrees(one(40.0), one(42.4)));
    assert!(!agrees(one(40.0), one(42.6)));
    assert!(agrees(one(32.5), one(27.0)));
    assert!(!agrees(one(32.5), one(26.0)));
    assert!(agrees(one(25.0), one(31.0)));
    assert!(!agrees(one(40.0), one(f64::NAN)));
    assert!(agrees([0.0; LAYERS], [0.0; LAYERS]));
    // A total of 30 dB 1.5 dB off its blend passes as quiet, but not where the blend is made from
    // a point of 37 dB: it paints visible pixels too.
    let beside = Loudest::of([(energies(29.2), loudness)].iter());
    assert!(agrees(energies(22.0), energies(23.5)));
    assert!(!visibly_agrees(
        &(energies(22.0), loudness),
        &(energies(23.5), loudness),
        &beside
    ));
    assert!(visibly_agrees(
        &(energies(22.0), loudness),
        &(energies(22.7), loudness),
        &beside
    ));
}
