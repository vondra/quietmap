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

#[test]
fn counted_spill_keeps_all_flights_and_stream_callback_failure_is_propagated() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("spill.arrow");
    let mut rows = vec![row(1, 500), row(2, 2)];
    rows[0].top_candidates[0].callsign = "A".repeat(8192);
    write_cruise_spill(&path, &rows).unwrap();
    let counts = CruiseSpillCounts::read(&path).unwrap();
    assert_eq!((counts.rows, counts.fids, counts.candidates), (2, 502, 52));
    assert_eq!(
        counts.callsign_bytes,
        rows.iter()
            .flat_map(|row| &row.top_candidates)
            .map(|candidate| candidate.callsign.len())
            .sum::<usize>()
    );
    let mut consumed = 0;
    let error = for_each_cruise_spill(&path, |_| {
        consumed += 1;
        anyhow::bail!("consumer stopped")
    })
    .unwrap_err();
    assert_eq!(consumed, 1);
    assert!(error.to_string().contains("consumer stopped"));
    assert_eq!(read_cruise_spill(&path).unwrap().len(), 2);
}

#[test]
fn ipc_byte_bound_covers_alignment_and_count_metadata_growth() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("spill.arrow");
    let overhead = spill_file_overhead_bound().unwrap();
    for (rows, fids) in [(0, 0), (1, 1), (17, 51), (1000, 200)] {
        let rows: Vec<_> = (0..rows).map(|index| row(index, fids)).collect();
        let counts = CruiseSpillCounts::from_rows(&rows);
        let payload = counts.encoded_buffers_bytes();
        write_cruise_spill(&path, &rows).unwrap();
        assert!(path.metadata().unwrap().len() <= payload as u64 + overhead, "rows={} fids={} candidates={} strings={} actual={} payload={payload} overhead={overhead}", counts.rows, counts.fids, counts.candidates, counts.callsign_bytes, path.metadata().unwrap().len());
    }
}
