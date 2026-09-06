//! Stage 2C destination and fail-before-wipe regressions.
use super::*;
use crate::arrow_io::read_airport_summary;
use crate::flight::{FlightSegment, Phase};

/// End-to-end Stage 2C pipeline: writer + reduce → loadable
/// airport_summary.arrow. Uses a single arrival rotation crossing
/// one runway microsegment; asserts the global UNION carries the
/// arrival count = 1.
#[test]
fn run_stage_2c_produces_airport_summary_arrow() {
    use crate::airport_io::AERODROME_AEROWAY_TYPE;
    use crate::arrow_io::write_segments;
    use crate::geo::square_path;
    use crate::stage_2c::airport_traffic_writer::tests::write_real_airport_lines_arrow;
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    let lat = 50.0_f64;
    let lon0 = 14.0;
    let lon1 = 14.001;
    let mid_lat = lat;
    let mid_lon = (lon0 + lon1) * 0.5;
    let square = crate::spatial::square_id(mid_lat, mid_lon).unwrap();
    let square_prepared_year_dir = prepared_year_dir.join(square_path(square));
    let square_input_dir = by_square_dir.join(square_path(square));
    std::fs::create_dir_all(&square_prepared_year_dir).unwrap();
    std::fs::create_dir_all(&square_input_dir).unwrap();
    write_real_airport_lines_arrow(
        &square_prepared_year_dir.join("airport_lines.arrow"),
        &[airport_traffic_writer::tests::FakeRealLine {
            osm_id: 42,
            segment_idx: 0,
            start_lat: lat,
            start_lon: lon0,
            end_lat: lat,
            end_lon: lon1,
            length_m: 100.0,
            aeroway_type: 0,
        }],
    );
    let leg = FlightSegment {
        flight_id: 0xDEAD_BEEF_u64,
        callsign: "TEST1".to_string(),
        aircraft_type: *b"B738",
        profile_idx: 23,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Ground,
        flags: 0, // is_departure=0 → arrival
        start_lat: lat as f32,
        start_lon: lon0 as f32,
        start_alt_m: 0.0,
        end_lat: lat as f32,
        end_lon: lon1 as f32,
        end_alt_m: 0.0,
        speed_kt: 90.0,
        length_m: 100.0,
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    };
    let aerodrome = AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "Test".to_string(),
        "LKTEST".to_string(),
        mid_lat,
        mid_lon,
        Vec::new(),
        100_000_000.0,
    );
    write_segments(&square_input_dir.join("ground.arrow"), &[leg]).unwrap();

    let n = run_stage_2c(
        &by_square_dir,
        std::slice::from_ref(&aerodrome),
        &prepared_year_dir,
        1,
        0,
        None,
    )
    .unwrap();
    assert!(n > 0);

    let summary_path = square_prepared_year_dir.join(AIRPORT_SUMMARY_FILENAME);
    assert!(
        summary_path.exists(),
        "airport_summary.arrow must exist at {}",
        summary_path.display()
    );
    let rows = read_airport_summary(&summary_path).unwrap();
    // One airport, one arrival fid → UNION count = 1.
    let lktest = rows
        .iter()
        .find(|r| r.airport_key == "LKTEST")
        .expect("LKTEST row");
    assert_eq!(lktest.airport_unique_arr_count, 1);
    assert_eq!(lktest.airport_unique_dep_count, 0);
    // Runway ops_kind = index 0.
    assert_eq!(lktest.airport_unique_ops_count_per_kind[0], 1);
    assert!(!prepared_year_dir.join("aircraft").exists());

    // airport_summary_parts scratch dir must be cleaned up.
    let parts = prepared_year_dir.join("airport_summary_parts");
    assert!(
        !parts.exists(),
        "airport_summary_parts/ must be cleaned up after run"
    );
}

