//! Regression tests for cruise spill behavior.

use super::*;
use tempfile::tempdir;

fn row(square: u64, n_fids: usize) -> CruiseSpillRow {
    CruiseSpillRow {
        square,
        cruise_cell_id: square + 1,
        class: 5,
        fl_bin: 3,
        period: 2,
        rep_profile_idx: 7,
        source_id: 1,
        origin: 2,
        sum_length_m: 1234.5,
        weight: 2345.6,
        rep_alt_m: 11_000.0,
        rep_speed_kt: 460.0,
        rep_len_m: 800.0,
        rep_len_w: 1234.5,
        fid_set: (0..n_fids as u64).collect(),
        top_candidates: (0..n_fids.min(crate::arrow_schemas::CRUISE_TOP_K))
            .map(|i| CruiseTopCandidate {
                flight_id: i as u64,
                callsign: format!("CALL{i}"),
                aircraft_type: [b'A', b'0' + (i % 10) as u8, 0, 0],
                peak_lmax_25m_db: 100.0 - i as f32,
                altitude_m: 11_000.0,
            })
            .collect(),
    }
}

#[test]
fn write_read_roundtrip() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("test.arrow");
    let rows = vec![row(0x841e3, 3), row(0x842e3, 5)];
    write_cruise_spill(&path, &rows).unwrap();
    let back = read_cruise_spill(&path).unwrap();
    assert_eq!(back.len(), 2);
    for (a, b) in rows.iter().zip(&back) {
        assert_eq!(a.square, b.square);
        assert_eq!(a.cruise_cell_id, b.cruise_cell_id);
        assert_eq!(a.fid_set, b.fid_set);
        assert_eq!(a.top_candidates, b.top_candidates);
        assert!((a.sum_length_m - b.sum_length_m).abs() < 1e-3);
    }
}

#[test]
fn empty_rows_writes_valid_arrow() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("empty.arrow");
    write_cruise_spill(&path, &[]).unwrap();
    let back = read_cruise_spill(&path).unwrap();
    assert!(back.is_empty());
}
