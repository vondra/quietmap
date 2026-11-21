//! Reject normalized streams that silently lose the estimate for unobserved calendar dates.
use super::{Movement, Record, STREAM_VERSION, read, write_record};

#[test]
fn a_normalized_stream_requires_the_current_version_and_its_calendar_component() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("movements.jsonl");
    let mut movement: Movement = serde_json::from_value(serde_json::json!({
        "id":"partial", "source_id":1, "country":"NO", "passenger":2.0, "freight":null,
        "passenger_status":2, "freight_status":0, "matching":0, "rail_types":[0],
        "coverage":{"window_days":4, "covered":[1,0,0,0,0,0]},
        "location":{"geometry":"unlocated", "reason":"test has no mapped station"}
    }))
    .unwrap();
    let write = |version, row: &Movement| {
        let mut file = std::fs::File::create(&path).unwrap();
        write_record(
            &mut file,
            &Record::Window {
                version,
                start: "2024-01-01".into(),
                end: "2024-01-05".into(),
            },
        )
        .unwrap();
        write_record(&mut file, &Record::Movement(Box::new(row.clone()))).unwrap();
    };
    let read_stream = || read(&path, "2024-01-01", "2024-01-05", |_| Ok(()));
    write(STREAM_VERSION - 1, &movement);
    assert!(read_stream().is_err());
    write(STREAM_VERSION, &movement);
    assert!(read_stream().unwrap_err().contains("calendar estimate"));
    movement.estimate_missing_calendar();
    assert_eq!(movement.missing_calendar_estimate, [6.0, 0.0]);
    write(STREAM_VERSION, &movement);
    read_stream().unwrap();
}

#[test]
fn crossing_stream_enforces_flow_invariants_and_accepts_missing_vs_zero() {
    use super::super::types::Crossing;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("crossings.jsonl");
    let check = |crossing: Crossing| {
        let mut file = std::fs::File::create(&path).unwrap();
        write_record(
            &mut file,
            &Record::Window {
                version: STREAM_VERSION,
                start: "2024-01-01".into(),
                end: "2024-01-05".into(),
            },
        )
        .unwrap();
        write_record(&mut file, &Record::Crossing(crossing)).unwrap();
        read(&path, "2024-01-01", "2024-01-05", |_| Ok(()))
    };

    let base = Crossing {
        id: "100001A".into(),
        lat: 41.8,
        lon: -87.6,
        soundings: [0.0; 3],
        speed_kmh: Some(80.0),
        source_id: 111,
        synth_osm: -1,
        total_periods: Some([10.0, 2.0, 6.0]),
        passenger_daily: Some(4.0),
        main_tracks: Some(2),
        freight_only: false,
    };
    check(base.clone()).unwrap();

    // Explicit zero with main_tracks is valid; horn-only with None total_periods is valid.
    let mut explicit_zero = base.clone();
    explicit_zero.total_periods = Some([0.0; 3]);
    explicit_zero.passenger_daily = Some(0.0);
    check(explicit_zero).unwrap();

    let horn_only = Crossing {
        soundings: [6.0, 2.0, 4.0],
        total_periods: None,
        passenger_daily: None,
        main_tracks: None,
        freight_only: false,
        ..base.clone()
    };
    check(horn_only).unwrap();

    // Missing main_tracks alongside total_periods is rejected.
    let mut no_tracks = base.clone();
    no_tracks.main_tracks = None;
    assert!(check(no_tracks).is_err());

    // Zero main_tracks is rejected.
    let mut zero_tracks = base.clone();
    zero_tracks.main_tracks = Some(0);
    assert!(check(zero_tracks).is_err());

    // Passenger exceeding total is rejected.
    let mut bad_pax = base.clone();
    bad_pax.passenger_daily = Some(20.0);
    assert!(check(bad_pax).is_err());

    // Freight-only with positive passenger is rejected.
    let mut bad_freight_only = base.clone();
    bad_freight_only.freight_only = true;
    assert!(check(bad_freight_only).is_err());

    // Empty crossing (neither soundings nor through-train counts) is rejected.
    let mut empty = base.clone();
    empty.total_periods = None;
    empty.passenger_daily = None;
    empty.main_tracks = None;
    assert!(check(empty).is_err());

    // Null Island (0, 0) is rejected.
    let mut null_island = base;
    null_island.lat = 0.0;
    null_island.lon = 0.0;
    assert!(check(null_island).is_err());
}
