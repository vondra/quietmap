//! Thrust: dev4's Doc 29 pilot values, the routing of the power bracket and the domain guard.

use super::*;
use crate::doc29::npd::{class_anchor, read_npd};
use crate::doc29::profiles_generated::{CLASS_NAMES, noise_class_of, profile_idx};
use crate::doc29::segment::{AircraftType, SegmentEmission};

fn class_of(designator: &str) -> usize {
    usize::from(noise_class_of(profile_idx(designator)))
}

fn climbing(
    pressure_altitude_m: f64,
    height_above_field_m: f64,
    speed_kt: f64,
    climb_sine: f64,
) -> SegmentFlight {
    SegmentFlight {
        departure: true,
        on_ground: false,
        speed_kt,
        pressure_altitude_m,
        climb_sine,
        height_above_field_m,
    }
}

/// ANP DEFAULT stage 1 step 6 (climb 2,040 -> 3,000 ft AFE) from a 355.5 m field: MaxClimb at CAS
/// 204.8 kt gives Fn/delta 18,093 lb, on the 16,000/19,000 lb rows with w 0.6977, which reads
/// 93.77 dB SEL at 1,000 ft instead of the 99.3 dB of the maximum-thrust row (Eq. 4-3).
#[test]
fn b738_cutback_matches_the_doc29_pilot() {
    let class = class_of("B738");
    let model = &THRUST[class];
    let h_ft = 3_000.0 + 355.5 / METRES_PER_FOOT;
    let thrust_lb = rated_thrust_lb(&model.climb_coef, 204.8, h_ft, 15.0);
    assert!((thrust_lb - 18_093.0).abs() < 0.5, "Fn/delta {thrust_lb}");
    let bracket = bracket_power(&model.dep_power, model.dep_rows, thrust_lb);
    assert_eq!(bracket.row, 2);
    assert!((bracket.weight - 0.6977).abs() < 1e-4, "{bracket:?}");
    let at_1000_ft = |bracket| read_npd(class, true, bracket, 1_000.0 * METRES_PER_FOOT).sel_db;
    assert!((at_1000_ft(bracket) - 93.77).abs() < 0.01);
    let top = PowerBracket {
        row: usize::from(model.dep_rows) - 1,
        weight: 0.0,
    };
    assert!((at_1000_ft(top) - 99.3).abs() < 1e-9);
    // Past the table's edges the weight is exactly 0.
    let edge = bracket_power(&model.dep_power, model.dep_rows, 10_000.0);
    assert_eq!(edge, PowerBracket::FIRST_ROW);
    assert_eq!(
        bracket_power(&model.dep_power, model.dep_rows, 100_000.0),
        top
    );
}

#[test]
fn the_bracket_routes_ground_rolls_cutback_and_force_balance() {
    let class = class_of("B738");
    let top_rows = usize::from(THRUST[class].dep_rows) - 2;
    let roll = SegmentFlight {
        on_ground: true,
        ..climbing(355.5, 0.0, 160.0, 0.0)
    };
    assert!(power_bracket(class, &roll).unwrap().row >= top_rows);
    // Below the 2,040 ft cutback the initial climb flies MaxTakeoff as well.
    let initial = climbing(
        355.5 + 500.0 * METRES_PER_FOOT,
        500.0 * METRES_PER_FOOT,
        185.0,
        0.12,
    );
    assert!(power_bracket(class, &initial).unwrap().row >= top_rows);
    // Level at FL360 force balance reads a high row: corrected thrust is thrust / delta (0.22).
    let cruise = climbing(11_000.0, 10_500.0, 450.0, 0.0);
    let bracket = power_bracket(class, &cruise).unwrap();
    assert!(bracket.row >= 2 && (0.0..=1.0).contains(&bracket.weight));
    // Pinned classes never interpolate.
    assert_eq!(
        power_bracket(class_of("DH8D"), &cruise),
        Some(PowerBracket::FIRST_ROW)
    );
}

