//! Rotation cases: splits at surface rests, the takeoff roll kept, callsigns and identities per
//! rotation, dropped and ground-vehicle designators.

use super::*;
use crate::aircraft::trace::{CallsignChange, SECONDARY_PROVIDER, SURFACE_REPORT};

fn air(timestamp: f64, lat: f32, altitude_ft: f32) -> TracePoint {
    TracePoint {
        timestamp,
        lat,
        lon: 14.0,
        altitude_ft,
        geometric_altitude_ft: f32::NAN,
        ground_speed_kt: 200.0,
        track_deg: 0.0,
        vertical_rate_fpm: 0.0,
        flags: 0,
    }
}

fn ground(timestamp: f64, speed_kt: f32) -> TracePoint {
    TracePoint {
        altitude_ft: f32::NAN,
        ground_speed_kt: speed_kt,
        flags: SURFACE_REPORT,
        ..air(timestamp, 50.002, 0.0)
    }
}

fn trace(designator: &str, points: Vec<TracePoint>, callsigns: &[(usize, &str)]) -> AircraftTrace {
    AircraftTrace {
        address: "49d328".into(),
        aircraft_type: designator.into(),
        emitter_category: 0,
        points,
        callsigns: callsigns
            .iter()
            .map(|&(point_index, callsign)| CallsignChange {
                point_index,
                callsign: callsign.into(),
            })
            .collect(),
    }
}

/// Landing, rollout, taxi, a 10 minute rest, taxi-out, the roll, lift-off: the second rotation
/// starts at the last taxi-speed sample before the roll accelerates, so it holds the roll and
/// the lift-off pair; the first keeps its rollout and taxi-in.
#[test]
fn a_rotation_starts_at_its_takeoff_roll() {
    let points = vec![
        air(0.0, 50.0, 1000.0),
        air(10.0, 50.001, 500.0),
        ground(20.0, 120.0),
        ground(40.0, 20.0),
        ground(700.0, 0.0),
        ground(800.0, 15.0),
        ground(830.0, 8.0),
        ground(840.0, 60.0),
        ground(850.0, 130.0),
        air(855.0, 50.01, 300.0),
        air(860.0, 50.02, 800.0),
    ];
    assert_eq!(split_rotations(&points), vec![0..6, 6..11]);
    // A roll first seen at speed starts at the run's first sample.
    let fast: Vec<_> = points
        .iter()
        .map(|p| TracePoint {
            ground_speed_kt: p.ground_speed_kt.max(40.0),
            ..*p
        })
        .collect();
    assert_eq!(split_rotations(&fast), vec![0..2, 2..11]);
}

#[test]
fn short_rests_gaps_and_leading_rests_keep_one_rotation() {
    let touch_and_go = vec![
        air(0.0, 50.0, 500.0),
        ground(10.0, 90.0),
        air(20.0, 50.003, 200.0),
    ];
    assert_eq!(split_rotations(&touch_and_go), vec![0..3]);
    let oceanic = vec![
        air(0.0, 50.0, 35_000.0),
        air(60.0, 50.1, 35_000.0),
        air(14_400.0, 51.0, 35_000.0),
    ];
    assert_eq!(split_rotations(&oceanic), vec![0..3]);
    let parked_first = vec![
        ground(0.0, 0.0),
        ground(600.0, 5.0),
        air(610.0, 50.0, 1000.0),
        air(615.0, 50.01, 1000.0),
    ];
    assert_eq!(split_rotations(&parked_first), vec![0..4]);
    assert!(split_rotations(&[air(0.0, 50.0, 1000.0)]).is_empty());
}

#[test]
fn each_rotation_has_its_own_identity_and_callsign() {
    let points = vec![
        air(1_000.0, 50.0, 1000.0),
        air(1_010.0, 50.001, 1000.0),
        ground(1_020.0, 5.0),
        ground(1_700.0, 5.0),
        air(1_710.0, 50.003, 1000.0),
        air(1_720.0, 50.004, 1000.0),
    ];
    let flights = trace_to_flights(
        trace("A320", points.clone(), &[(0, "ABC")]),
        ADSB_LOL,
        ADSB_EXCHANGE,
    );
    assert_eq!(flights.len(), 2);
    assert_ne!(flights[0].flight_id, flights[1].flight_id);
    assert_eq!(flights[0].flight_id, (0x49d328 << 40) | 1_000);
    assert_eq!(
        (flights[0].callsign.as_str(), flights[1].callsign.as_str()),
        ("ABC", "")
    );
    let flights = trace_to_flights(
        trace("A320", points, &[(1, "TVS100P"), (4, "TVS200X")]),
        ADSB_LOL,
        ADSB_EXCHANGE,
    );
    assert_eq!(
        (flights[0].callsign.as_str(), flights[1].callsign.as_str()),
        ("TVS100P", "TVS200X")
    );
}

