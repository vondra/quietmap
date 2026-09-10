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
fn airport_summary_part_preserves_all_membership_bits_and_rejects_invalid_masks() {
    use crate::stage_2c::movements::*;
    let dir = tempdir().unwrap();
    let path = dir.path().join("part.arrow");
    let rows = vec![AirportSummaryPartRow {
        airport_key: "LKPR".into(),
        members: vec![
            (1, AIRPORT_FLAGS),
            (u64::MAX, ARRIVAL | GA_DEPARTURE | GSE[2]),
        ],
    }];
    write_airport_summary_part(&path, &rows).unwrap();
    let actual = read_airport_summary_part(&path).unwrap();
    assert_eq!(actual[0].airport_key, rows[0].airport_key);
    assert_eq!(actual[0].members, rows[0].members);
    write_airport_summary_part(
        &path,
        &[AirportSummaryPartRow {
            airport_key: "A".into(),
            members: vec![(1, NON_GA)],
        }],
    )
    .unwrap();
    assert!(read_airport_summary_part(&path).is_err());
}
