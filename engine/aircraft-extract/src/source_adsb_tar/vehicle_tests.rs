//! Ground vehicles, silent towers, gliders, and unknown aircraft retain their dev1 routing.

use super::*;

#[test]
fn gnd_typecode_routes_to_gse_with_class_from_callsign() {
    use crate::trace::{AircraftTrace, CallsignChange, TracePoint};
    let pts = vec![
        TracePoint {
            timestamp: 0.0,
            lat: 50.1,
            lon: 14.25,
            alt_ft: 0.0,
            speed_kt: 5.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1.0,
            lat: 50.1001,
            lon: 14.2501,
            alt_ft: 0.0,
            speed_kt: 5.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49f001".into(),
        aircraft_type: "GND".into(),
        points: pts,
        callsigns: vec![CallsignChange {
            point_idx: 0,
            value: "POZAR4".into(),
        }],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 1);
    let f = &flights[0];
    assert_eq!(f.veh_kind, 1, "GND typecode should route to GSE");
    // POZAR (ARFF / fire truck) → HEAVY (class 2).
    assert_eq!(
        f.gse_class,
        noise_compute::emission::gse::GSE_CLASS_HEAVY,
        "POZAR callsign should map to HEAVY GSE class"
    );
    // The sentinel marks no aircraft profile; veh_kind selects GSE emission.
    assert_eq!(f.profile_idx, u8::MAX, "GSE profile_idx must be sentinel");
}

#[test]
fn lowercase_gnd_routes_to_gse() {
    use crate::trace::{AircraftTrace, CallsignChange, TracePoint};
    let pts = vec![
        TracePoint {
            timestamp: 0.0,
            lat: 50.1,
            lon: 14.25,
            alt_ft: 0.0,
            speed_kt: 5.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1.0,
            lat: 50.1001,
            lon: 14.2501,
            alt_ft: 0.0,
            speed_kt: 5.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49f001".into(),
        aircraft_type: " gnd ".into(), // mixed case + whitespace
        points: pts,
        callsigns: vec![CallsignChange {
            point_idx: 0,
            value: "FOLLOWME".into(),
        }],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(
        flights.len(),
        1,
        "case-insensitive GND match must route, not aircraft-path"
    );
    assert_eq!(flights[0].veh_kind, 1);
    assert_eq!(
        flights[0].gse_class,
        noise_compute::emission::gse::GSE_CLASS_LIGHT
    );
}

#[test]
fn twr_typecode_still_drops() {
    use crate::trace::{AircraftTrace, TracePoint};
    let pts = vec![
        TracePoint {
            timestamp: 0.0,
            lat: 50.1,
            lon: 14.25,
            alt_ft: 0.0,
            speed_kt: 0.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1.0,
            lat: 50.1,
            lon: 14.25,
            alt_ft: 0.0,
            speed_kt: 0.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
    ];
    let tr = AircraftTrace {
        icao24: "49f002".into(),
        aircraft_type: "TWR".into(),
        points: pts,
        callsigns: Vec::new(),
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert!(
        flights.is_empty(),
        "TWR transponders carry no acoustic signal"
    );
}

#[test]
fn glider_typecodes_drop_blank_stays() {
    use crate::trace::{AircraftTrace, TracePoint};
    let pts = || {
        vec![
            TracePoint {
                timestamp: 0.0,
                lat: 47.32,
                lon: 11.48,
                alt_ft: 8000.0,
                speed_kt: 60.0,
                track_deg: 0.0,
                baro_rate_fpm: 0.0,
                flags: 0,
            },
            TracePoint {
                timestamp: 1.0,
                lat: 47.321,
                lon: 11.481,
                alt_ft: 8000.0,
                speed_kt: 60.0,
                track_deg: 0.0,
                baro_rate_fpm: 0.0,
                flags: 0,
            },
        ]
    };
    for glider in ["VENT", "GLID", "AS21", "DG80", "LS8"] {
        let tr = AircraftTrace {
            icao24: "4d2240".into(),
            aircraft_type: glider.into(),
            points: pts(),
            callsigns: Vec::new(),
        };
        assert!(
            trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All).is_empty(),
            "{glider} is a sailplane — must be dropped at Stage 0"
        );
    }
    // Blank typecode = truly unknown, NOT a glider — must survive and
    // keep the FALLBACK energy-mean profile (Apr-29 semantics).
    let tr = AircraftTrace {
        icao24: "4d2241".into(),
        aircraft_type: "".into(),
        points: pts(),
        callsigns: Vec::new(),
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 1, "blank typecode must NOT be dropped");
    assert_eq!(flights[0].profile_idx, profile::FALLBACK_PROFILE_IDX);
}

#[test]
fn aircraft_typecode_stays_veh_kind_zero() {
    use crate::trace::{AircraftTrace, CallsignChange, TracePoint};
    let pts = vec![
        TracePoint {
            timestamp: 0.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 5000.0,
            speed_kt: 250.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        },
        TracePoint {
            timestamp: 1.0,
            lat: 50.001,
            lon: 14.001,
            alt_ft: 5000.0,
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
        callsigns: vec![CallsignChange {
            point_idx: 0,
            value: "TVS100P".into(),
        }],
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::All);
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].veh_kind, 0);
    assert_eq!(flights[0].gse_class, 0);
}

#[test]
fn absent_day_is_an_input_failure_not_a_zero_traffic_day() {
    let dir = tempfile::tempdir().unwrap();
    let source = AdsbTarSource::new(dir.path());
    assert!(source.read_day("2025-01-01").is_err());
    std::fs::create_dir_all(dir.path().join("2025/2025-01-01")).unwrap();
    assert!(source.read_day("2025-01-01").is_err());
}