/// Cutback compares the height above the field, not the local height: a B738 7 deg climb 700 m
/// above its field has passed the 622 m cutback over ground 270 m above the runway, while 500 m
/// above the field over ground 300 m below it still flies MaxTakeoff.
#[test]
fn cutback_compares_the_height_above_the_field() {
    let class = class_of("B738");
    let bracket = |altitude_m, height_above_field_m| {
        let flight = climbing(altitude_m, height_above_field_m, 185.0, 0.122);
        let bracket = power_bracket(class, &flight).unwrap();
        (bracket.row, (bracket.weight * 10_000.0).round() as i64)
    };
    assert_eq!(bracket(700.0, 700.0), (1, 5142));
    assert_eq!(bracket(900.0, 500.0), (3, 5239));
}

/// An ADS-B outlier the filters admit (a B789 at FL510 and 525 kt) inverts the Idle/MaxClimb
/// ratings: the segment is rejected, never clamped into crossed bounds; over a 1,000 ft / 25 kt
/// sweep to 60,000 ft exactly the inverted or non-finite combinations reject, in every class.
#[test]
fn inverted_idle_and_climb_ratings_reject_the_segment() {
    let class = class_of("B789");
    assert_eq!(THRUST[class].class_name, "WING_B789");
    let h_ft = 51_000.0;
    let flight = climbing(h_ft * METRES_PER_FOOT, h_ft * METRES_PER_FOOT, 525.0, 0.0);
    assert_eq!(power_bracket(class, &flight), None);
    let aircraft = AircraftType::from_designator("B789");
    assert_eq!(SegmentEmission::new(&aircraft, &flight, false), None);
    let mut rejected = 0;
    for class in (0..THRUST.len()).filter(|&class| THRUST[class].has_thrust) {
        let model = &THRUST[class];
        for h_ft in (-1_000..=60_000).step_by(1_000).map(f64::from) {
            for speed_kt in (100..=600).step_by(25).map(f64::from) {
                let vc_kt = speed_kt * isa_density_ratio(h_ft).sqrt();
                let temperature_c = isa_temperature_c(h_ft);
                let idle = rated_thrust_lb(&model.idle_coef, vc_kt, h_ft, temperature_c);
                let climb = rated_thrust_lb(&model.climb_coef, vc_kt, h_ft, temperature_c);
                let outside = !(idle.is_finite() && climb.is_finite() && idle <= climb);
                let approach = SegmentFlight {
                    departure: false,
                    ..climbing(
                        h_ft * METRES_PER_FOOT,
                        h_ft * METRES_PER_FOOT,
                        speed_kt,
                        0.0,
                    )
                };
                let bracket = power_bracket(class, &approach);
                assert_eq!(bracket.is_none(), outside, "{} {h_ft} ft", model.class_name);
                rejected += usize::from(outside);
            }
        }
    }
    assert!(rejected > 0, "the sweep must exercise rejections");
}

#[test]
fn generated_tables_follow_class_names_and_anchors() {
    assert_eq!(THRUST.len(), CLASS_NAMES.len());
    for (class, model) in THRUST.iter().enumerate() {
        assert_eq!(model.class_name, CLASS_NAMES[class]);
        assert_eq!(model.anchor_name, class_anchor(class).name, "class {class}");
        if model.has_thrust {
            assert!(model.dep_rows >= 2 && model.app_rows >= 2);
            assert!(usize::from(model.dep_rows.max(model.app_rows)) <= MAX_POWER_ROWS);
            assert!(model.engines >= 1 && model.weight_lb > 0.0);
        }
    }
}

#[test]
fn a_power_bracket_survives_its_tile_code() {
    for row in 0..MAX_POWER_ROWS {
        for weight in [0.0, 0.25, 0.6977, 1.0] {
            let bracket = PowerBracket { row, weight };
            let back = PowerBracket::from_code(bracket.code());
            assert_eq!(back.row, row);
            assert!(
                (back.weight - weight).abs() <= 0.5 / 8191.0,
                "{weight} -> {}",
                back.weight
            );
        }
    }
}
