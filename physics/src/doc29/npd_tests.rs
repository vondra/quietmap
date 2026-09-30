//! NPD curves: dev4's interpolation, tail and scaled-distance cases, and the class power rows.

use super::*;
use crate::doc29::profiles_generated::{noise_class_of, profile_idx};
use crate::doc29::thrust::MAX_POWER_ROWS;

fn profile(designator: &str) -> &'static NpdProfile {
    &PROFILES[usize::from(profile_idx(designator))]
}

fn class_of(designator: &str) -> usize {
    usize::from(noise_class_of(profile_idx(designator)))
}

fn feet(feet: f64) -> f64 {
    feet * METRES_PER_FOOT
}

#[test]
fn a_curve_reads_its_table_value_at_every_npd_distance() {
    let curve = &profile("B738").approach_sel;
    let alpha = tail_absorption_db_per_m(curve);
    for (k, distance_ft) in NPD_DISTANCES_FT.iter().enumerate() {
        let level = curve_level_db(curve, alpha, feet(*distance_ft));
        assert!((level - curve[k]).abs() < 1e-9, "{distance_ft} ft: {level}");
    }
}

#[test]
fn between_distances_the_level_stays_between_its_neighbours() {
    let curve = &profile("B738").approach_sel;
    let level = curve_level_db(curve, 0.0, feet(1_500.0));
    assert!(level < curve[3] && level > curve[4], "{level}");
}

#[test]
fn below_200_ft_the_first_interval_extrapolates_down_to_100_ft() {
    let curve = &profile("B738").approach_sel;
    let at_100_ft = curve_level_db(curve, 0.0, feet(100.0));
    assert!(
        (at_100_ft - (2.0 * curve[0] - curve[1])).abs() < 1e-9,
        "{at_100_ft}"
    );
    assert_eq!(curve_level_db(curve, 0.0, 5.0), at_100_ft);
}

#[test]
fn past_25000_ft_the_tail_reads_30_to_45_db_at_15_km() {
    let curve = &PROFILES[0].approach_sel;
    let level = curve_level_db(curve, tail_absorption_db_per_m(curve), feet(50_000.0));
    assert!(level > 30.0 && level < 45.0, "{level}");
}

#[test]
fn the_tail_is_continuous_at_25000_ft_for_every_profile() {
    for profile in &PROFILES {
        for curve in [&profile.approach_sel, &profile.departure_sel] {
            let alpha = tail_absorption_db_per_m(curve);
            for slant_m in [
                NPD_LAST_DISTANCE_M * (1.0 - 1e-9),
                NPD_LAST_DISTANCE_M * (1.0 + 1e-9),
            ] {
                let level = curve_level_db(curve, alpha, slant_m);
                assert!((level - curve[9]).abs() < 1e-6, "{}: {level}", profile.name);
            }
        }
    }
}

#[test]
fn tail_absorption_is_small_and_never_negative() {
    for profile in &PROFILES {
        for curve in [&profile.approach_sel, &profile.departure_sel] {
            let alpha = tail_absorption_db_per_m(curve);
            assert!((0.0..=0.003).contains(&alpha), "{}: {alpha}", profile.name);
        }
    }
    let b738 = tail_absorption_db_per_m(&profile("B738").approach_sel);
    assert!(b738 > 0.0 && b738 < 0.003, "{b738}");
}

/// Doc 29 Eq. 4-11 on the B738 departure curve (V_ref 160 kt): SEL - LAmax interpolated in log
/// distance and extrapolated past 25,000 ft with the last interval.
#[test]
fn b738_departure_scaled_distance_follows_sel_minus_lamax() {
    let anchor = profile("B738");
    let row = PowerRow::new(
        &anchor.departure_sel,
        &anchor.departure_lmax,
        anchor.v_ref_kt,
    );
    let class = class_of("B738");
    let top = PowerBracket {
        row: usize::from(THRUST[class].dep_rows) - 1,
        weight: 0.0,
    };
    for (distance_ft, expected_m) in [
        (1_000.0, 288.0),
        (10_000.0, 2_566.5),
        (25_000.0, 4_259.3),
        (50_000.0, 6_780.3),
    ] {
        let exact = row
            .read(&NpdPosition::at(feet(distance_ft)))
            .scaled_distance_m;
        assert!(
            (exact - expected_m).abs() < 0.15,
            "{distance_ft} ft: {exact} m"
        );
        // The class's loudest departure row carries the anchor's maximum-thrust curves.
        let class_reading = read_npd(class, true, top, feet(distance_ft));
        // The atmosphere's increment moves SEL and LAmax alike: SEL - LAmax, and with it
        // d_lambda, keep their values to rounding.
        assert!((class_reading.scaled_distance_m - exact).abs() < 1e-9 * exact);
    }
}

