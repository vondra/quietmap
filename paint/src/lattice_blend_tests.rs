//! Regression checks for the blend between exact points: one rule along a shared edge, and the pixels left to a pass.

use super::*;

/// A pixel nearer the block's own sources than every exact point around it is left to a pass
/// though its energies lie within theirs (one point hears a louder source of the layer): what
/// the ground and the terrain make of a source nearer than any point is unknown. So is a source
/// no point heard, where its free field alone could be heard.
#[test]
fn a_pixel_nearer_own_sources_than_every_exact_point_is_left_to_a_pass() {
    let point = |own: f64, rest: f64| {
        let (mut energies, mut owns, mut field) = ([0.0; LAYERS], [0.0; LAYERS], [0.0; LAYERS]);
        (energies[0], owns[0], field[0]) = (own + rest, own, 1_000.0);
        Heard::of(&Anchor {
            paint: (energies, [1.0; LAYERS + 1]),
            own: owns,
            free: field,
        })
    };
    let blend_at = |around: [&Heard; 4], free: f64| {
        let mut field = [0.0; LAYERS];
        field[0] = free;
        blend(around, [0.25; 4], &field, &[0.3; LAYERS + 1])
    };
    let (beside, loud) = (point(800.0, 100.0), point(800.0, 50_000.0));
    let heard = [&beside, &beside, &beside, &loud];
    assert!(blend_at(heard, 1_100.0).is_some());
    assert_eq!(blend_at(heard, 3_000.0), None);
    let (deaf, loud) = (point(0.0, 100.0), point(0.0, 50_000.0));
    let unheard = [&deaf, &deaf, &deaf, &loud];
    assert!(blend_at(unheard, 1_100.0).is_some());
    assert_eq!(blend_at(unheard, 3_000.0), None);
}

/// Two cells of a lattice blend alike along the edge they share, whatever the points across it
/// hear: one of them hearing none of the block's own sources once changed the rule for its cell
/// alone, a step of a decibel along an edge where the excess falls by 6 dB.
#[test]
fn two_cells_blend_alike_along_the_edge_they_share() {
    let point = |own: f64| {
        let (mut energies, mut owns, mut field) = ([0.0; LAYERS], [0.0; LAYERS], [0.0; LAYERS]);
        (energies[0], owns[0], field[0]) = (own + 300.0, own, 8_000.0);
        Heard::of(&Anchor {
            paint: (energies, [1.0; LAYERS + 1]),
            own: owns,
            free: field,
        })
    };
    let (top, bottom, hearing, deaf) = (point(4_000.0), point(1_000.0), point(2_000.0), point(0.0));
    let mut field = [0.0; LAYERS];
    field[0] = 8_000.0;
    for weight in [0.25, 0.5, 0.75] {
        let weights = [weight, 0.0, 1.0 - weight, 0.0];
        let blend_beside = |across: &Heard| {
            blend(
                [&top, across, &bottom, across],
                weights,
                &field,
                &[0.3; LAYERS + 1],
            )
            .unwrap()
        };
        assert_eq!(blend_beside(&hearing), blend_beside(&deaf));
    }
}

/// Sources no exact point hears add nothing to a pixel, however near it stands to one of them:
/// the ratio of their free field there to the points' alone once made 35 dB of silent sources
/// (a pixel on one of two faint emitters, a point on the other, so that the pixel is no nearer).
#[test]
fn sources_no_exact_point_hears_add_nothing() {
    let point = |free: f64| {
        let (mut energies, mut field) = ([0.0; LAYERS], [0.0; LAYERS]);
        (energies[0], field[0]) = (300.0, free);
        Heard::of(&Anchor {
            paint: (energies, [1.0; LAYERS + 1]),
            own: [0.0; LAYERS],
            free: field,
        })
    };
    let (on_an_emitter, far) = (point(1e-3), point(1e-7));
    let mut field = [0.0; LAYERS];
    field[0] = 1e-3;
    let around = [&on_an_emitter, &far, &far, &far];
    let (energies, _) = blend(around, [0.1, 0.3, 0.3, 0.3], &field, &[0.3; LAYERS + 1]).unwrap();
    assert!((energies[0] - 300.0).abs() < 1e-9, "{}", energies[0]);
}
