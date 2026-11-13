//! Regression coverage for frequency boundaries and publisher CSV normalization.

#[test]
fn a_departure_before_the_exclusive_end_is_not_rounded_away() {
    assert_eq!(super::frequency_departures(0, 601, 300), 3);
    assert_eq!(super::frequency_departures(0, 600, 300), 2);
}

#[test]
fn missing_shapes_stay_unknown_and_identical_vertices_do_not_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let required = ["route".to_owned()].into_iter().collect();
    let read = || super::super::gtfs_tables::read_shapes(directory.path(), &required);
    assert!(read().unwrap().is_empty());
    let path = directory.path().join("shapes.txt");
    std::fs::write(
        &path,
        "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nother,50,14,1\n",
    )
    .unwrap();
    assert!(read().unwrap().is_empty());
    std::fs::write(
        &path,
        "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nroute,50,14,1\nroute,50,14,1\n",
    )
    .unwrap();
    assert_eq!(read().unwrap()["route"].len(), 2);
    std::fs::write(
        &path,
        "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nroute,50,14,1\nroute,51,15,1\n",
    )
    .unwrap();
    assert!(read().unwrap_err().contains("repeated sequence"));
}
#[test]
fn padded_tram_csv_without_sequences_keeps_every_departure() {
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("routes.txt", "route_id, route_type\nr,0\n"),
        ("trips.txt", "route_id,service_id,trip_id\nr,s,t\n"),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\ns,20240101,1\n",
        ),
        ("stops.txt", "stop_id, stop_lat, stop_lon\np,50,14\n"),
        ("stop_times.txt", "trip_id, stop_id\nt   ,p   \n"),
        (
            "frequencies.txt",
            "trip_id,start_time,end_time,headway_secs\nt,00:00:00,00:10:01,300\n",
        ),
    ] {
        std::fs::write(directory.path().join(name), text).unwrap();
    }
    let options = super::Options {
        identity: "fixture",
        source_id: 1,
        country: "CZ",
        rail_types: &[],
        tram_types: &[0],
        excluded_names: &[],
        excluded_agency_names: &[],
        ignore_feed_window: false,
        frozen_comparison: false,
    };
    let start = super::super::calendar::compact_date("20240101").unwrap();
    let end = super::super::calendar::compact_date("20240103").unwrap();
    let rows = super::convert(directory.path(), &options, start, end).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].passenger, Some(1.5));
    assert_eq!(rows[0].coverage.missing_fraction(), 0.5);
    std::fs::write(
        directory.path().join("stop_times.txt"),
        "trip_id,stop_id\nt,missing\n",
    )
    .unwrap();
    let unlocated = super::convert(directory.path(), &options, start, end).unwrap();
    assert!(matches!(
        unlocated[0].location,
        crate::railways::movements::Location::Unlocated { .. }
    ));
    assert_eq!(unlocated[0].passenger, Some(1.5));
    std::fs::write(
        directory.path().join("routes.txt"),
        "route_id,route_type\nr,2\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("stop_times.txt"),
        "trip_id,stop_id,stop_sequence\nt,p,1\n",
    )
    .unwrap();
    let rail = super::Options {
        rail_types: &[2],
        tram_types: &[],
        ..options
    };
    let unlocated = super::convert(directory.path(), &rail, start, end).unwrap();
    assert!(matches!(
        unlocated[0].location,
        crate::railways::movements::Location::Unlocated { .. }
    ));
    assert_eq!(unlocated[0].passenger, Some(1.5));
    std::fs::write(
        directory.path().join("stop_times.txt"),
        "trip_id,stop_id,stop_sequence\nt,missing,1\nt,p,2\n",
    )
    .unwrap();
    let unlocated = super::convert(directory.path(), &rail, start, end).unwrap();
    assert!(matches!(
        unlocated[0].location,
        crate::railways::movements::Location::Unlocated { .. }
    ));
    assert_eq!(unlocated[0].passenger, Some(1.5));
}

#[test]
fn publisher_codeshares_do_not_remove_distinct_regional_services() {
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        (
            "agency.txt",
            "agency_id,agency_name\nalias,Intercity (Codesharing)\nregional,Regional trains\n",
        ),
        (
            "routes.txt",
            "route_id,agency_id,route_type,route_short_name\na,alias,2,RE17\nb,regional,2,RE17\n",
        ),
        (
            "trips.txt",
            "route_id,service_id,trip_id\na,s,alias\nb,s,original\n",
        ),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\ns,20240101,1\n",
        ),
        (
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nx,50,14\ny,50.1,14.1\n",
        ),
        (
            "stop_times.txt",
            "trip_id,stop_id,stop_sequence\nalias,x,1\nalias,y,2\noriginal,x,1\noriginal,y,2\n",
        ),
    ] {
        std::fs::write(directory.path().join(name), text).unwrap();
    }
    let options = super::Options {
        identity: "fixture",
        source_id: 1,
        country: "DE",
        rail_types: &[2],
        tram_types: &[],
        excluded_names: &[],
        excluded_agency_names: &["Intercity (Codesharing)".into()],
        ignore_feed_window: false,
        frozen_comparison: false,
    };
    let start = super::super::calendar::compact_date("20240101").unwrap();
    let end = super::super::calendar::compact_date("20240103").unwrap();
    let rows = super::convert(directory.path(), &options, start, end).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].passenger, Some(0.5));
    assert_eq!(rows[0].id, "fixture/original");
    let mut writer = Vec::new();
    super::super::write_movement(&mut writer, rows[0].clone()).unwrap();
    let crate::railways::movements::Record::Movement(converted) =
        serde_json::from_slice(&writer).unwrap()
    else {
        panic!("not a movement")
    };
    assert_eq!(converted.passenger, Some(0.5));
    assert_eq!(converted.missing_calendar_estimate, [0.5, 0.0]);
}

