//! Airport identity parts keep every membership bit and reject invalid masks.

use super::*;
use tempfile::tempdir;

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
