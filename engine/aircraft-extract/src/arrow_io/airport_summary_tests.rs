//! Regression tests for airport summary behavior.

use super::*;
use tempfile::tempdir;

fn sample_row() -> AirportSummaryRow {
    AirportSummaryRow {
        airport_key: "LKPR".into(),
        airport_unique_arr_count: 100,
        airport_unique_dep_count: 105,
        airport_unique_gse_count_per_class: [12, 34, 56],
        airport_unique_ops_count_per_kind: [205, 1100, 800],
        airport_unique_ga_arr_count: 7,
        airport_unique_ga_dep_count: 8,
        airport_unique_ga_ops_count_per_kind: [15, 4, 0],
    }
}

#[test]
fn airport_summary_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("airport_summary.arrow");
    let rows = vec![sample_row()];
    write_airport_summary(&path, &rows).unwrap();
    let read = read_airport_summary(&path).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0], rows[0],
        "every field incl. the GA split must round-trip"
    );
}

#[test]
fn airport_summary_part_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("part.arrow");
    let rows = vec![
        AirportSummaryPartRow {
            airport_key: "LKPR".into(),
            arr_fids: vec![1, 2, 3],
            dep_fids: vec![4, 5],
            gse_fids_per_class: [vec![10, 11], vec![], vec![20]],
            ops_fids_per_kind: [vec![1, 2, 3], vec![1, 2, 3, 4], vec![]],
            ga_arr_fids: vec![900, 901],
            ga_dep_fids: vec![902],
            ga_ops_fids_per_kind: [vec![900, 901, 902], vec![], vec![]],
        },
        AirportSummaryPartRow {
            airport_key: "LKKB".into(),
            arr_fids: vec![100],
            dep_fids: vec![],
            gse_fids_per_class: [vec![], vec![], vec![]],
            ops_fids_per_kind: [vec![100], vec![], vec![]],
            ga_arr_fids: vec![],
            ga_dep_fids: vec![],
            ga_ops_fids_per_kind: [vec![], vec![], vec![]],
        },
    ];
    write_airport_summary_part(&path, &rows).unwrap();
    let read = read_airport_summary_part(&path).unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].airport_key, rows[0].airport_key);
    assert_eq!(read[0].arr_fids, rows[0].arr_fids);
    assert_eq!(read[0].dep_fids, rows[0].dep_fids);
    assert_eq!(
        read[0].ga_arr_fids, rows[0].ga_arr_fids,
        "GA arr split round-trips"
    );
    assert_eq!(read[0].ga_dep_fids, rows[0].ga_dep_fids);
    for c in 0..arrow_schemas::NUM_GSE_CLASSES as usize {
        assert_eq!(
            read[0].gse_fids_per_class[c], rows[0].gse_fids_per_class[c],
            "gse[{c}]"
        );
    }
    for k in 0..arrow_schemas::NUM_OPS_KINDS as usize {
        assert_eq!(
            read[0].ops_fids_per_kind[k], rows[0].ops_fids_per_kind[k],
            "ops[{k}]"
        );
        assert_eq!(
            read[0].ga_ops_fids_per_kind[k], rows[0].ga_ops_fids_per_kind[k],
            "ga_ops[{k}]"
        );
    }
    assert_eq!(read[1].airport_key, rows[1].airport_key);
    assert_eq!(read[1].arr_fids, rows[1].arr_fids);
}
