//! Archive layout and complementary hybrid class routing regression cases.

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
        ("PC12", AirlinePass),   // GA turbine single → DH8D residual
        ("GLF4", AirlinePass),   // bizjet shares FUSE_CRJ9 with regional jets
        ("C172", GaPass),        // GA piston
        ("WT9", GaPass),         // ultralight → PROP_C172 class
        ("R44", GaPass),         // helicopter
        ("GYRO", GaPass),        // rotorcraft special designator
        ("GND", AirlinePass),    // GSE belongs to the airline pass
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