/// Regression for the wipe-on-scope bug: a stale
/// `airport_traffic.arrow` from a previous run that landed in an
/// in-scope z9 with NO current ground traffic must be deleted
/// before `run_stage_2c` returns. Mirrors the LKPR-14d reproducer
/// that motivated the wipe.
#[test]
fn run_stage_2c_wipes_in_scope_stale_airport_traffic() {
    use crate::geo::square_path;
    use crate::scope::ScopeBbox;
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    // Praha z9 — in-scope. No ground.arrow input → writer does
    // not emit a fresh airport_traffic.arrow for this run.
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let square_dir = prepared_year_dir.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("airport_traffic.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    let stale_summary = square_dir.join(AIRPORT_SUMMARY_FILENAME);
    std::fs::write(&stale_summary, b"stale-summary").unwrap();
    std::fs::create_dir_all(&by_square_dir).unwrap();
    // Praha scope.
    let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let n = run_stage_2c(&by_square_dir, &[], &prepared_year_dir, 1, 0, Some(&scope)).unwrap();
    assert_eq!(n, 0, "no ground segments → no z9 written");
    assert!(
        !stale.exists(),
        "stale airport_traffic.arrow must be wiped from in-scope z9"
    );
    assert!(!stale_summary.exists());
}

/// Regression for the wipe-before-error fragility: when
/// `segments_by_square_dir` carries a shard with a wrong-schema
/// `ground.arrow`, the precheck must reject the run BEFORE the
/// destructive wipe runs. Otherwise a stale shuffle leaves the
/// popup with neither old nor new airport_traffic.arrow files.
#[test]
fn run_stage_2c_aborts_on_stale_input_before_wipe() {
    use crate::geo::square_path;
    use crate::scope::ScopeBbox;
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    // Pre-populate a fresh-looking airport_traffic.arrow in
    // prepared_year_dir to confirm precheck keeps it intact on rejection.
    let prepared_year_square = prepared_year_dir.join(square_path(square));
    std::fs::create_dir_all(&prepared_year_square).unwrap();
    let prior = prepared_year_square.join("airport_traffic.arrow");
    std::fs::write(&prior, b"prior-good-output").unwrap();
    // Corrupt ground.arrow shard (will fail the schema check).
    let by_square_square = by_square_dir.join(square_path(square));
    std::fs::create_dir_all(&by_square_square).unwrap();
    std::fs::write(by_square_square.join("ground.arrow"), b"not-an-arrow-file").unwrap();
    let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let result = run_stage_2c(&by_square_dir, &[], &prepared_year_dir, 1, 0, Some(&scope));
    assert!(result.is_err(), "precheck must reject corrupt shard");
    assert!(
        prior.exists(),
        "precheck must abort before wipe — prior airport_traffic.arrow lost"
    );
}

/// Symmetric counterexample: a stale `airport_traffic.arrow`
/// inside an OUT-OF-scope z9 must survive. Partial reextracts
/// must not touch other regions' data.
#[test]
fn scoped_run_rejects_existing_global_traffic_before_replacing_summary() {
    use crate::geo::square_path;
    use crate::scope::ScopeBbox;
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    // Gran Canaria z9 — out of Praha scope.
    let square = crate::spatial::square_id(27.93, -15.39).unwrap();
    let square_dir = prepared_year_dir.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("airport_traffic.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    std::fs::create_dir_all(&by_square_dir).unwrap();
    let praha = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let summary = square_dir.join(AIRPORT_SUMMARY_FILENAME);
    std::fs::create_dir_all(summary.parent().unwrap()).unwrap();
    std::fs::write(&summary, b"prior-global-summary").unwrap();
    let error =
        run_stage_2c(&by_square_dir, &[], &prepared_year_dir, 1, 0, Some(&praha)).unwrap_err();
    assert!(error.to_string().contains("global movement union"));
    assert_eq!(std::fs::read(summary).unwrap(), b"prior-global-summary");
    assert!(
        stale.exists(),
        "out-of-scope z9 file must survive a scoped reextract"
    );
}
