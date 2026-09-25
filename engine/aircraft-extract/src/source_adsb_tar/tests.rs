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

/// A rotation made only of secondary-provider samples is tagged with the
/// secondary provider; any primary sample makes it a primary rotation.
#[test]
fn a_rotation_takes_the_secondary_tag_only_when_every_sample_is_secondary() {
    let sample = |timestamp: f64, flags: u8| TracePoint {
        timestamp,
        lat: 50.1,
        lon: 14.25 + (timestamp - 1_700_000_000.0) as f32 * 1e-4,
        alt_ft: 3000.0,
        speed_kt: 150.0,
        track_deg: 90.0,
        baro_rate_fpm: 0.0,
        flags,
    };
    let secondary = crate::trace::FLAG_SECONDARY_PROVIDER;
    for (flags, expected) in [
        ([secondary, secondary], source_id::ADSB_EXCHANGE),
        ([0, secondary], source_id::ADSB_LOL_TAR),
    ] {
        let tr = AircraftTrace {
            icao24: "49f001".into(),
            aircraft_type: "B738".into(),
            points: vec![sample(1_700_000_000.0, flags[0]), sample(1_700_000_010.0, flags[1])],
            callsigns: Vec::new(),
        };
        let flights = trace_to_flight(tr, source_id::ADSB_LOL_TAR, source_id::ADSB_EXCHANGE);
        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].source_id, expected);
    }
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
    let day = s.read_provider_day("2025-01-21").expect("read_provider_day");
    let flights: Vec<_> = day
        .traces
        .into_iter()
        .flat_map(|trace| trace_to_flight(trace, source_id::ADSB_LOL_TAR, source_id::ADSB_EXCHANGE))
        .collect();
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
