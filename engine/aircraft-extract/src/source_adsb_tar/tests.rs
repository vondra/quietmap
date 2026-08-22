use super::*;

fn is_synth(fid: u64) -> bool {
    fid & profile::SYNTHETIC_BIT != 0
}

#[test]
fn day_dir_year_nested_used_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let nested = root.join("2025").join("2025-07-17");
    std::fs::create_dir_all(&nested).unwrap();
    let s = AdsbTarSource::new(root);
    assert_eq!(s.day_dir("2025-07-17"), nested);
}

#[test]
fn day_dir_flat_fallback_when_year_layer_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let flat = root.join("2025-07-17");
    std::fs::create_dir_all(&flat).unwrap();
    let s = AdsbTarSource::new(root);
    // `<root>/2025/2025-07-17` does not exist; should fall back to
    // `<root>/2025-07-17` for bbox/radius subsets emitted directly
    // under the cache root.
    assert_eq!(s.day_dir("2025-07-17"), flat);
}

/// Raw adsb.lol release naming (`<archive-root>/<year>/` layout) resolves
/// as the second candidate;
/// the plain year-nested form keeps precedence when both exist.
/// The `…prod-0tmp` upstream tag variant (15 real days in 2025-05/06)
/// resolves too.
#[test]
fn day_dir_release_naming_used_when_plain_day_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let release = root.join("2025").join("v2025.07.01-planes-readsb-prod-0");
    std::fs::create_dir_all(&release).unwrap();
    let s = AdsbTarSource::new(root);
    assert_eq!(s.day_dir("2025-07-01"), release);
    let tmp_variant = root
        .join("2025")
        .join("v2025.06.05-planes-readsb-prod-0tmp");
    std::fs::create_dir_all(&tmp_variant).unwrap();
    assert_eq!(s.day_dir("2025-06-05"), tmp_variant);
    let plain = root.join("2025").join("2025-07-01");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(
        s.day_dir("2025-07-01"),
        plain,
        "plain layout wins over release naming"
    );
}

/// Pass complementarity: across
/// jet / airline-turboprop / GA / heli / GSE / TWR / glider / blank
/// traces, every trace lands in EXACTLY one of (GaOnly-kept,
/// NonGa-kept, dropped-by-both-as-designed {TWR, glider}) — and the
/// union of the two passes equals the single-window `All` keep set.
#[test]
fn class_window_filter_complementarity() {
    #[derive(PartialEq, Debug, Clone, Copy)]
    enum Lands {
        GaPass,
        AirlinePass,
        DroppedByBoth,
    }
    use Lands::*;
    let mk = |typecode: &str| AircraftTrace {
        icao24: "49d328".into(),
        aircraft_type: typecode.into(),
        points: vec![
            TracePoint {
                timestamp: 0.0,
                lat: 50.0,
                lon: 14.0,
                alt_ft: 2000.0,
                speed_kt: 120.0,
                track_deg: 0.0,
                baro_rate_fpm: 0.0,
                flags: 0,
            },
            TracePoint {
                timestamp: 10.0,
                lat: 50.001,
                lon: 14.001,
                alt_ft: 2000.0,
                speed_kt: 120.0,
                track_deg: 0.0,
                baro_rate_fpm: 0.0,
                flags: 0,
            },
        ],
        callsigns: Vec::new(),
    };
    let cases: &[(&str, Lands)] = &[
        ("B738", AirlinePass),   // jet
        ("AT72", AirlinePass),   // airline turboprop — PROP_DH8D stays 12-day
        ("PC12", AirlinePass),   // GA turbine single → DH8D residual (plan §3)
        ("GLF4", AirlinePass),   // bizjet shares FUSE_CRJ9 with regional jets
        ("C172", GaPass),        // GA piston
        ("WT9", GaPass),         // ultralight → PROP_C172 class
        ("R44", GaPass),         // helicopter
        ("GYRO", GaPass),        // rotorcraft special designator
        ("GND", AirlinePass),    // GSE belongs to the airline pass (plan §3)
        ("TWR", DroppedByBoth),  // control-tower transponder
        ("GLID", DroppedByBoth), // sailplane
        ("", AirlinePass),       // blank = FALLBACK, non-GA by design
    ];
    for &(typecode, expected) in cases {
        let kept = |w: ClassWindowFilter| {
            !trace_to_flight(mk(typecode), source_id::ADSB_LOL_TAR, w).is_empty()
        };
        let ga = kept(ClassWindowFilter::GaOnly);
        let non_ga = kept(ClassWindowFilter::NonGa);
        let all = kept(ClassWindowFilter::All);
        assert!(!(ga && non_ga), "{typecode:?} must not land in both passes");
        assert_eq!(
            ga || non_ga,
            all,
            "{typecode:?}: union of GA + airline passes must equal the All keep set"
        );
        let landed = match (ga, non_ga) {
            (true, false) => GaPass,
            (false, true) => AirlinePass,
            (false, false) => DroppedByBoth,
            (true, true) => unreachable!(),
        };
        assert_eq!(landed, expected, "{typecode:?}");
        // The probe-side predicate must agree with the authoritative
        // trace_to_flight outcome for both passes.
        assert_eq!(
            ClassWindowFilter::GaOnly.keeps_typecode(typecode),
            ga,
            "{typecode:?} probe/GaOnly"
        );
        assert_eq!(
            ClassWindowFilter::NonGa.keeps_typecode(typecode),
            non_ga,
            "{typecode:?} probe/NonGa"
        );
    }
}

