use super::*;

fn aggregate_events_for_square(segments: &[FlightSegment]) -> Vec<AirborneEvent> {
    let mut events = AirborneEvents::default();
    events.extend(segments);
    events.finish()
}

fn seg(flight_id: u64, lat: f32, lon: f32) -> FlightSegment {
    FlightSegment {
        callsign: String::new(),
        aircraft_type: [0u8; 4],
        flight_id,
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Airborne,
        flags: 0,
        start_lat: lat,
        start_lon: lon,
        start_alt_m: 1000.0,
        end_lat: lat + 0.001,
        end_lon: lon + 0.001,
        end_alt_m: 1100.0,
        speed_kt: 250.0,
        length_m: 200.0,
        agl_avg_m: 500.0,
        start_elev_m: 250.0,
        end_elev_m: 260.0,
    }
}

#[test]
fn aggregate_groups_per_flight() {
    let many: Vec<_> = (0..32).rev().map(|id| seg(id, 50.10, 14.26)).collect();
    assert_eq!(
        aggregate_events_for_square(&many)
            .iter()
            .map(|event| event.flight_id)
            .collect::<Vec<_>>(),
        (0..32).collect::<Vec<_>>()
    );
    let s1 = seg(1, 50.10, 14.26);
    let s2 = seg(1, 50.10, 14.27);
    let s3 = seg(2, 50.10, 14.26);
    let segs = vec![s1, s2, s3];
    let events = aggregate_events_for_square(&segs);
    assert_eq!(events.len(), 2);
    let f1 = events.iter().find(|e| e.flight_id == 1).unwrap();
    assert_eq!(f1.sub_segments.len(), 2);
    let f2 = events.iter().find(|e| e.flight_id == 2).unwrap();
    assert_eq!(f2.sub_segments.len(), 1);
}

#[test]
fn aggregate_filters_non_aircraft_and_non_airborne() {
    let mut gse = seg(1, 50.10, 14.26);
    gse.veh_kind = 1;
    let mut ground = seg(2, 50.10, 14.26);
    ground.phase = Phase::Ground;
    let ok = seg(3, 50.10, 14.26);
    let events = aggregate_events_for_square(&[gse, ground, ok]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].flight_id, 3);
}

#[test]
fn aggregate_propagates_terrain_elevs_from_stage1() {
    // Synthetic case: Stage 1 said start=250 m, end=260 m. Verify both
    // endpoint elevations pass through unchanged.
    let events = aggregate_events_for_square(&[seg(1, 50.10, 14.26)]);
    assert_eq!(events.len(), 1);
    let sub = &events[0].sub_segments[0];
    assert!((sub.terrain_start_elev_m - 250.0).abs() < 1e-3);
    assert!((sub.terrain_end_elev_m - 260.0).abs() < 1e-3);
}

/// Round trip: write a per-z9 airborne shard via the shuffle
/// schema, run Stage 2A, verify an airborne.arrow lands in prepared_year.
#[test]
fn run_stage_2a_consumes_per_square_shard() {
    use crate::arrow_io::write_segments;
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    let square = { crate::spatial::square_id(50.10, 14.26).unwrap() };
    let square_dir = by_square.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    write_segments(
        &square_dir.join("airborne.arrow"),
        &[seg(1, 50.10, 14.26), seg(2, 50.10, 14.27)],
    )
    .unwrap();

    let n = run_stage_2a(&by_square, &prepared_year, 1, 0, None).unwrap();
    assert_eq!(n, 1);
    let out = prepared_year
        .join(square_path(square))
        .join("airborne.arrow");
    assert!(out.exists(), "Stage 2A must write airborne.arrow");
}

/// Regression for wipe-on-scope applied to airborne: a stale
/// `airborne.arrow` in an in-scope z9 must be wiped before
/// `run_stage_2a` returns, even if no airborne segments hit that
/// z9 this run. Symmetric to Stage 2B/2C tests.
#[test]
fn run_stage_2a_wipes_in_scope_stale_airborne() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    // Praha z9 — in-scope. No segments_by_square input → writer does
    // not emit a fresh airborne.arrow for this run.
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let square_dir = prepared_year.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("airborne.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    std::fs::create_dir_all(&by_square).unwrap();
    let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let n = run_stage_2a(&by_square, &prepared_year, 1, 0, Some(&scope)).unwrap();
    assert_eq!(n, 0, "no z9 shards → no z9 written");
    assert!(
        !stale.exists(),
        "stale airborne.arrow must be wiped from in-scope z9"
    );
}

/// Out-of-scope counterexample for the airborne wipe: a stale
/// `airborne.arrow` in an z9 OUTSIDE the scope bbox must survive.
#[test]
fn run_stage_2a_leaves_out_of_scope_stale_airborne() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    // Gran Canaria z9 — outside Praha scope.
    let square = crate::spatial::square_id(27.93, -15.39).unwrap();
    let square_dir = prepared_year.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("airborne.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    std::fs::create_dir_all(&by_square).unwrap();
    let praha = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let _ = run_stage_2a(&by_square, &prepared_year, 1, 0, Some(&praha)).unwrap();
    assert!(
        stale.exists(),
        "out-of-scope z9 airborne.arrow must survive a scoped reextract"
    );
}

#[test]
fn aggregation_preserves_flights_across_decode_batches() {
    let first = seg(7, 50.10, 14.26);
    let mut second = seg(7, 50.11, 14.27);
    second.period = 2;
    second.date_id = 31;
    let mut events = AirborneEvents::default();
    events.extend(&[first, seg(2, 50.10, 14.26)]);
    events.extend(&[second]);
    let events = events.finish();
    assert_eq!(
        events
            .iter()
            .map(|event| event.flight_id)
            .collect::<Vec<_>>(),
        vec![2, 7]
    );
    let segments = &events[1].sub_segments;
    assert_eq!(segments.len(), 2);
    assert_eq!(
        (segments[0].period, segments[1].period, segments[1].date_id),
        (0, 2, 31)
    );
    assert_eq!(
        (segments[0].start_lon, segments[1].start_lon),
        (14.26, 14.27)
    );
}
