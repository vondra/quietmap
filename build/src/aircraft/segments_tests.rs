//! Segment cases: pairs, gaps, phase transitions, secondary suppression, departure classification,
//! the departure field and the helicopter descent flag.

use super::*;
use crate::aircraft::altitude::tests::sample;
use crate::aircraft::trace::{SECONDARY_PROVIDER, SURFACE_REPORT};

fn at(timestamp: f64, lat: f32, altitude_ft: f32, speed_kt: f32) -> Sample {
    sample(timestamp, lat, 14.0, altitude_ft, speed_kt)
}

fn surface(timestamp: f64, lat: f32, speed_kt: f32) -> Sample {
    let mut s = at(timestamp, lat, f32::NAN, speed_kt);
    s.point.flags = SURFACE_REPORT;
    s.altitude_m = 0.0;
    s.height_m = 0.0;
    s
}

fn phases_of(samples: &[Sample]) -> Vec<Phase> {
    samples
        .iter()
        .map(|s| {
            if s.point.is_surface_report() {
                Phase::Ground
            } else {
                Phase::Airborne
            }
        })
        .collect()
}

#[test]
fn every_joinable_pair_is_a_segment() {
    let samples: Vec<_> = (0..10)
        .map(|i| at(f64::from(i) * 5.0, 50.0 + 0.001 * i as f32, 5000.0, 250.0))
        .collect();
    let segments = build_segments(&samples, &[Phase::Airborne; 10], Airframe::Jet);
    assert_eq!(segments.len(), 9);
    assert!(
        segments
            .iter()
            .all(|s| s.phase == Phase::Airborne && s.length_m > 50.0)
    );
    let taxi = [at(0.0, 50.0, 0.0, 5.0), at(1.0, 50.000_001, 0.0, 5.0)];
    assert!(
        build_segments(&taxi, &[Phase::Ground; 2], Airframe::Jet).is_empty(),
        "10 m minimum"
    );
}

#[test]
fn gap_budgets_follow_the_more_permissive_phase() {
    let pair = |dt: f64, a: Phase, b: Phase| {
        let samples = [
            at(0.0, 50.0, 30_000.0, 450.0),
            at(dt, 50.4, 20_000.0, 450.0),
        ];
        build_segments(&samples, &[a, b], Airframe::Jet).len()
    };
    assert_eq!(pair(200.0, Phase::Airborne, Phase::Airborne), 0);
    assert_eq!(pair(200.0, Phase::Cruise, Phase::Cruise), 1);
    assert_eq!(
        pair(1800.0, Phase::Cruise, Phase::Airborne),
        1,
        "a cruise dropout reappearing lower"
    );
    for (a, b) in [
        (Phase::Cruise, Phase::Ground),
        (Phase::Ground, Phase::Cruise),
    ] {
        assert_eq!(
            pair(10.0, a, b),
            0,
            "no observed climb between cruise and ground"
        );
    }
}

/// The flare pair is airborne; its ground end keeps the airborne pressure altitude (v16) and sits
/// on the terrain above EGM2008.
#[test]
fn a_flare_is_airborne_and_its_ground_end_is_on_the_terrain() {
    let samples = [at(0.0, 50.0, 1_300.0, 130.0), surface(10.0, 50.001, 120.0)];
    let segments = build_segments(&samples, &[Phase::Airborne, Phase::Ground], Airframe::Jet);
    assert_eq!(segments.len(), 1);
    let s = &segments[0];
    assert_eq!(s.phase, Phase::Airborne);
    assert_eq!(s.end_barometric_m, s.start_barometric_m);
    assert_eq!(s.end_altitude_m, 0.0);
    assert_eq!(s.flags & ON_GROUND, 0);
}