#[test]
fn designators_decide_drops_ground_vehicles_and_profiles() {
    let two = || vec![air(0.0, 50.1, 1000.0), air(1.0, 50.1001, 1000.0)];
    for dropped in ["TWR", "VENT", "GLID", "AS21", "LS8", "BALL"] {
        assert!(
            trace_to_flights(trace(dropped, two(), &[]), ADSB_LOL, ADSB_EXCHANGE).is_empty(),
            "{dropped}"
        );
    }
    let vehicle = &trace_to_flights(
        trace(" gnd ", two(), &[(0, "POZAR4")]),
        ADSB_LOL,
        ADSB_EXCHANGE,
    )[0];
    assert_eq!(
        (
            vehicle.vehicle_kind,
            vehicle.ground_vehicle_class,
            vehicle.profile
        ),
        (1, 2, NO_PROFILE)
    );
    let follow = &trace_to_flights(
        trace("GND", two(), &[(0, "FOLLOWME")]),
        ADSB_LOL,
        ADSB_EXCHANGE,
    )[0];
    assert_eq!(follow.ground_vehicle_class, 0);
    let unknown = &trace_to_flights(trace("", two(), &[]), ADSB_LOL, ADSB_EXCHANGE)[0];
    assert_eq!(
        (unknown.vehicle_kind, unknown.profile),
        (0, physics::doc29::profiles_generated::FALLBACK_PROFILE_IDX)
    );
    let helicopter = &trace_to_flights(trace("EC35", two(), &[]), ADSB_LOL, ADSB_EXCHANGE)[0];
    assert_eq!(helicopter.airframe, Airframe::Helicopter);
}

/// A rotation of secondary samples only carries the secondary provider; anonymous and reserved
/// addresses get deterministic synthetic ids.
#[test]
fn provenance_and_synthetic_identities() {
    let secondary = |flags: [u8; 2]| {
        let mut points = vec![
            air(1_700_000_000.0, 50.1, 3000.0),
            air(1_700_000_010.0, 50.11, 3000.0),
        ];
        points[0].flags = flags[0];
        points[1].flags = flags[1];
        trace_to_flights(trace("B738", points, &[]), ADSB_LOL, ADSB_EXCHANGE)[0].source_id
    };
    assert_eq!(
        secondary([SECONDARY_PROVIDER, SECONDARY_PROVIDER]),
        ADSB_EXCHANGE
    );
    assert_eq!(secondary([0, SECONDARY_PROVIDER]), ADSB_LOL);
    for address in ["~49d328", "ffffff", ""] {
        let mut anonymous = trace(
            "C172",
            vec![air(5.0, 50.1, 1000.0), air(6.0, 50.11, 1000.0)],
            &[],
        );
        anonymous.address = address.into();
        let id = trace_to_flights(anonymous.clone(), ADSB_LOL, ADSB_EXCHANGE)[0].flight_id;
        assert_ne!(id & SYNTHETIC_BIT, 0, "{address}");
        assert_eq!(
            trace_to_flights(anonymous, ADSB_LOL, ADSB_EXCHANGE)[0].flight_id,
            id
        );
    }
}

/// A designator on the fallback flies by the transponder or the whole flight: a rotorcraft's
/// emitter category the helicopter class, a light one the C172, a glider none; no flight number
/// at a median under 140 kt the C172, a glitch to 400 kt notwithstanding; a flight number, a
/// faster flight or a known type keeps its own.
#[test]
fn a_rotation_of_unknown_type_flies_by_its_whole_flight() {
    let speeds = |speeds: &[f32]| -> Vec<TracePoint> {
        speeds
            .iter()
            .enumerate()
            .map(|(k, &speed)| TracePoint {
                ground_speed_kt: speed,
                ..air(10.0 * k as f64, 50.0 + 0.001 * k as f32, 1_000.0)
            })
            .collect()
    };
    let light = profile_idx("C172");
    let slow = speeds(&[95.0, 400.0, 110.0]);
    assert_eq!(rotation_profile("", 0, "OKBYS", &slow), Some(light));
    assert_eq!(
        rotation_profile("PIAX", 0, "N4721K", &speeds(&[88.0, 120.0, 140.0])),
        Some(light)
    );
    let fallback = Some(FALLBACK_PROFILE_IDX);
    assert_eq!(
        rotation_profile("", 0, "RCH123", &speeds(&[130.0, 120.0])),
        fallback
    );
    assert_eq!(
        rotation_profile("", 0, "OKJFA", &speeds(&[130.0, 230.0, 250.0])),
        fallback
    );
    assert_eq!(rotation_profile("C172", 0, "OKABC", &slow), Some(light));
    assert_eq!(
        rotation_profile("B738", 0, "", &slow),
        Some(profile_idx("B738"))
    );
    let rotorcraft = rotation_profile("ALO3", 0xA7, "", &speeds(&[60.0])).unwrap();
    assert!(is_helicopter_class(usize::from(noise_class_of(rotorcraft))));
    assert_eq!(
        rotation_profile("", 0xA1, "", &speeds(&[300.0])),
        Some(light)
    );
    assert_eq!(rotation_profile("", 0xB1, "", &slow), None);
}