/// GSE routing survives the airline pass: a GND trace kept by NonGa
/// still becomes a `veh_kind = 1` GSE flight (not an aircraft).
#[test]
fn non_ga_pass_keeps_gse_routing() {
    let tr = AircraftTrace {
        icao24: "49f001".into(),
        aircraft_type: "GND".into(),
        points: vec![
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
        ],
        callsigns: Vec::new(),
    };
    let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, ClassWindowFilter::NonGa);
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].veh_kind, 1);
}

/// Skips unless QM_FLIGHTS_CACHE points at a radius cache root containing
/// 2025/2025-01-21 (the same cache as ADSB_CACHE in scripts/run-aircraft-extract.sh).
#[test]
fn smoke_real_praha_day() {
    let Ok(root) = std::env::var("QM_FLIGHTS_CACHE") else {
        return;
    };
    if !std::path::Path::new(&root).join("2025/2025-01-21").exists() {
        return;
    }
    let s = AdsbTarSource::new(root);
    let flights = s.read_day("2025-01-21").expect("read_day");
    assert!(flights.len() > 100, "got {}", flights.len());
    let real = flights.iter().filter(|f| !is_synth(f.flight_id)).count();
    assert!(
        real * 2 > flights.len(),
        "real {} vs total {}",
        real,
        flights.len()
    );
    // Per-movement flight_ids: every rotation gets a unique ID,
    // so `flights.len() > unique_icao24` once any aircraft does
    // a turn-around inside the cache day.
    let unique_real_ids: std::collections::HashSet<u64> =
        flights.iter().map(|f| f.flight_id).collect();
    assert_eq!(
        unique_real_ids.len(),
        flights.len(),
        "every movement must have a unique flight_id"
    );
    // Callsign survival across rotations: ≥30% of flights carry a
    // non-empty callsign so display can identify the operator.
    let with_callsign = flights.iter().filter(|f| !f.callsign.is_empty()).count();
    assert!(
        with_callsign * 10 > flights.len() * 3,
        "expected ≥30% Flight.callsign post-rebase, got {with_callsign}/{}",
        flights.len()
    );
}

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
    // GSE flights must NOT carry a real aircraft profile_idx — the
    // u8::MAX sentinel forces a panic on any accidental NPD lookup
    // (otherwise WING_FALLBACK = 123 would silently re-introduce a
    // ~25 dB over-estimate).
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