/// An airborne end 100 m under the terrain drops; -30 m keeps; ground pairs pass.
#[test]
fn the_airborne_height_gate_is_minus_30_m() {
    let pair = |height_m: f32, phase: Phase| {
        let mut samples = [
            at(0.0, 50.0, 1312.0, 250.0),
            at(10.0, 50.001, 1312.0, 250.0),
        ];
        for s in &mut samples {
            s.height_m = height_m;
        }
        pair_phase(&samples, &[phase; 2], Airframe::Jet, 0, 1)
    };
    assert_eq!(pair(-100.0, Phase::Airborne), None);
    assert_eq!(pair(-30.0, Phase::Airborne), Some(Phase::Airborne));
    assert_eq!(pair(-100.0, Phase::Ground), Some(Phase::Ground));
}

/// A secondary sample in a primary gap no pair bridges is coverage; inside a joinable primary pair
/// it goes, and the pair stays on the baseline estimator.
#[test]
fn secondary_samples_survive_only_in_real_gaps() {
    let three = |times: [f64; 3], altitude_ft: f32, height_m: f32, phase: Phase| {
        let mut samples: Vec<_> = times
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let mut s = at(t, 50.0, altitude_ft, 450.0);
                s.point.lon = 14.0 + 0.02 * i as f32;
                s.height_m = height_m;
                s
            })
            .collect();
        samples[1].point.flags = SECONDARY_PROVIDER;
        let mut phases = vec![phase; 3];
        suppress_covered_secondary(&mut samples, &mut phases, Airframe::Jet);
        build_segments(&samples, &phases, Airframe::Jet)
    };
    let gap = three([0.0, 100.0, 200.0], 28_000.0, 6534.0, Phase::Airborne);
    assert_eq!(gap.len(), 2);
    assert!(gap.iter().all(|s| s.flags & SECONDARY_ONLY != 0));
    let joined = three([0.0, 100.0, 200.0], 24_300.0, 7350.0, Phase::Cruise);
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].flags & SECONDARY_ONLY, 0);
}

/// dev4's departure cases: a takeoff roll accelerates, a rollout and taxi do not, a climb departs,
/// a descent approaches, one baro spike is smoothed out.
#[test]
fn departures_follow_the_smoothed_acceleration_and_climb() {
    let mut roll: Vec<_> = (0..6)
        .map(|i| {
            surface(
                f64::from(i) * 5.0,
                50.0 + 0.0008 * i as f32,
                20.0 + 20.0 * i as f32,
            )
        })
        .collect();
    roll.push(at(30.0, 50.005, 50.0, 140.0));
    roll.extend((7..10).map(|i| {
        at(
            f64::from(i) * 5.0,
            50.005 + 0.001 * (i - 6) as f32,
            300.0 + 100.0 * (i - 6) as f32,
            140.0,
        )
    }));
    assert!(
        departures(&roll, &phases_of(&roll))[6],
        "lift-off during the roll departs"
    );
    let mut landing: Vec<_> = (0..6)
        .map(|i| {
            at(
                f64::from(i) * 5.0,
                50.0 + 0.002 * i as f32,
                500.0 - 80.0 * i as f32,
                200.0 - 12.0 * i as f32,
            )
        })
        .collect();
    landing.extend((6..12).map(|i| {
        surface(
            f64::from(i) * 5.0,
            50.012 + 0.0008 * (i - 6) as f32,
            120.0 - 12.0 * (i - 6) as f32,
        )
    }));
    let departure = departures(&landing, &phases_of(&landing));
    assert!(!departure[6] && !departure[8]);
    let constant: Vec<_> = (0..12)
        .map(|i| surface(f64::from(i) * 5.0, 50.0 + 0.0002 * i as f32, 40.0))
        .collect();
    assert!(
        !departures(&constant, &phases_of(&constant))
            .iter()
            .any(|&d| d)
    );
    let mut up_down: Vec<_> = (0..6)
        .map(|i| at(f64::from(i) * 5.0, 50.0, 1000.0 + 50.0 * i as f32, 200.0))
        .collect();
    up_down.extend((6..12).map(|i| {
        at(
            f64::from(i) * 5.0,
            50.0,
            1250.0 - 50.0 * (i - 6) as f32,
            200.0,
        )
    }));
    let departure = departures(&up_down, &phases_of(&up_down));
    assert!(departure[1] && departure[3] && !departure[11]);
    let mut jitter: Vec<_> = (0..11)
        .map(|i| at(f64::from(i) * 5.0, 50.0, 5000.0 + 80.0 * i as f32, 250.0))
        .collect();
    jitter[5].point.altitude_ft = jitter[4].point.altitude_ft - 200.0;
    assert!(departures(&jitter, &phases_of(&jitter))[6]);
}

