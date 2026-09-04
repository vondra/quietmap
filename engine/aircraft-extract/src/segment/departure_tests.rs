//! Departure classification preserves acceleration and smoothed ROCD behavior.

use super::tests::{ground_pt, pt};
use super::*;

#[test]
fn classify_is_departure_air_to_ground_transition_classifies_as_arrival() {
    // Approach decelerating 200→140 kt, then touchdown:
    // first ground point after touchdown with continued
    // deceleration 120→60 kt over the rollout. The transition
    // pair (last airborne 140 kt + first ground 120 kt) has
    // one None alt → speed branch. Lookback sees decelerating
    // trend → must NOT classify as departure.
    let mut points: Vec<TracePoint> = (0..6)
        .map(|i| {
            pt(
                i as f64 * 5.0,
                50.0 + 0.002 * i as f32,
                14.0,
                500.0 - 80.0 * i as f32,
                200.0 - 12.0 * i as f32,
                0.0,
            )
        })
        .collect();
    for i in 6..12 {
        points.push(ground_pt(
            i as f64 * 5.0,
            50.012 + 0.0008 * (i - 6) as f32,
            14.0,
            120.0 - 12.0 * (i - 6) as f32,
        ));
    }
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    assert!(
        !result[6],
        "touchdown transition must NOT classify as departure"
    );
    assert!(
        !result[8],
        "mid landing-rollout must NOT classify as departure"
    );
}

#[test]
fn classify_is_departure_ground_to_air_transition_classifies_as_departure() {
    // 6 ground samples accelerating 20→120 kt (takeoff roll),
    // then lift-off: a single transition pair (last ground point +
    // first airborne point at 140 kt / 50 ft). The transition has
    // one None alt → speed branch via the OR predicate. Lookback
    // sees +200 kt/min trend → should classify as departure.
    let mut points: Vec<TracePoint> = (0..6)
        .map(|i| {
            ground_pt(
                i as f64 * 5.0,
                50.0 + 0.0008 * i as f32,
                14.0,
                20.0 + 20.0 * i as f32,
            )
        })
        .collect();
    points.push(pt(30.0, 50.005, 14.0, 50.0, 140.0, 0.0));
    for i in 7..10 {
        points.push(pt(
            i as f64 * 5.0,
            50.005 + 0.001 * (i - 6) as f32,
            14.0,
            300.0 + 100.0 * (i - 6) as f32,
            140.0,
            0.0,
        ));
    }
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    assert!(
        result[6],
        "ground→air transition during takeoff roll must classify as departure"
    );
}

#[test]
fn classify_is_departure_ground_constant_speed_no_false_positive() {
    // Runway crossing at a constant 40 kt — high speed, zero
    // acceleration. Should NOT trigger departure classification.
    let points: Vec<TracePoint> = (0..12)
        .map(|i| ground_pt(i as f64 * 5.0, 50.0 + 0.0002 * i as f32, 14.0, 40.0))
        .collect();
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    for (i, dep) in result.iter().enumerate().skip(1) {
        assert!(
            !dep,
            "constant 40 kt at i={i} must not classify as departure"
        );
    }
}

#[test]
fn classify_is_departure_ground_taxi_burst_smoothed_out() {
    // Steady 10-kt taxi, one apron stop-start burst at i=5-6
    // (0→20 kt in 5 s = 240 kt/min per-pair, but a single pair
    // amid steady samples). Median over ±5 must reject the
    // transient and keep all pairs at NOT-departure.
    let mut points: Vec<TracePoint> = (0..12)
        .map(|i| ground_pt(i as f64 * 5.0, 50.0 + 0.0001 * i as f32, 14.0, 10.0))
        .collect();
    points[5].speed_kt = 0.0;
    points[6].speed_kt = 20.0; // single-pair burst of +240 kt/min
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    for (i, dep) in result.iter().enumerate().skip(1) {
        assert!(!dep, "taxi burst at i={i} must be median-rejected");
    }
}

#[test]
fn classify_is_departure_ground_speed_trend() {
    // Takeoff roll: speed accelerates 20→180 kt over 8 × 5 s pairs
    // (= +240 kt/min per-pair, well above 60 kt/min threshold).
    // Then 4 steady-taxi pairs at 15 kt (~0 acceleration).
    // Then 4 landing-rollout pairs decelerating 160→40 kt
    // (= −480 kt/min per-pair, well below threshold).
    let mut points: Vec<TracePoint> = (0..9)
        .map(|i| {
            ground_pt(
                i as f64 * 5.0,
                50.0 + 0.0005 * i as f32,
                14.0,
                20.0 + 20.0 * i as f32,
            )
        })
        .collect();
    for i in 9..13 {
        points.push(ground_pt(
            i as f64 * 5.0,
            50.0045 + 0.00001 * i as f32,
            14.0,
            15.0,
        ));
    }
    for i in 13..17 {
        let k = (i - 13) as f32;
        points.push(ground_pt(
            i as f64 * 5.0,
            50.005 + 0.0001 * k,
            14.0,
            160.0 - 40.0 * k,
        ));
    }
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    assert!(result[2], "early takeoff roll should classify as departure");
    assert!(result[5], "mid takeoff roll");
    assert!(!result[11], "steady taxi should not classify as departure");
    assert!(
        !result[15],
        "landing rollout should not classify as departure"
    );
}

#[test]
fn classify_is_departure_climb_descent() {
    // 6 climbing samples at +600 fpm, then 6 descending at -600 fpm.
    // Early indices = Departure; late indices = Approach.
    let mut points: Vec<TracePoint> = (0..6)
        .map(|i| {
            pt(
                i as f64 * 5.0,
                50.0 + 0.001 * i as f32,
                14.0,
                1000.0 + 50.0 * i as f32,
                200.0,
                0.0,
            )
        })
        .collect();
    for i in 6..12 {
        points.push(pt(
            i as f64 * 5.0,
            50.0 + 0.001 * i as f32,
            14.0,
            1250.0 - 50.0 * (i - 6) as f32,
            200.0,
            0.0,
        ));
    }
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    assert!(result[1], "first climbing pair");
    assert!(result[3], "mid climb");
    assert!(!result[11], "late descent");
}

#[test]
fn classify_is_departure_smoothing_resists_jitter() {
    // Steady +960 fpm climb; one anomalous baro spike at i=5.
    let n = 11;
    let mut points: Vec<TracePoint> = (0..n)
        .map(|i| {
            pt(
                i as f64 * 5.0,
                50.0 + 0.001 * i as f32,
                14.0,
                5000.0 + 80.0 * i as f32,
                250.0,
                0.0,
            )
        })
        .collect();
    points[5].alt_ft = points[4].alt_ft - 200.0;
    let alts: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let phases: Vec<Phase> = alts
        .iter()
        .map(|a| {
            if a.is_some() {
                Phase::Airborne
            } else {
                Phase::Ground
            }
        })
        .collect();
    let result = classify_is_departure_per_sample(&points, &alts, &phases);
    assert!(result[6], "median smoothing rejects single anomaly");
}
