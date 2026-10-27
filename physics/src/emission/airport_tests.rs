//! Ground operations against dev4's numbers: the per-class taxiing at the reference speed, the 1 km
//! pass read at 25 m, the dwell clamp and the ground vehicles' pass.

use super::*;
use crate::bands::{A_WEIGHTING_DB, energy};
use crate::doc29::profiles_generated::CLASS_NAMES;
use crate::line::POINT_DIVERGENCE_LINEAR;

/// The class of an ANP aircraft.
fn class_named(name: &str) -> u8 {
    CLASS_NAMES
        .iter()
        .position(|&n| n == name)
        .expect("a class") as u8
}

fn total_db(bands: &[f64; BANDS]) -> f64 {
    10.0 * bands
        .iter()
        .map(|level| energy(*level))
        .sum::<f64>()
        .log10()
}

fn a_weighted_db(bands: &[f64; BANDS]) -> f64 {
    10.0 * (0..BANDS)
        .map(|band| energy(bands[band] + A_WEIGHTING_DB[band]))
        .sum::<f64>()
        .log10()
}

/// Z-weighted pass energy of a 737-800 taxiing (dev4's class 2 in its tests).
fn b738(speed_kt: f64) -> f64 {
    total_db(&aircraft_taxi_energy_db(class_named("737800"), speed_kt).unwrap())
}

/// The families keep dev4's per-class taxiing, its runway anchors less 12 dB: its narrowbody,
/// widebody, regional, business, turboprop, piston and helicopter classes.
#[test]
fn every_family_taxis_twelve_decibels_below_dev4s_runway_anchor() {
    for (name, runway_anchor) in [
        ("737800", 104.0),
        ("A320-232", 104.0),
        ("7879", 108.0),
        ("7478", 108.0),
        ("CRJ9-ER", 100.0),
        ("CIT3", 99.0),
        ("DHC830", 97.0),
        ("CNA172", 92.0),
        ("HELICOPTER", 94.0),
    ] {
        let family = CLASS_FAMILY[usize::from(class_named(name))];
        assert_eq!(taxi_event_sel_db(family), runway_anchor - 12.0, "{name}");
    }
}

/// At 18 kt the sum is dev4's per-metre level (anchor + 9.01 dB) plus the 11 dB of the point
/// divergence: 112.01 dB for a 92 dB anchor, in dev4's taxi shape.
#[test]
fn the_reference_speed_gives_dev4_levels_plus_the_point_divergence() {
    assert!((b738(18.0) - 112.01).abs() < 1e-9);
    let widebody = aircraft_taxi_energy_db(class_named("7879"), 18.0).unwrap();
    assert!((total_db(&widebody) - 116.01).abs() < 1e-9);
    let spectrum = aircraft_taxi_energy_db(class_named("737800"), 18.0).unwrap();
    assert!(
        (spectrum[0] - spectrum[7] - 21.0).abs() < 1e-9,
        "taxi shape 14 .. -7 dB"
    );
}

/// dev4's calibration: 25 m from the middle of a 1 km taxi roll the receiver reads the 1 km event
/// anchor (104 - 12 dB) minus 0.14 dB; here through the CNOSSOS point sum theta / (10^1.1 d).
#[test]
fn a_kilometre_roll_reads_the_anchor_at_25_m() {
    let theta = 2.0 * (500.0f64 / 25.0).atan();
    let received = b738(18.0) + 10.0 * (theta / (POINT_DIVERGENCE_LINEAR * 25.0)).log10();
    let anchor = 104.0 - 12.0;
    assert!((received - (anchor - 0.14)).abs() < 0.01, "{received}");
}

/// Slower is louder per metre, within +-3 dB: 8 kt pins at +3 dB, 36 kt at -3 dB; below 1 kt no
/// correction, and a stopped aircraft leaves nothing.
#[test]
fn the_dwell_correction_is_clamped_at_three_decibels() {
    assert!(b738(10.0) > b738(18.0) && b738(18.0) > b738(30.0));
    assert!((b738(8.0) - b738(18.0) - 3.0).abs() < 1e-9);
    assert!((b738(36.0) - b738(18.0) + 3.0).abs() < 1e-9);
    assert!((b738(0.5) - b738(18.0)).abs() < 1e-9);
    for stopped in [0.0, -1.0, f64::NAN] {
        assert_eq!(
            aircraft_taxi_energy_db(class_named("737800"), stopped),
            None
        );
        assert_eq!(vehicle_pass_energy_db(GroundVehicle::Heavy, stopped), None);
    }
}

/// dev4's per-event SEL at 25 m of a vehicle passing a 50 m stretch at 9.72 kt,
/// Lw + 10 lg(atan(L / 2r) / (2 pi v r)), read here through the line: equal within 0.01 dB
/// (10^1.1 against 4 pi).
#[test]
fn a_vehicle_pass_reads_dev4_moving_point_level() {
    let (length, speed_kt, r) = (50.0f64, 9.72, 25.0f64);
    let speed = speed_kt * METRES_PER_SECOND_PER_KT;
    for vehicle in [
        GroundVehicle::Light,
        GroundVehicle::Medium,
        GroundVehicle::Heavy,
    ] {
        let theta = 2.0 * (length / (2.0 * r)).atan();
        let line = vehicle_pass_energy_db(vehicle, speed_kt).unwrap();
        let dev4_offset =
            10.0 * ((length / (2.0 * r)).atan() / (2.0 * std::f64::consts::PI * speed * r)).log10();
        for (band, level) in line.iter().enumerate() {
            let received = level + 10.0 * (theta / (POINT_DIVERGENCE_LINEAR * r)).log10();
            let dev4 = vehicle.sound_power_db()[band] + dev4_offset;
            assert!(
                (received - dev4).abs() < 0.01,
                "{vehicle:?} {received} {dev4}"
            );
        }
    }
    // Ten times faster, a tenth of the energy per metre.
    let slow = total_db(&vehicle_pass_energy_db(GroundVehicle::Medium, 5.0).unwrap());
    let fast = total_db(&vehicle_pass_energy_db(GroundVehicle::Medium, 50.0).unwrap());
    assert!((slow - fast - 10.0).abs() < 1e-9);
}

#[test]
fn vehicle_classes_grow_louder_and_other_codes_are_none() {
    let level = |code| a_weighted_db(&GroundVehicle::from_code(code).unwrap().sound_power_db());
    assert!((level(0) - 89.3).abs() < 0.5, "{}", level(0));
    assert!((level(1) - 100.2).abs() < 0.5, "{}", level(1));
    assert!((level(2) - 103.3).abs() < 0.5, "{}", level(2));
    assert_eq!(GroundVehicle::from_code(3), None);
}