/// PLAN-z13 fix: the field is the terrain under the takeoff roll, found from the lift-off; a
/// flight first seen airborne has none.
#[test]
fn the_departure_field_is_the_terrain_under_the_roll() {
    let mut samples = vec![
        surface(0.0, 50.0, 3.0),
        surface(60.0, 50.001, 8.0),
        surface(80.0, 50.002, 90.0),
    ];
    samples.push(at(90.0, 50.004, 1200.0, 150.0));
    for (s, terrain) in samples.iter_mut().zip([371.0, 372.5, 380.0, 385.0]) {
        s.terrain_m = terrain;
    }
    let on_ground: Vec<bool> = samples
        .iter()
        .map(|s| s.point.is_surface_report())
        .collect();
    assert_eq!(departure_field_m(&samples, &on_ground), 372.5);
    assert!(departure_field_m(&samples[3..], &on_ground[3..]).is_nan());
}

/// PLAN-z13 fix: a helicopter's descent is its smoothed rate, not one pair's altitude loss: a
/// 600 ft/min descent is flagged at 1 s and at 10 s cadence alike, level flight never.
#[test]
fn a_helicopter_descent_does_not_depend_on_the_cadence() {
    for cadence_s in [1.0, 2.0, 5.0, 10.0] {
        let descent: Vec<_> = (0..40)
            .map(|i| {
                let t = f64::from(i) * cadence_s;
                let altitude = 2000.0 - (600.0 * t / 60.0 / 25.0).floor() as f32 * 25.0;
                at(t, 50.0 + 0.0002 * i as f32, altitude, 80.0)
            })
            .collect();
        let segments = build_segments(&descent, &phases_of(&descent), Airframe::Helicopter);
        let flagged = segments
            .iter()
            .filter(|s| s.flags & HELICOPTER_DESCENT != 0)
            .count();
        assert!(
            flagged * 10 >= segments.len() * 9,
            "cadence {cadence_s} s: {flagged} of {}",
            segments.len()
        );
        let jet = build_segments(&descent, &phases_of(&descent), Airframe::Jet);
        assert!(jet.iter().all(|s| s.flags & HELICOPTER_DESCENT == 0));
    }
    let level: Vec<_> = (0..40)
        .map(|i| {
            at(
                f64::from(i) * 2.0,
                50.0 + 0.0002 * i as f32,
                if i % 7 == 3 { 2025.0 } else { 2000.0 },
                80.0,
            )
        })
        .collect();
    let segments = build_segments(&level, &phases_of(&level), Airframe::Helicopter);
    assert!(segments.iter().all(|s| s.flags & HELICOPTER_DESCENT == 0));
}

/// A slow climb (800 ft/min) reported every second in 25 ft steps is a departure: most per-step
/// rates are zero, so dev4's median of them read the climb as level flight.
#[test]
fn a_slow_climb_at_one_second_cadence_departs() {
    let climb: Vec<_> = (0..60)
        .map(|i| {
            let feet = 3_000.0 + (f64::from(i) * 800.0 / 60.0 / 25.0).floor() * 25.0;
            at(f64::from(i), 50.0 + 0.0001 * i as f32, feet as f32, 180.0)
        })
        .collect();
    let departure = departures(&climb, &phases_of(&climb));
    assert!(
        departure[20..40].iter().all(|&d| d),
        "{:?}",
        &departure[20..40]
    );
}
