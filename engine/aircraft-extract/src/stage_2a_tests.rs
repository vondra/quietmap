use super::*;
use crate::arrow_io::{read_record_batches, write_owner_shard, write_segments};
use arrow::array::{Array, DictionaryArray, Float32Array, Int16Array, UInt64Array, UInt8Array};
use arrow::datatypes::Int32Type;

fn seg(flight_id: u64, lat: f32, lon: f32) -> FlightSegment {
    FlightSegment::airborne_fixture(flight_id, lat, lon)
}

/// Run Stage 2A over one owner shard and decode the written rows as
/// `(flight_id, period, date_id, start_lon, terrain_start_elev_m)`, file order.
fn flatten(rows: &[FlightSegment]) -> Vec<(u64, u8, i16, f32, i16)> {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    write_owner_shard(
        &by_square.join(square_path(square)).join("airborne.arrow"),
        rows,
    )
    .unwrap();
    let written = run_stage_2a(&by_square, &prepared_year, 1, 0, None).unwrap();
    let out = prepared_year
        .join(square_path(square))
        .join("airborne.arrow");
    if written == 0 {
        assert!(!out.exists());
        return Vec::new();
    }
    let (_, batches) = read_record_batches(&out).unwrap();
    let mut decoded = Vec::new();
    let mut flights_in_file = 0;
    for batch in &batches {
        let column = |name: &str| batch.column_by_name(name).unwrap();
        let flight_id = column("flight_id")
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let period = column("period")
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let date = column("date_id")
            .as_any()
            .downcast_ref::<Int16Array>()
            .unwrap();
        let start_gx = column("start_gx")
            .as_any()
            .downcast_ref::<arrow::array::Int32Array>()
            .unwrap();
        let start_gy = column("start_gy")
            .as_any()
            .downcast_ref::<arrow::array::Int32Array>()
            .unwrap();
        let terrain = column("terrain_start_elev_m")
            .as_any()
            .downcast_ref::<Int16Array>()
            .unwrap();
        let flight = column("flight")
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .unwrap();
        // One dictionary per file, shared by every z14 block batch.
        flights_in_file = flight.values().len();
        for i in 0..batch.num_rows() {
            let (lon, _) =
                square_store::grid_cols::grid_cell_lonlat(start_gx.value(i), start_gy.value(i));
            decoded.push((
                flight_id.value(i),
                period.value(i),
                date.value(i),
                lon as f32,
                terrain.value(i),
            ));
        }
    }
    let mut ids: Vec<u64> = decoded.iter().map(|row| row.0).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        flights_in_file,
        ids.len(),
        "one dictionary entry per flight"
    );
    decoded
}

/// Rows come out one per sub-segment with every field intact; ground rows
/// and GSE never enter airborne.arrow. (File order is z14 block order.)
#[test]
fn flatten_orders_rows_by_flight_and_filters_non_aircraft_and_non_airborne() {
    let mut second = seg(7, 50.11, 14.27);
    second.period = 2;
    second.date_id = 31;
    let mut gse = seg(1, 50.10, 14.26);
    gse.veh_kind = 1;
    let mut ground = seg(2, 50.10, 14.26);
    ground.phase = Phase::Ground;
    let mut rows = flatten(&[
        seg(7, 50.10, 14.26),
        seg(3, 50.10, 14.26),
        gse,
        ground,
        second,
    ]);
    rows.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(
        rows.iter()
            .map(|row| (row.0, row.1, row.2))
            .collect::<Vec<_>>(),
        [(3, 0, 0), (7, 0, 0), (7, 2, 31)]
    );
    assert!((rows[1].3 - 14.26).abs() < 1e-5 && (rows[2].3 - 14.27).abs() < 1e-5);
    // Stage 1's endpoint terrain passes through unchanged.
    assert!(rows.iter().all(|row| row.4 == 250));
    assert!(flatten(&[ground_only()]).is_empty());
}

fn ground_only() -> FlightSegment {
    let mut ground = seg(2, 50.10, 14.26);
    ground.phase = Phase::Ground;
    ground
}

/// A Stage 1 day file (no owner stamp) or a shard of the retired
/// support-copy layout would flatten into duplicated or unsplit rows.
#[test]
fn stage_2a_refuses_shards_without_the_owner_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let stale = prepared_year
        .join(square_path(square))
        .join("airborne.arrow");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"previous run").unwrap();
    write_segments(
        &by_square.join(square_path(square)).join("airborne.arrow"),
        &[seg(1, 50.10, 14.26)],
    )
    .unwrap();
    let error = run_stage_2a(&by_square, &prepared_year, 1, 0, None).unwrap_err();
    assert!(format!("{error:#}").contains("rerun shuffle"), "{error:#}");
    assert!(
        stale.exists(),
        "inputs are validated before any output is wiped"
    );
}

/// The writer's length gate is the reader pad's invariant: a piece longer
/// than the cap can only come from a foreign shard, never from the shuffle.
#[test]
fn stage_2a_refuses_rows_longer_than_the_length_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let mut long = seg(1, 50.10, 14.26);
    long.end_lon = 14.26 + 4_001.0 / (111_320.0 * 50.1_f32.to_radians().cos());
    write_owner_shard(
        &by_square.join(square_path(square)).join("airborne.arrow"),
        &[long],
    )
    .unwrap();
    let error = run_stage_2a(&by_square, &prepared_year, 1, 0, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("longer than the 4000 m cap"),
        "{error:#}"
    );
}

#[test]
fn written_rows_keep_length_and_altitude() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let mut row = seg(1, 50.10, 14.26);
    row.length_m = 1234.5;
    write_owner_shard(
        &by_square.join(square_path(square)).join("airborne.arrow"),
        &[row],
    )
    .unwrap();
    assert_eq!(
        run_stage_2a(&by_square, &prepared_year, 3, 12, None).unwrap(),
        1
    );
    let (schema, batches) = read_record_batches(
        &prepared_year
            .join(square_path(square))
            .join("airborne.arrow"),
    )
    .unwrap();
    assert_eq!(schema.metadata().get("n_days").unwrap(), "3");
    assert_eq!(schema.metadata().get("ga_n_days").unwrap(), "12");
    let length = batches[0]
        .column_by_name("length_m")
        .unwrap()
        .as_any()
        .downcast_ref::<Float32Array>()
        .unwrap();
    assert_eq!(length.values(), &[1234.5]);
    let blocks = arrow_batching::parse_blocks(
        schema
            .metadata()
            .get(arrow_batching::QM_BLOCKS_KEY)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].alt_m, [1000.0, 1100.0]);
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
