//! Pins of the foliage: Table A.1's three ranges and its 200 m cap, and the depth as the
//! cover-weighted metres inside the crowns, linear between samples, the downwind arc lifted out.

use super::*;
use crate::cnossos::MeteorologicalState;

/// Flat ground at 0 m under a ray of `horizontal_m`, sampled at `t`, with the cover and crown
/// height of each sample.
fn forest(horizontal_m: f64, t: &[f64], cover: &[f64], canopy_m: &[f64]) -> Profile {
    let mut profile = Profile::default();
    profile.clear(horizontal_m);
    for ((&t, &cover), &canopy) in t.iter().zip(cover).zip(canopy_m) {
        profile.push(t, 0.0, 1.0, cover, canopy);
    }
    profile
}

fn depth(profile: &Profile, state: MeteorologicalState, height_m: f64) -> f64 {
    let (source, receiver) = ((0.0, height_m), (profile.horizontal_m, height_m));
    depth_m(
        profile,
        &StateRay::between(state, source, receiver),
        source,
        receiver,
    )
}

#[test]
fn table_a1_has_nothing_under_10_m_steps_to_20_m_and_stops_growing_at_200_m() {
    assert_eq!(attenuation_db(9.99), [0.0; BANDS]);
    assert_eq!(attenuation_db(10.0), SHORT_DB);
    assert_eq!(attenuation_db(19.99), SHORT_DB);
    let at_100 = attenuation_db(100.0);
    assert!((at_100[0] - 2.0).abs() < 1e-12 && (at_100[7] - 12.0).abs() < 1e-12);
    assert_eq!(attenuation_db(200.0), attenuation_db(5000.0));
}

/// Calm air at 5 m over 20 m crowns: the whole ray; a sample whose crowns stay under the ray, or
/// whose cover is none, weighs nothing and its two intervals take half; a cover of a half halves.
#[test]
fn the_depth_weighs_each_sample_by_its_cover_inside_the_crowns() {
    let t = [0.0, 0.5, 1.0];
    let calm = MeteorologicalState::Homogeneous;
    let dense = forest(100.0, &t, &[1.0; 3], &[20.0; 3]);
    assert!((depth(&dense, calm, 5.0) - 100.0).abs() < 1e-9);
    let low_middle = forest(100.0, &t, &[1.0; 3], &[20.0, 2.0, 20.0]);
    assert!((depth(&low_middle, calm, 5.0) - 50.0).abs() < 1e-9);
    let open_end = forest(100.0, &t, &[1.0, 1.0, 0.0], &[20.0; 3]);
    assert!((depth(&open_end, calm, 5.0) - 75.0).abs() < 1e-9);
    let sparse = forest(100.0, &t, &[0.5; 3], &[20.0; 3]);
    assert!((depth(&sparse, calm, 5.0) - 50.0).abs() < 1e-9);
    assert_eq!(depth(&dense, calm, 25.0), 0.0);
}

/// Downwind the 400 m ray (Γ = 3,200 m) rises about x (400 - x) / 6,400 m over its chord: at 4 m
/// over 6 m crowns it stays inside for the first and the last 35 m only, the next 5 m step half.
#[test]
fn the_downwind_arc_rises_out_of_low_crowns() {
    let t: Vec<f64> = (0..=80).map(|step| f64::from(step) / 80.0).collect();
    let low = forest(400.0, &t, &[1.0; 81], &[6.0; 81]);
    assert!((depth(&low, MeteorologicalState::Homogeneous, 4.0) - 400.0).abs() < 1e-9);
    let downwind = depth(&low, MeteorologicalState::Favourable, 4.0);
    assert!((downwind - 2.0 * 37.5).abs() < 0.2, "{downwind}");
}
