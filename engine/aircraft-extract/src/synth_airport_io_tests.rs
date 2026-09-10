//! Regression tests for synth airport io behavior.

use super::*;
#[test]
fn synthetic_identity_has_one_reversible_grid_key() {
    let id = synth_osm_id_for(50.1, 14.26);
    assert_ne!(id & SYNTHETIC_OSM_ID_BIT, 0);
    assert!(synth_airport_key_for(50.1, 14.26).starts_with("auto-142"));
    assert_eq!(id, synth_osm_id_for(50.1, 14.26));
    assert_ne!(id, synth_osm_id_for(50.5, 14.26));
}

fn sample_lines_row(seg_idx: u16) -> SynthAirportLineRow {
    SynthAirportLineRow {
        osm_id: synth_osm_id_for(50.1, 14.26),
        segment_idx: seg_idx,
        airport_key: synth_airport_key_for(50.1, 14.26),
        start_gx: grid::lonlat_to_grid(14.26, 50.1).0,
        start_gy: grid::lonlat_to_grid(14.26, 50.1).1,
        end_gx: grid::lonlat_to_grid(14.27, 50.105).0,
        end_gy: grid::lonlat_to_grid(14.27, 50.105).1,
        length_m: 500.0,
        heading_deg: 60.0,
        aeroway_type: AIRSTRIP_AEROWAY_TYPE,
        name: DISCOVERED_AIRSTRIP_NAME.into(),
    }
}

#[test]
fn write_then_read_lines_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("synth_airport_lines.arrow");
    let rows = vec![sample_lines_row(0), sample_lines_row(1)];
    write_synth_airport_lines(&path, rows.clone()).unwrap();
    let back = read_synth_airport_lines(&path).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].osm_id, rows[0].osm_id);
    assert_eq!(back[0].airport_key, rows[0].airport_key);
    assert_eq!(back[1].segment_idx, 1);
    assert_eq!(back[0].name, rows[0].name);
    assert_eq!(back[0].aeroway_type, AIRSTRIP_AEROWAY_TYPE);
}

#[test]
fn read_missing_file_returns_empty_vec() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = read_synth_airport_lines(&tmp.path().join("absent.arrow")).unwrap();
    assert!(lines.is_empty());
}

#[test]
fn write_overwrite_replaces_lines_does_not_append() {
    // Truncate-and-rewrite invariant: a second write at the
    // same path must replace previous content, not extend it.
    // Atomicity is provided by `arrow_io::write_record_batches`;
    // this test pins the behavioural contract callers depend on.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("synth_airport_lines.arrow");
    write_synth_airport_lines(
        &path,
        [
            sample_lines_row(0),
            sample_lines_row(1),
            sample_lines_row(2),
        ],
    )
    .unwrap();
    assert_eq!(read_synth_airport_lines(&path).unwrap().len(), 3);
    write_synth_airport_lines(&path, [sample_lines_row(0)]).unwrap();
    assert_eq!(read_synth_airport_lines(&path).unwrap().len(), 1);
}

#[test]
fn write_creates_missing_parent_dir() {
    // `arrow_io::write_record_batches` runs `create_dir_all`
    // before writing — Stage 1.5 emits into per-z9 dirs that
    // may not exist yet (no OSM data → no prior aircraft files).
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("84/1e3/5ff");
    write_synth_airport_lines(
        &nested.join("synth_airport_lines.arrow"),
        [sample_lines_row(0)],
    )
    .unwrap();
    assert!(nested.join("synth_airport_lines.arrow").exists());
}

#[test]
fn synthetic_high_bit_disjoint_from_real_osm_ids() {
    // Real OSM IDs (both ways and relations) are positive `i64`
    // in this codebase (`osm-extract/main.rs:200,287`), so their
    // high bit is always 0. The synth encoding must never collide.
    let real_ids: [u64; 4] = [1, 1_234_567, i64::MAX as u64, (i64::MAX as u64) - 1];
    let synth = synth_osm_id_for(50.1, 14.26);
    for r in real_ids {
        assert_eq!(r & SYNTHETIC_OSM_ID_BIT, 0);
        assert_ne!(synth, r);
    }
}

#[test]
fn streaming_writer_keeps_row_order_across_bounded_batches_and_replaces_with_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lines.arrow");
    let count = 2 * SYNTH_WRITE_BATCH_ROWS + 1;
    write_synth_airport_lines(&path, (0..count).map(|i| sample_lines_row(i as u16))).unwrap();
    let (_, batches) = crate::arrow_io::read_record_batches(&path).unwrap();
    assert_eq!(
        batches
            .iter()
            .map(RecordBatch::num_rows)
            .collect::<Vec<_>>(),
        [SYNTH_WRITE_BATCH_ROWS, SYNTH_WRITE_BATCH_ROWS, 1]
    );
    let rows = read_synth_airport_lines(&path).unwrap();
    assert_eq!(rows.len(), count);
    assert!(rows
        .iter()
        .enumerate()
        .all(|(i, row)| usize::from(row.segment_idx) == i));
    write_synth_airport_lines(&path, []).unwrap();
    assert!(read_synth_airport_lines(&path).unwrap().is_empty());
}
