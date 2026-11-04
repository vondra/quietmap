//! Check raw CIS bitmap integration, incomplete coverage and archive-order-independent zero services.
use std::{fs::File, io::Write};

#[test]
fn terminal_location_can_omit_the_paths_traffic_classification() {
    let document = roxmltree::Document::parse(
        "<path><CZPTTLocation><TrafficType>C3</TrafficType></CZPTTLocation><CZPTTLocation/></path>",
    )
    .unwrap();
    super::validate_passenger_path(document.root_element()).unwrap();
    let absent = roxmltree::Document::parse("<path><CZPTTLocation/></path>").unwrap();
    assert!(super::validate_passenger_path(absent.root_element()).is_err());
}

#[test]
fn every_path_uses_the_complete_window_regardless_of_archive_order() {
    let root = tempfile::tempdir().unwrap();
    let station_path = root.path().join("stations");
    std::fs::write(
        &station_path,
        r#"{"A":{"lat":50,"lon":14},"B":{"lat":50,"lon":15}}"#,
    )
    .unwrap();
    let entries = [
        ("outside.xml", "2025-02-27", "2025-03-02", "1111"),
        ("inside.xml", "2024-02-27", "2024-03-01", "1010"),
        ("zero.xml", "2024-02-27", "2024-03-01", "0000"),
    ];
    for order in [[0, 1, 2], [2, 1, 0]] {
        let archive_path = root.path().join("timetable");
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        for index in order {
            let (name, first, last, bitmap) = entries[index];
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            write!(archive, "<CZPTTCISMessage><PlannedCalendar><StartDateTime>{first}</StartDateTime><EndDateTime>{last}</EndDateTime><BitmapDays>{bitmap}</BitmapDays></PlannedCalendar><CZPTTLocation><LocationPrimaryCode>a</LocationPrimaryCode><PrimaryLocationName>A</PrimaryLocationName><TrafficType>11</TrafficType></CZPTTLocation><CZPTTLocation><LocationPrimaryCode>b</LocationPrimaryCode><PrimaryLocationName>B</PrimaryLocationName></CZPTTLocation></CZPTTCISMessage>").unwrap();
        }
        archive.finish().unwrap();
        let rows = super::convert(
            &archive_path,
            &station_path,
            super::date("2024-02-26").unwrap(),
            super::date("2024-03-04").unwrap(),
            1,
            "CZ",
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .find(|row| row.id.starts_with("zero"))
                .unwrap()
                .passenger,
            Some(0.0)
        );
        let inside = rows
            .iter()
            .find(|row| row.id.starts_with("inside"))
            .unwrap();
        assert_eq!(inside.passenger, Some(2.0 / 7.0));
        assert_eq!(inside.coverage.missing_fraction(), 3.0 / 7.0);
    }
}

#[test]
fn unresolved_cis_paths_keep_their_count_and_resolved_timing_points_stay_ordered() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stations");
    std::fs::write(
        &path,
        r#"{"A":{"lat":50,"lon":14},"B":{"lat":50,"lon":15}}"#,
    )
    .unwrap();
    let sequence = |id: &str, names: &[&str]| super::Sequence {
        id: id.into(),
        passenger: Some(0.5),
        freight: None,
        stops: names
            .iter()
            .map(|name| super::NamedStop {
                code: (*name).into(),
                name: (*name).into(),
            })
            .collect(),
    };
    let rows = super::pairs(
        vec![
            sequence("unlocated", &["A", "unknown"]),
            sequence("resolved", &["A", "unknown", "B"]),
        ],
        &super::Stations::read(&path).unwrap(),
        Default::default(),
        1,
        "CZ",
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].passenger, Some(0.5));
    assert!(matches!(
        rows[0].location,
        super::Location::Unlocated { .. }
    ));
    let super::Location::StationPair { from, to } = &rows[1].location else {
        panic!("resolved pair")
    };
    assert_eq!((from.longitude, to.longitude), (14.0, 15.0));
}
