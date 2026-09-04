//! Callsigns and flight identities remain per rotation, including silent second movements.

use super::*;

#[test]
fn rotation_picks_first_in_range_callsign() {
    use crate::trace::{AircraftTrace, CallsignChange, TracePoint};
    // Callsign frame arrives at point 1 (not at the rotation's
    // very first point). The scalar must still pick it up — it's
    // the first identification inside the movement.
    let pts = vec![
        TracePoint {
            timestamp: 0.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1.0,
            lat: 50.001,
            lon: 14.001,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 2.0,
            lat: 50.002,
            lon: 14.002,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49d328".into(),
        aircraft_type: "A320".into(),
        points: pts,
        callsigns: vec![CallsignChange {
            point_idx: 1,
            value: "TVS100P".into(),
        }],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].callsign, "TVS100P");
}

#[test]
fn second_rotation_without_callsign_stays_empty_not_inherited() {
    use crate::trace::{
        AircraftTrace, CallsignChange, TracePoint, FLAG_ALT_IS_GROUND, FLAG_ON_GROUND_RAW,
    };
    // Rotation 1 announces "ABC", lands and sits 10 min at the gate
    // (sustained on-ground rest ≥ MIN_TURNAROUND_S → leg split).
    // Rotation 2 never broadcasts a callsign — must NOT inherit
    // "ABC".
    let ground = |ts, lat, lon| TracePoint {
        timestamp: ts,
        lat,
        lon,
        alt_ft: f32::NAN,
        speed_kt: 5.0,
        track_deg: 0.0,
        baro_rate_fpm: 0.0,
        flags: FLAG_ON_GROUND_RAW | FLAG_ALT_IS_GROUND,
    };
    let pts = vec![
        TracePoint {
            timestamp: 1_000.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1_010.0,
            lat: 50.001,
            lon: 14.001,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        ground(1_020.0, 50.002, 14.002),
        ground(1_700.0, 50.002, 14.002),
        TracePoint {
            timestamp: 1_710.0,
            lat: 50.003,
            lon: 14.003,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1_720.0,
            lat: 50.004,
            lon: 14.004,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49d328".into(),
        aircraft_type: "A320".into(),
        points: pts,
        callsigns: vec![CallsignChange {
            point_idx: 0,
            value: "ABC".into(),
        }],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 2);
    assert_eq!(flights[0].callsign, "ABC");
    assert_eq!(
        flights[1].callsign, "",
        "rotation 2 must not inherit rotation 1's callsign"
    );
}

#[test]
fn ground_rest_splits_into_two_movements_with_distinct_ids() {
    use crate::trace::{
        AircraftTrace, CallsignChange, TracePoint, FLAG_ALT_IS_GROUND, FLAG_ON_GROUND_RAW,
    };
    // Two rotations separated by a sustained on-ground rest.
    let ground = |ts, lat, lon| TracePoint {
        timestamp: ts,
        lat,
        lon,
        alt_ft: f32::NAN,
        speed_kt: 5.0,
        track_deg: 0.0,
        baro_rate_fpm: 0.0,
        flags: FLAG_ON_GROUND_RAW | FLAG_ALT_IS_GROUND,
    };
    let pts = vec![
        // Rotation 1 (TVS100P)
        TracePoint {
            timestamp: 1_000.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1_010.0,
            lat: 50.001,
            lon: 14.001,
            alt_ft: 1000.0,
            speed_kt: 200.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        // Ground rest at the gate — 10 min ≥ MIN_TURNAROUND_S.
        ground(1_020.0, 50.002, 14.002),
        ground(1_620.0, 50.002, 14.002),
        // Rotation 2 (TVS200X) — taxi-out + lift-off.
        TracePoint {
            timestamp: 1_630.0,
            lat: 51.0,
            lon: 15.0,
            alt_ft: 2000.0,
            speed_kt: 250.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1_640.0,
            lat: 51.001,
            lon: 15.001,
            alt_ft: 2000.0,
            speed_kt: 250.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49d328".into(),
        aircraft_type: "A320".into(),
        points: pts,
        callsigns: vec![
            CallsignChange {
                point_idx: 0,
                value: "TVS100P".into(),
            },
            CallsignChange {
                point_idx: 4,
                value: "TVS200X".into(),
            },
        ],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 2);
    assert_ne!(flights[0].flight_id, flights[1].flight_id);
    assert_eq!(flights[0].callsign, "TVS100P");
    assert_eq!(flights[1].callsign, "TVS200X");
}
