//! Operator horizons remain separate until their observed and estimated departures are known.
use super::{Options, convert};
use crate::railways::movements::Movement;

fn fixture(tram: bool, handover: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let calendars = if handover {
        "a,1,1,1,1,1,1,1,20240101,20240102\nb,1,1,1,1,1,1,1,20240103,20240104\n"
    } else {
        "a,1,1,1,1,1,1,1,20240101,20240101\nb,1,1,1,1,1,1,1,20240101,20240102\n"
    };
    std::fs::write(root.path().join("calendar.txt"), format!("service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n{calendars}")).unwrap();
    std::fs::write(
        root.path().join("routes.txt"),
        format!(
            "route_id,agency_id,route_type\na,short,{}\nb,long,{}\n",
            if tram { 0 } else { 2 },
            if tram { 0 } else { 2 }
        ),
    )
    .unwrap();
    for (name, contents) in [
        (
            "agency.txt",
            "agency_id,agency_name\nshort,Short horizon\nlong,Long horizon\n",
        ),
        ("trips.txt", "route_id,service_id,trip_id\na,a,a\nb,b,b\n"),
        (
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nx,50,14\ny,50.1,14.1\n",
        ),
        (
            "stop_times.txt",
            "trip_id,stop_id,stop_sequence\na,x,1\na,y,2\nb,x,1\nb,y,2\n",
        ),
    ] {
        std::fs::write(root.path().join(name), contents).unwrap();
    }
    root
}

fn movements(root: &std::path::Path, ignore_feed_window: bool) -> Vec<Movement> {
    let options = Options {
        identity: "fixture",
        source_id: 1,
        country: "NO",
        rail_types: &[2],
        tram_types: &[0],
        excluded_names: &[],
        excluded_agency_names: &[],
        ignore_feed_window,
        frozen_comparison: false,
    };
    let mut rows = convert(
        root,
        &options,
        super::super::calendar::compact_date("20240101").unwrap(),
        super::super::calendar::compact_date("20240105").unwrap(),
    )
    .unwrap();
    for row in &mut rows {
        row.estimate_missing_calendar();
    }
    rows
}

#[test]
fn different_operator_horizons_survive_shared_train_patterns_and_tram_stops() {
    for tram in [false, true] {
        let root = fixture(tram, false);
        let rows = movements(root.path(), false);
        assert_eq!(rows.len(), if tram { 4 } else { 2 });
        assert_eq!(
            rows.iter()
                .map(|row| &row.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            rows.len()
        );
        let factor = if tram { 2.0 } else { 1.0 };
        assert_eq!(
            rows.iter().map(|row| row.passenger.unwrap()).sum::<f64>(),
            0.75 * factor
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.missing_calendar_estimate[0])
                .sum::<f64>(),
            1.25 * factor
        );
        assert!(
            rows.iter()
                .all(|row| [1, 2].contains(&row.coverage.covered_days()))
        );
        std::fs::write(
            root.path().join("feed_info.txt"),
            "feed_start_date,feed_end_date\n20240101,20240104\n",
        )
        .unwrap();
        let ignored = movements(root.path(), true);
        assert_eq!(
            ignored
                .iter()
                .map(|row| row.missing_calendar_estimate[0])
                .sum::<f64>(),
            1.25 * factor
        );
    }
}

#[test]
fn a_complete_published_window_preserves_a_concession_handover_without_double_counting() {
    let root = fixture(false, true);
    std::fs::write(
        root.path().join("feed_info.txt"),
        "feed_start_date,feed_end_date\n20240101,20240104\n",
    )
    .unwrap();
    let rows = movements(root.path(), false);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().map(|row| row.passenger.unwrap()).sum::<f64>(),
        1.0
    );
    assert!(
        rows.iter().all(
            |row| row.missing_calendar_estimate == [0.0; 2] && row.coverage.covered_days() == 4
        )
    );
}
