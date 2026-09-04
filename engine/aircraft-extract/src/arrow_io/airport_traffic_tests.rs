//! Regression tests for airport traffic behavior.

use super::*;
use tempfile::tempdir;

fn sample_row() -> AirportTrafficRow {
    AirportTrafficRow {
        airport_key: "LKPR".into(),
        osm_id: 42,
        segment_idx: 7,
        geometry_kind: arrow_schemas::GEOMETRY_KIND_LINE,
        start_gx: grid::lonlat_to_grid(14.260, 50.105).0,
        start_gy: grid::lonlat_to_grid(14.260, 50.105).1,
        end_gx: grid::lonlat_to_grid(14.262, 50.106).0,
        end_gy: grid::lonlat_to_grid(14.262, 50.106).1,
        length_m: 250.0,
        ops_kind: 1, // runway
        is_departure: 1,
        veh_kind: 0,
        class_idx: 2, // WING_B738
        period: 0,    // day
        // 8 strictly distinct values — a transposition of any two
        // positions changes the read-back.
        band_energy_lin: [1.0e6, 2.0e6, 3.0e6, 4.0e6, 5.0e6, 6.0e6, 7.0e6, 8.0e6],
        unique_movement_count: 25,
        unique_arr_count: 0,
        unique_dep_count: 25,
        unique_gse_count_per_class: [0, 0, 0],
        microseg_unique_count: 50,
        microseg_unique_arr_count: 25,
        microseg_unique_dep_count: 25,
        microseg_unique_gse_count_per_class: [0, 0, 0],
        microseg_unique_ga_count: 3,
        microseg_unique_ga_arr_count: 1,
        microseg_unique_ga_dep_count: 2,
    }
}

#[test]
fn round_trip_preserves_all_fields() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("airport_traffic.arrow");
    let rows = vec![sample_row()];
    write_airport_traffic(&path, &rows, 14, 365).unwrap();
    let read = read_airport_traffic(&path).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0], rows[0], "every field must round-trip exactly");
}

#[test]
fn round_trip_two_rows_distinguishable() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("airport_traffic.arrow");
    let mut row_gse = sample_row();
    row_gse.veh_kind = 1;
    row_gse.class_idx = 2; // HEAVY
    row_gse.airport_key = "strip:871e3558effffff".into();
    row_gse.unique_movement_count = 6;
    row_gse.unique_arr_count = 0;
    row_gse.unique_dep_count = 0;
    row_gse.unique_gse_count_per_class = [0, 0, 6];
    row_gse.microseg_unique_count = 9;
    row_gse.microseg_unique_arr_count = 0;
    row_gse.microseg_unique_dep_count = 0;
    row_gse.microseg_unique_gse_count_per_class = [1, 2, 6];
    // GSE row: no GA aircraft split (airline-pass only).
    row_gse.microseg_unique_ga_count = 0;
    row_gse.microseg_unique_ga_arr_count = 0;
    row_gse.microseg_unique_ga_dep_count = 0;
    // Distinct band values so a row offset bug surfaces.
    row_gse.band_energy_lin = [
        10.0e6, 20.0e6, 30.0e6, 40.0e6, 50.0e6, 60.0e6, 70.0e6, 80.0e6,
    ];
    let rows = vec![sample_row(), row_gse.clone()];
    write_airport_traffic(&path, &rows, 14, 365).unwrap();
    let read = read_airport_traffic(&path).unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!(read[0], rows[0], "row 0 round-trip");
    assert_eq!(read[1], rows[1], "row 1 round-trip");
}

#[test]
fn empty_rows_writes_valid_arrow_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("airport_traffic.arrow");
    write_airport_traffic(&path, &[], 14, 0).unwrap();
    let read = read_airport_traffic(&path).unwrap();
    assert!(read.is_empty());
}

#[test]
fn hybrid_window_metadata_stamped() {
    // Hybrid extract stamps n_days, ga_n_days, and the per-class vector
    // the consumer's ClassWeights parses.
    let dir = tempdir().unwrap();
    let path = dir.path().join("airport_traffic.arrow");
    write_airport_traffic(&path, &[sample_row()], 12, 365).unwrap();
    let (schema, _) = crate::arrow_io::read_record_batches(&path).unwrap();
    let md = schema.metadata();
    assert_eq!(md.get("n_days").map(String::as_str), Some("12"));
    assert_eq!(md.get("ga_n_days").map(String::as_str), Some("365"));
    let vec = md
        .get("sample_days_by_class")
        .expect("sample_days_by_class stamped");
    assert_eq!(vec.split(',').count(), 15, "15-class vector");
    assert!(vec.contains("365"), "GA classes carry 365: {vec}");
    assert!(vec.contains("12"), "airline classes carry 12: {vec}");

    // Single-window extract: no ga_n_days, uniform vector.
    let p2 = dir.path().join("single.arrow");
    write_airport_traffic(&p2, &[sample_row()], 14, 0).unwrap();
    let (s2, _) = crate::arrow_io::read_record_batches(&p2).unwrap();
    assert!(s2.metadata().get("ga_n_days").is_none());
    assert!(s2
        .metadata()
        .get("sample_days_by_class")
        .unwrap()
        .split(',')
        .all(|d| d == "14"));
}

#[test]
fn reader_rejects_wrong_contract() {
    // Synthetic file with bogus contract metadata must be rejected
    // by `assert_airport_traffic_contract`. Older versions had
    // different column shapes or energy normalization; silent
    // decoding would produce wrong popup numbers.
    for stale_contract in [
        "bogus_v9",
        "airport_traffic_v1",
        "airport_traffic_v2",
        "airport_traffic_v3",
        "airport_traffic_v4",
        "airport_traffic_v8",
    ] {
        use crate::arrow_io::write_record_batches;
        use std::sync::Arc;
        let dir = tempdir().unwrap();
        let path = dir.path().join("bogus.arrow");
        let schema = arrow_schemas::airport_traffic_schema();
        let mut md = schema.metadata().clone();
        md.insert("airport_traffic_contract".into(), stale_contract.into());
        let bogus = Arc::new((*schema).clone().with_metadata(md));
        let empty_batch = RecordBatch::new_empty(bogus.clone());
        write_record_batches(&path, &bogus, &[empty_batch]).unwrap();
        let err = read_airport_traffic(&path).unwrap_err();
        assert!(
            err.to_string().contains("airport_traffic_contract"),
            "stale_contract={stale_contract}: expected contract-mismatch error, got: {err}"
        );
    }
}