#[test]
fn mixed_national_archive_does_not_turn_freight_into_passenger_trains() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../years/2026.json")).unwrap();
    let input = manifest["railways"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["input"] == "fi-timetable-fi-finland-digitraffic")
        .unwrap();
    let selected: Vec<u16> = serde_json::from_value(input["rail_route_types"].clone()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        (
            "routes.txt",
            "route_id,route_type,route_short_name\nfreight,100,T 55345\npassenger,109,T\n",
        ),
        (
            "trips.txt",
            "route_id,service_id,trip_id\nfreight,s,freight\npassenger,s,passenger\n",
        ),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\ns,20240101,1\n",
        ),
        (
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nx,60,25\ny,60.1,25.1\n",
        ),
        (
            "stop_times.txt",
            "trip_id,stop_id,stop_sequence\nfreight,x,1\nfreight,y,2\npassenger,x,1\npassenger,y,2\n",
        ),
    ] {
        std::fs::write(directory.path().join(name), text).unwrap();
    }
    let options = super::Options {
        identity: "fixture",
        source_id: 1,
        country: "FI",
        rail_types: &selected,
        tram_types: &[],
        excluded_names: &[],
        excluded_agency_names: &[],
        ignore_feed_window: false,
        frozen_comparison: false,
    };
    let start = super::super::calendar::compact_date("20240101").unwrap();
    let end = super::super::calendar::compact_date("20240103").unwrap();
    let rows = super::convert(directory.path(), &options, start, end).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].passenger, Some(0.5));
    assert_eq!(rows[0].id, "fixture/passenger");
}

/// A tram stop's departures count at its station: two directions' platforms of one name 25 m
/// apart are one station (Muni files each direction at its own corner), so are the platforms of
/// one parent; a stop of the same name 2 km away is another, two parents of one name 100 m apart
/// stay two, and a stop whose parent has no position keeps its own.
#[test]
fn tram_departures_count_by_station() {
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("routes.txt", "route_id,route_type\nr,0\n"),
        (
            "trips.txt",
            "route_id,service_id,trip_id\nr,s,north\nr,s,south\nr,s,far\nr,s,in\nr,s,out\nr,s,lone\nr,s,east\nr,s,west\n",
        ),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\ns,20240101,1\n",
        ),
        (
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon,parent_station\n\
             a,Square,50.0000,14.0000,\nb,Square,50.0002,14.0002,\nc,Square,50.0180,14.0000,\n\
             P,Station,51.0000,15.0000,\np1,Platform 1,51.0001,15.0000,P\np2,Platform 2,51.0002,15.0000,P\n\
             Q,Unplaced,,,\nq,Platform,52.0000,16.0000,Q\n\
             H1,Hub,53.0000,17.0000,\nh1,Hub,53.0000,17.0000,H1\nH2,Hub,53.0009,17.0000,\nh2,Hub,53.0009,17.0000,H2\n",
        ),
        (
            "stop_times.txt",
            "trip_id,stop_id\nnorth,a\nsouth,b\nfar,c\nin,p1\nout,p2\nlone,q\neast,h1\nwest,h2\n",
        ),
    ] {
        std::fs::write(directory.path().join(name), text).unwrap();
    }
    let options = super::Options {
        identity: "fixture",
        source_id: 1,
        country: "CZ",
        rail_types: &[],
        tram_types: &[0],
        excluded_names: &[],
        excluded_agency_names: &[],
        ignore_feed_window: false,
        frozen_comparison: false,
    };
    let day = super::super::calendar::compact_date("20240101").unwrap();
    let rows = super::convert(directory.path(), &options, day, day.succ_opt().unwrap()).unwrap();
    let mut stations: Vec<(String, f64)> = rows
        .iter()
        .map(|row| match &row.location {
            crate::railways::movements::Location::Station { stop } => {
                (stop.id.clone(), row.passenger.unwrap())
            }
            other => panic!("{other:?}"),
        })
        .collect();
    stations.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        stations,
        [
            ("H1".into(), 1.0),
            ("H2".into(), 1.0),
            ("P".into(), 2.0),
            ("a".into(), 2.0),
            ("c".into(), 1.0),
            ("q".into(), 1.0)
        ]
    );
}
