//! Verify dated service, authoritative feed windows and seasonal calendars before spatial matching.
use super::*;
#[test]
fn repeated_exception_is_idempotent_but_conflicting_exception_fails() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("calendar_dates.txt");
    let read = || {
        ServiceCalendar::read(
            root.path(),
            compact_date("20240101").unwrap(),
            compact_date("20240103").unwrap(),
        )
    };
    std::fs::write(
        &path,
        "service_id,date,exception_type\ns,20240101,1\ns,20240101,1\n",
    )
    .unwrap();
    assert_eq!(read().unwrap().active_days("s").unwrap(), 1);
    std::fs::write(
        &path,
        "service_id,date,exception_type\ns,20240101,1\ns,20240101,2\n",
    )
    .unwrap();
    assert!(read().unwrap_err().contains("contradictory"));
}
#[test]
fn full_window_includes_weekends_leap_day_and_exceptions_without_extrapolation() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("calendar.txt"), "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nwork,1,1,1,1,1,0,0,20240226,20240303\n").unwrap();
    std::fs::write(root.path().join("calendar_dates.txt"), "service_id,date,exception_type\nwork,20240229,2\nwork,20240302,1\nextra,20240229,1\nextra,20240310,1\n").unwrap();
    let c = ServiceCalendar::read(
        root.path(),
        compact_date("20240226").unwrap(),
        compact_date("20240304").unwrap(),
    )
    .unwrap();
    assert_eq!(c.days.len(), 7);
    assert_eq!(c.active_days("work").unwrap(), 5);
    assert_eq!(c.active_days("extra").unwrap(), 1);
    assert_eq!(c.annual_departures("extra", 3.0).unwrap(), 3.0 / 7.0);
    assert_eq!(
        c.coverage_for(c.active.keys().map(String::as_str))
            .unwrap()
            .covered_days(),
        7
    );
    assert!(c.active_days("missing").is_err());
}
#[test]
fn exception_only_calendars_do_not_claim_coverage_of_missing_dates() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        ServiceCalendar::read(
            root.path(),
            compact_date("20240101").unwrap(),
            compact_date("20250101").unwrap()
        )
        .is_err()
    );
    std::fs::write(
        root.path().join("calendar_dates.txt"),
        "service_id,date,exception_type\ns,20240229,1\ns,20240301,2\n",
    )
    .unwrap();
    let c = ServiceCalendar::read(
        root.path(),
        compact_date("20240101").unwrap(),
        compact_date("20250101").unwrap(),
    )
    .unwrap();
    assert_eq!(c.days.len(), 366);
    assert_eq!(c.active_days("s").unwrap(), 1);
    assert_eq!(
        c.coverage_for(c.active.keys().map(String::as_str))
            .unwrap()
            .covered_days(),
        1
    );
}
#[test]
fn feed_info_limits_authoritative_days_and_asserts_omitted_days_are_zero() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("calendar_dates.txt"),
        "service_id,date,exception_type\ns,20240101,1\ns,20240103,1\ns,20240105,1\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("feed_info.txt"),
        "feed_start_date,feed_end_date\n20240102,20240104\n",
    )
    .unwrap();
    let calendar = ServiceCalendar::read(
        root.path(),
        compact_date("20240101").unwrap(),
        compact_date("20240106").unwrap(),
    )
    .unwrap();
    let coverage = calendar
        .coverage_for(calendar.active.keys().map(String::as_str))
        .unwrap();
    assert_eq!(
        (0..5)
            .map(|index| coverage.contains(index))
            .collect::<Vec<_>>(),
        [false, true, true, true, false]
    );
    assert_eq!(calendar.annual_departures("s", 1.0).unwrap(), 1.0 / 5.0);
}

#[test]
fn seasonal_patterns_share_one_feed_window_but_future_only_paths_do_not_assert_zero() {
    let root = tempfile::tempdir().unwrap();
    for (name, text) in [
        (
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nearly,1,1,1,1,1,1,1,20240101,20240102\nlate,1,1,1,1,1,1,1,20240103,20240104\nfuture,1,1,1,1,1,1,1,20240110,20240120\ncancelled,0,0,0,0,0,0,0,20240101,20240104\n",
        ),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\nfuture-date,20240110,1\n",
        ),
        ("routes.txt", "route_id,route_type\nr,2\n"),
        (
            "trips.txt",
            "route_id,service_id,trip_id\nr,early,a\nr,late,b\nr,future,future\nr,future-date,future-date\nr,cancelled,cancelled\n",
        ),
        (
            "stops.txt",
            "stop_id,stop_lat,stop_lon\na,50,14\nb,50.1,14.1\nc,51,15\nd,51.1,15.1\ne,52,16\nf,52.1,16.1\n",
        ),
        (
            "stop_times.txt",
            "trip_id,stop_id,stop_sequence\na,a,1\na,b,2\nb,a,1\nb,b,2\nfuture,c,1\nfuture,d,2\nfuture-date,c,1\nfuture-date,d,2\ncancelled,e,1\ncancelled,f,2\n",
        ),
    ] {
        std::fs::write(root.path().join(name), text).unwrap();
    }
    let options = super::super::gtfs::Options {
        identity: "fixture",
        source_id: 1,
        country: "CZ",
        rail_types: &[2],
        tram_types: &[],
        excluded_names: &[],
        excluded_agency_names: &[],
        ignore_feed_window: false,
        frozen_comparison: false,
    };
    let convert = || {
        super::super::gtfs::convert(
            root.path(),
            &options,
            compact_date("20240101").unwrap(),
            compact_date("20240105").unwrap(),
        )
        .unwrap()
    };
    let mut rows = convert();
    assert_eq!(rows.len(), 2);
    let seasonal = rows.iter_mut().find(|row| row.id == "fixture/a").unwrap();
    seasonal.estimate_missing_calendar();
    assert_eq!(seasonal.passenger, Some(1.0));
    assert_eq!(seasonal.missing_calendar_estimate, [0.0; 2]);
    assert_eq!(
        rows.iter()
            .find(|row| row.id == "fixture/cancelled")
            .unwrap()
            .passenger,
        Some(0.0)
    );
    std::fs::write(
        root.path().join("feed_info.txt"),
        "feed_start_date,feed_end_date\n20240101,20240104\n",
    )
    .unwrap();
    let complete = convert();
    assert_eq!(complete.len(), 3);
    assert_eq!(
        complete
            .iter()
            .find(|row| row.id.starts_with("fixture/future"))
            .unwrap()
            .passenger,
        Some(0.0)
    );
}