#[test]
fn a_placeholder_lamax_takes_the_dipole_limit() {
    for designator in ["C172", "AS50"] {
        let class = class_of(designator);
        for slant_m in [10.0, 300.0, 3_000.0, 12_000.0] {
            let reading = read_npd(class, false, PowerBracket::FIRST_ROW, slant_m);
            assert_eq!(reading.scaled_distance_m, slant_m.max(NPD_NEAREST_SLANT_M));
        }
    }
}

/// The two generators agree: every thrust class's loudest departure row is its anchor's departure
/// curve and its quietest approach row its anchor's approach curve.
#[test]
fn power_row_edges_reproduce_the_anchor_curves() {
    for (class, model) in THRUST
        .iter()
        .enumerate()
        .filter(|(_, model)| model.has_thrust)
    {
        let anchor = class_anchor(class);
        let top = usize::from(model.dep_rows) - 1;
        for k in 0..NPD_DISTANCES {
            let departure = model.dep_sel[top][k] - anchor.departure_sel[k];
            let approach = model.app_sel[0][k] - anchor.approach_sel[k];
            assert!(
                departure.abs() < 0.051,
                "{} departure {k}",
                model.class_name
            );
            assert!(approach.abs() < 0.051, "{} approach {k}", model.class_name);
        }
    }
}

/// A pinned class reads its anchor's curve on row zero, moved to the model's atmosphere.
#[test]
fn pinned_classes_read_their_anchor_curve_on_row_zero() {
    for class in (0..NUM_CLASSES).filter(|&class| !THRUST[class].has_thrust) {
        let anchor = class_anchor(class);
        for (departure, raw) in [(false, &anchor.approach_sel), (true, &anchor.departure_sel)] {
            let increments = crate::doc29::atmosphere::class_increments_db(class, departure);
            let curve: [f64; NPD_DISTANCES] = std::array::from_fn(|k| raw[k] + increments[k]);
            for slant_m in [20.0, 150.0, 1_000.0, 7_620.0, 14_000.0] {
                let reading = read_npd(class, departure, PowerBracket::FIRST_ROW, slant_m);
                let expected = curve_level_db(&curve, tail_absorption_db_per_m(&curve), slant_m);
                assert_eq!(reading.sel_db, expected, "class {class}");
            }
        }
    }
}

/// A power bracket interpolates linearly between its two rows (Eq. 4-3).
#[test]
fn a_bracket_lerps_between_its_rows() {
    let class = class_of("A320");
    let at = |row, weight| read_npd(class, true, PowerBracket { row, weight }, 900.0);
    let (low, high, mid) = (at(1, 0.0), at(2, 0.0), at(1, 0.25));
    assert!((mid.sel_db - (0.75 * low.sel_db + 0.25 * high.sel_db)).abs() < 1e-12);
    let lerp_m = 0.75 * low.scaled_distance_m + 0.25 * high.scaled_distance_m;
    assert!((mid.scaled_distance_m - lerp_m).abs() < 1e-9);
}

/// The aircraft bound reads a box's levels at its nearest slant, so no row may rise with distance.
#[test]
fn every_sel_row_falls_with_distance() {
    for rows in POWER_ROWS.iter().flatten() {
        assert!(!rows.is_empty() && rows.len() <= MAX_POWER_ROWS);
        for row in rows {
            assert!(
                row.sel_db.windows(2).all(|pair| pair[1] < pair[0]),
                "{:?}",
                row.sel_db
            );
        }
    }
}

#[test]
fn the_lamax_rise_bound_holds_every_row_between_its_entries() {
    let mut slant_m = 20.0;
    while slant_m < 2.0e6 {
        let bound = lamax_rise_bound_db(slant_m);
        for class in 0..NUM_CLASSES {
            for departure in [false, true] {
                for row in 0..POWER_ROWS[class][usize::from(departure)].len() {
                    for weight in [0.0, 0.5] {
                        if weight > 0.0
                            && row + 1 == POWER_ROWS[class][usize::from(departure)].len()
                        {
                            continue;
                        }
                        let power = PowerBracket { row, weight };
                        let rise = read_npd(class, departure, power, slant_m).lamax_db
                            - read_npd(class, departure, power, LAMAX_REFERENCE_SLANT_M).lamax_db;
                        assert!(rise <= bound + 1e-9, "{slant_m} m: {rise} > {bound}");
                    }
                }
            }
        }
        slant_m *= 1.037;
    }
    assert!(lamax_rise_bound_db(100.0) > 5.0);
    assert!(lamax_rise_bound_db(LAMAX_REFERENCE_SLANT_M).abs() < 1e-9);
    assert!(lamax_rise_bound_db(10_000.0) < -15.0);
}
