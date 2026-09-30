//! Filter cases: telemetry sanity, the bogus tail, teleports, keepable segments.

use super::*;
use crate::aircraft::altitude::tests::sample;
use crate::aircraft::trace::SURFACE_REPORT;

fn point(timestamp: f64, lat: f32, lon: f32, altitude_ft: f32) -> TracePoint {
    sample(timestamp, lat, lon, altitude_ft, 250.0).point
}

#[test]
fn insane_points_are_rejected() {
    for bad in [f32::NAN, f32::INFINITY, -1.0, 1600.0] {
        assert!(!point_is_sane(&TracePoint {
            ground_speed_kt: bad,
            ..point(1000.0, 50.0, 14.0, 1000.0)
        }));
    }
    assert!(!point_is_sane(&point(f64::NAN, 50.0, 14.0, 1000.0)));
    assert!(!point_is_sane(&point(
        f64::from(u32::MAX) + 1.0,
        50.0,
        14.0,
        1000.0
    )));
    assert!(
        !point_is_sane(&point(0.0, 0.0, 0.0, 1000.0)),
        "no-fix sentinel"
    );
    assert!(!point_is_sane(&point(0.0, f32::NAN, 14.0, 1000.0)));
    assert!(!point_is_sane(&point(0.0, 50.0, 14.0, 80_000.0)));
    assert!(
        !point_is_sane(&point(0.0, 50.0, 14.0, f32::NAN)),
        "airborne without altitude"
    );
    let dead_sea = TracePoint {
        flags: SURFACE_REPORT,
        ..point(0.0, 32.5, 35.5, f32::NAN)
    };
    assert!(point_is_sane(&dead_sea));
}

fn heights(values: &[f32], vertical_rate_fpm: f32) -> Vec<Sample> {
    values
        .iter()
        .enumerate()
        .map(|(i, &height)| {
            let mut s = sample(
                i as f64 * 5.0,
                50.0 + 0.001 * i as f32,
                14.0 + 0.001 * i as f32,
                5000.0,
                250.0,
            );
            s.height_m = height;
            s.point.vertical_rate_fpm = vertical_rate_fpm;
            s
        })
        .collect()
}

/// The first height under -300 m cuts the tail, walking back over the negative heights before it.
#[test]
fn an_underground_tail_is_cut() {
    let mut samples = heights(&[1500.0, 1000.0, 800.0, -100.0, -500.0], 0.0);
    validate_trajectory(&mut samples);
    assert_eq!(samples.len(), 3);
}

#[test]
fn a_sustained_anomalous_descent_is_cut() {
    let mut samples = heights(&[1500.0, 1000.0, 600.0, 200.0, 100.0], -8500.0);
    validate_trajectory(&mut samples);
    assert!(samples.len() <= 1, "{}", samples.len());
}

/// A teleport is judged against the last kept point, so the point after it survives.
#[test]
fn teleports_are_dropped_against_the_last_kept_point() {
    for (end, expected) in [((50.001, 14.001), 3), ((70.001, 30.001), 2)] {
        let mut samples = vec![
            sample(0.0, 50.0, 14.0, 5000.0, 250.0),
            sample(5.0, 50.0, 14.0, 5500.0, 250.0),
            sample(7.0, 70.0, 30.0, 5500.0, 250.0),
            sample(9.0, end.0, end.1, 5500.0, 250.0),
        ];
        validate_trajectory(&mut samples);
        assert_eq!(samples.len(), expected);
        assert_eq!(samples[1].point.timestamp, 5.0);
    }
    let mut dateline = vec![
        sample(1.0, 1.0, 179.999, 10_000.0, 450.0),
        sample(6.0, 1.0, -179.999, 10_000.0, 450.0),
    ];
    validate_trajectory(&mut dateline);
    assert_eq!(dateline.len(), 2);
}

#[test]
fn keepable_segments() {
    let keep = |length, dt, a, b, speed, airframe| {
        segment_is_keepable(length, dt, a, b, speed, airframe, true)
    };
    assert!(keep(1000.0, 5.0, 100.0, 200.0, 250.0, Airframe::Jet));
    assert!(
        !keep(5.0, 1.0, 0.0, 0.0, 250.0, Airframe::Jet),
        "taxi remnant"
    );
    assert!(
        !keep(200_000.0, 30.0, 0.0, 0.0, 250.0, Airframe::Jet),
        "24,000 kt"
    );
    assert!(
        keep(200_000.0, 1800.0, 10_500.0, 10_500.0, 450.0, Airframe::Jet),
        "an oceanic hole"
    );
    assert!(!keep(1000.0, 0.0, 100.0, 200.0, 250.0, Airframe::Jet));
    assert!(!keep(1000.0, 5.0, -500.0, -400.0, 250.0, Airframe::Jet));
    assert!(
        !keep(1000.0, 5.0, 100.0, 200.0, 70.0, Airframe::Jet),
        "stalled jet"
    );
    assert!(keep(1000.0, 5.0, 100.0, 200.0, 70.0, Airframe::Propeller));
    assert!(
        !keep(1000.0, 5.0, 200.0, 7_500.0, 80.0, Airframe::Helicopter),
        "decode spike"
    );
    assert!(keep(
        1000.0,
        5.0,
        4_000.0,
        5_000.0,
        80.0,
        Airframe::Helicopter
    ));
    assert!(keep(1000.0, 5.0, 200.0, 7_500.0, 250.0, Airframe::Jet));
    assert!(
        segment_is_keepable(1000.0, 5.0, 0.0, 0.0, 5.0, Airframe::Jet, false),
        "a slow jet on the ground"
    );
    assert!(!keep(f32::NAN, 5.0, 100.0, 100.0, 250.0, Airframe::Jet));
}
