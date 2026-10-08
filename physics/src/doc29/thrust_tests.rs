//! Thrust: dev4's Doc 29 pilot values, the routing of the power bracket and the domain guard.

use super::*;
use crate::doc29::npd::{class_anchor, read_npd};
use crate::doc29::profiles_generated::{CLASS_NAMES, noise_class_of, profile_idx};

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
        acceleration_ms2: None,
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
    // The pilot's levels are in the AIR-1845 atmosphere; the curves read in the model's.
    let atmosphere = crate::doc29::atmosphere::class_increments_db(class, true)[3];
    let at_1000_ft = |bracket| read_npd(class, true, bracket, 1_000.0 * METRES_PER_FOOT).sel_db;
    assert!((at_1000_ft(bracket) - atmosphere - 93.77).abs() < 0.01);
    let top = PowerBracket {
        row: usize::from(model.dep_rows) - 1,
        weight: 0.0,
    };
    assert!((at_1000_ft(top) - atmosphere - 99.3).abs() < 1e-9);
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
    assert!(!THRUST[class_of("AS50")].has_thrust);
    assert_eq!(
        power_bracket(class_of("AS50"), &cruise),
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

/// Where the Idle and MaxClimb fits cross (both outside the envelope they were fitted on) the
/// force balance stands alone: an A380 level at FL350 at 450 kt, an E190 at FL350, a B772 at FL410
/// and a DH8D at 360 kt read the bracket of their balance, where dev4 dropped them. Over a 1,000 ft
/// / 25 kt sweep to 60,000 ft every class reads the balance clamped to its ratings where they do
/// not cross, the balance alone where they do, and never nothing.
#[test]
fn crossed_ratings_leave_the_force_balance() {
    let level = |designator: &str, h_ft: f64, speed_kt: f64| {
        let class = class_of(designator);
        let flight = climbing(
            h_ft * METRES_PER_FOOT,
            h_ft * METRES_PER_FOOT,
            speed_kt,
            0.0,
        );
        let model = &THRUST[class];
        let delta = isa_pressure_ratio(h_ft);
        let balance = force_balance_thrust_lb(model, 0.0, None, 0.95, delta);
        let expected = bracket_power(&model.dep_power, model.dep_rows, balance);
        (power_bracket(class, &flight), expected)
    };
    for (designator, h_ft, speed_kt) in [
        ("A388", 35_000.0, 450.0),
        ("E190", 35_000.0, 450.0),
        ("B772", 41_000.0, 480.0),
        ("DH8D", 20_000.0, 360.0),
    ] {
        let (bracket, expected) = level(designator, h_ft, speed_kt);
        assert_eq!(bracket, Some(expected), "{designator}");
    }
    let mut crossed = 0;
    for class in (0..THRUST.len()).filter(|&class| THRUST[class].has_thrust) {
        let model = &THRUST[class];
        for h_ft in (-1_000..=60_000).step_by(1_000).map(f64::from) {
            for speed_kt in (100..=600).step_by(25).map(f64::from) {
                let vc_kt = speed_kt * isa_density_ratio(h_ft).sqrt();
                let temperature_c = isa_temperature_c(h_ft);
                let idle = model.rated_thrust_lb(Rating::Idle, vc_kt, h_ft, temperature_c);
                let climb = model.rated_thrust_lb(Rating::Climb, vc_kt, h_ft, temperature_c);
                let flight = climbing(h_ft * METRES_PER_FOOT, 3_000.0, speed_kt, 0.0);
                let k = if vc_kt <= 200.0 { 1.01 } else { 0.95 };
                let balance =
                    force_balance_thrust_lb(model, 0.0, None, k, isa_pressure_ratio(h_ft));
                let thrust = if idle <= climb {
                    balance.clamp(idle, climb)
                } else {
                    crossed += 1;
                    balance
                };
                let (got, want) = (
                    power_bracket(class, &flight).expect("a finite thrust"),
                    bracket_power(&model.dep_power, model.dep_rows, thrust),
                );
                assert!(
                    got.row == want.row && (got.weight - want.weight).abs() < 1e-9,
                    "{} {h_ft} ft {speed_kt} kt: {got:?} {want:?}",
                    model.class_name
                );
            }
        }
    }
    assert!(crossed > 0, "the sweep must cross ratings");
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

/// An A320 climbing 7 % at 200 kt after cutback while accelerating at 0.5 m/s^2 flies more thrust
/// than the same climb of unknown acceleration (Eq. B-20: a/g joins the climb gradient and R, and
/// no K), within MaxClimb; decelerating on approach reads less, never below idle.
#[test]
fn acceleration_adds_thrust_to_the_climb() {
    let class = class_of("A320");
    let model = &THRUST[class];
    let steady = climbing(900.0, 600.0, 200.0, 0.07);
    let accelerating = SegmentFlight {
        acceleration_ms2: Some(0.5),
        ..steady
    };
    let thrust = |flight: &SegmentFlight| {
        let bracket = power_bracket(class, flight).expect("in the domain");
        model.dep_power[bracket.row]
            + bracket.weight * (model.dep_power[bracket.row + 1] - model.dep_power[bracket.row])
    };
    // The steady climb's 9,856 lb sits below the table's first row (read at 10,000 lb); the
    // accelerating one's 13,954 lb is bracketed.
    let delta = (1.0_f64 - 6.8756e-6 * 900.0 / METRES_PER_FOOT).powf(5.2559);
    let expected = model.weight_lb / delta * (0.07 + model.drag_ratio + 0.5 / GRAVITY_MS2) / 2.0;
    let (slow, fast) = (thrust(&steady), thrust(&accelerating));
    assert!(
        slow == model.dep_power[0] && (fast - expected).abs() < 1.0,
        "{slow} {fast} {expected}"
    );
    let braking = SegmentFlight {
        departure: false,
        climb_sine: -0.052,
        acceleration_ms2: Some(-0.5),
        ..steady
    };
    assert!(power_bracket(class, &braking).is_some());
}

/// Codex's case (review of the r054 plan): a B738 at 1,500 m and 240 kt climbing at sine 0.07
/// while accelerating at 0.15 m/s^2 flies Eq. B-20, 12,814.57 lb a side, where the factor K on top
/// of the observed acceleration read 13,148.11 lb.
#[test]
fn an_observed_acceleration_replaces_the_constant_cas_factor() {
    let class = class_of("B738");
    let model = &THRUST[class];
    let flight = SegmentFlight {
        acceleration_ms2: Some(0.15),
        ..climbing(1_500.0, 1_500.0, 240.0, 0.07)
    };
    let h_ft = 1_500.0 / METRES_PER_FOOT;
    let delta = isa_pressure_ratio(h_ft);
    let thrust = force_balance_thrust_lb(model, 0.07, Some(0.15), 0.95, delta);
    assert!((thrust - 12_814.57).abs() < 0.01, "{thrust}");
    let with_k =
        model.weight_lb / delta * (0.07 / 0.95 + model.drag_ratio + 0.15 / GRAVITY_MS2) / 2.0;
    assert!((with_k - 13_148.11).abs() < 0.01, "{with_k}");
    let bracket = power_bracket(class, &flight).expect("in the domain");
    assert_eq!(
        bracket,
        bracket_power(&model.dep_power, model.dep_rows, thrust)
    );
}

/// An A320 on a 3 degree glideslope at 140 kt, 300 m above the field, flies its landing flap and
/// gear (Eq. B-25): some 4,500 lb a side, between the 2,700 and 6,000 lb approach rows, where the
/// clean ratio read idle below the first row; above the configuration height it stays at idle.
#[test]
fn a_final_approach_flies_its_landing_configuration() {
    let class = class_of("A320");
    let model = &THRUST[class];
    let approach = &APPROACH[class];
    let final_approach = SegmentFlight {
        departure: false,
        on_ground: false,
        speed_kt: 140.0,
        pressure_altitude_m: 600.0,
        climb_sine: -0.0523,
        acceleration_ms2: None,
        height_above_field_m: 300.0,
    };
    let bracket = power_bracket(class, &final_approach).expect("in the domain");
    let delta = (1.0_f64 - 6.8756e-6 * 600.0 / METRES_PER_FOOT).powf(5.2559);
    let expected =
        approach.landing_weight_lb / delta * (approach.drag_ratio - 0.0523 / APPROACH_K) / 2.0;
    let read = model.app_power[bracket.row]
        + bracket.weight * (model.app_power[bracket.row + 1] - model.app_power[bracket.row]);
    assert!(
        (read - expected).abs() < 1.0 && (2_700.0..6_000.0).contains(&read),
        "{read} {expected}"
    );
    let higher = SegmentFlight {
        height_above_field_m: 1_000.0,
        ..final_approach
    };
    assert_eq!(power_bracket(class, &higher), Some(PowerBracket::FIRST_ROW));
    assert_eq!(APPROACH[class].class_name, CLASS_NAMES[class]);
}

/// The approach table follows the class names; every thrust class has its landing configuration
/// but the MD-11, whose ANP entry gives no approach aerodynamics.
#[test]
fn approach_configurations_follow_class_names() {
    for (class, approach) in APPROACH.iter().enumerate() {
        assert_eq!(approach.class_name, CLASS_NAMES[class]);
        assert_eq!(
            approach.drag_ratio > 0.0,
            THRUST[class].has_thrust && approach.class_name != "MD11GE",
            "{}",
            approach.class_name
        );
    }
}

/// The propeller ratings (Eq. B-5): a C172 climbing out at 80 kt at sea level has 326 x 0.69 x
/// 140 / 80 = 393.6 lb at MaxClimb, 90 % of its 436 lb static thrust, between its 59.6 % and 100 %
/// rows; in level flight at 2,000 ft its force balance, W R / delta = 219 lb, sits between the
/// approach rows (26.6 % and 58.2 %), where the approach row alone read 6.9 dB less at 1,000 ft.
/// The DHC830's jet-form MaxTakeoff at 120 kt 500 ft up is 4,539 lb, 92 % (the BUF tasks' DH8C
/// flies 95-97 % there).
#[test]
fn propeller_and_turboprop_ratings() {
    let c172 = &THRUST[class_of("C172")];
    let climb = c172.rated_thrust_lb(Rating::Climb, 80.0, 0.0, 15.0);
    assert!((climb - 393.645).abs() < 0.01, "{climb}");
    assert_eq!(c172.rated_thrust_lb(Rating::Idle, 80.0, 0.0, 15.0), 0.0);
    let bracket = bracket_power(&c172.dep_power, c172.dep_rows, climb);
    assert_eq!(bracket.row, 0);
    assert!((bracket.weight - 0.7595).abs() < 1e-3, "{bracket:?}");
    let h_m = 2_000.0 * METRES_PER_FOOT;
    let level = SegmentFlight {
        departure: false,
        ..climbing(h_m, h_m, 100.0, 0.0)
    };
    let bracket = power_bracket(class_of("C172"), &level).unwrap();
    assert_eq!(bracket.row, 0);
    assert!((bracket.weight - 0.7475).abs() < 1e-3, "{bracket:?}");
    let dh8d = &THRUST[class_of("DH8D")];
    let takeoff = dh8d.rated_thrust_lb(Rating::Takeoff, 120.0, 500.0, 14.0);
    assert!((takeoff - 4_539.02).abs() < 0.01, "{takeoff}");
    let bracket = bracket_power(&dh8d.dep_power, dh8d.dep_rows, takeoff);
    assert_eq!(bracket.row, 0);
    assert!((bracket.weight - 0.2294).abs() < 1e-3, "{bracket:?}");
}
