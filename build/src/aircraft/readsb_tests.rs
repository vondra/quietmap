//! Trace parser cases: surface reports, callsign changes, short rows, geometric altitude, flags.

use super::*;
use crate::aircraft::trace::address_identity;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

pub fn gz(json: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(json.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn a_surface_report_carries_no_altitude() {
    let raw = gz(r#"{"icao":"49d261","t":"PC12","timestamp":1000,"trace":[
            [10,50.0,14.0,"ground",12.0,90.0,0,0],
            [20,50.001,14.001,600.0,80.0,120.0,0,0]
        ]}"#);
    let trace = parse_trace(&raw).unwrap().unwrap();
    assert_eq!(trace.points.len(), 2);
    assert!(trace.points[0].is_surface_report());
    assert!(trace.points[0].altitude_ft.is_nan());
    assert!(trace.points[0].airborne_altitude_ft().is_none());
    assert_eq!(trace.points[1].airborne_altitude_ft(), Some(600.0));
    assert_eq!(trace.points[1].timestamp, 1020.0);
}

#[test]
fn a_trace_with_fewer_than_two_points_is_dropped() {
    let raw = gz(
        r#"{"icao":"abc123","t":"B738","timestamp":1,"trace":[[1,50.0,14.0,1000.0,250.0,90.0,0,0]]}"#,
    );
    assert!(parse_trace(&raw).unwrap().is_none());
}

/// The details object re-appears on many rows: only changes are recorded, trimmed.
#[test]
fn callsign_changes_are_recorded_once_each() {
    let raw = gz(r#"{"icao":"49d328","t":"A320","timestamp":1000,"trace":[
            [10,50.0,14.0,1000.0,250.0,90.0,0,0,null],
            [20,50.001,14.001,1100.0,250.0,90.0,0,0,{"flight":"TVS100P  ","category":"A3"}],
            [30,50.002,14.002,1200.0,250.0,90.0,0,0,{"flight":"TVS100P  "}],
            [40,50.003,14.003,1300.0,250.0,90.0,0,0,{"flight":"TVS200X  "}]
        ]}"#);
    let trace = parse_trace(&raw).unwrap().unwrap();
    let changes: Vec<_> = trace
        .callsigns
        .iter()
        .map(|c| (c.point_index, c.callsign.as_str()))
        .collect();
    assert_eq!(changes, [(1, "TVS100P"), (3, "TVS200X")]);
    assert_eq!(trace.emitter_category, 0xa3);
}

/// Rows of 7 fields (no vertical rate) parse and shorter rows are skipped, as dev4 did; a row that
/// is no array breaks the document.
#[test]
fn short_rows_are_skipped_and_malformed_rows_are_corrupt() {
    let raw = gz(r#"{"icao":"49d262","t":"PC12","timestamp":1000,"trace":[
            [10,50.0,14.0,500.0,80.0,90.0,0],
            [15,50.0,14.0,500.0],
            [20,50.001,14.001,600.0,null,null,0]
        ]}"#);
    let trace = parse_trace(&raw).unwrap().unwrap();
    assert_eq!(trace.points.len(), 2);
    assert_eq!(trace.points[0].vertical_rate_fpm, 0.0);
    assert_eq!(trace.points[1].ground_speed_kt, 0.0);
    assert!(trace.points[0].geometric_altitude_ft.is_nan());
    let junk = gz(
        r#"{"icao":"49d262","timestamp":1000,"trace":[[10,50.0,14.0,500.0,80.0,90.0,0],"junk"]}"#,
    );
    assert!(parse_trace(&junk).is_err());
}

/// Index 10 is geometric altitude on airborne rows; on surface reports readsb repeats a stale
/// value; an altitude field marked geometric (flag bit 3) is both altitudes when 10 is empty.
#[test]
fn geometric_altitude_is_read_on_airborne_rows_only() {
    let raw = gz(r#"{"icao":"4cadef","t":"B38M","timestamp":0,"trace":[
            [0,51.88,0.23,"ground",51.5,202.5,8,-64,null,"adsb_icao",500,null,null,null],
            [5,51.88,0.23,1200,140.0,202.5,4,-64,null,"adsb_icao",1375,null,null,null],
            [9,51.88,0.23,1300,140.0,202.5,8,-64,null,"adsb_icao",null,null,null,null]
        ]}"#);
    let points = parse_trace(&raw).unwrap().unwrap().points;
    assert!(points[0].geometric_altitude_ft.is_nan());
    assert_eq!(
        (points[1].altitude_ft, points[1].geometric_altitude_ft),
        (1200.0, 1375.0)
    );
    assert_eq!(
        (points[2].altitude_ft, points[2].geometric_altitude_ft),
        (1300.0, 1300.0)
    );
}

/// readsb flag bit 0 marks a gap before a point, never the ground.
#[test]
fn the_stale_position_bit_is_not_ground() {
    let raw = gz(r#"{"icao":"49c083","t":"C172","timestamp":0,"trace":[
            [0,50.0,14.0,2000.0,90.0,90.0,0,0],
            [100,50.0,14.01,2000.0,90.0,90.0,1,0],
            [200,50.0,14.02,2000.0,90.0,90.0,3,0]
        ]}"#);
    let points = parse_trace(&raw).unwrap().unwrap().points;
    assert!(points.iter().all(|p| p.flags == 0));
}

#[test]
fn a_document_without_timestamp_or_rows_is_corrupt() {
    assert!(parse_trace(&gz(r#"{"icao":"abc123","trace":[]}"#)).is_err());
    assert!(parse_trace(&gz(r#"{"icao":"abc123","timestamp":1}"#)).is_err());
    assert!(parse_trace(b"not gzip").is_err());
    assert!(parse_trace(&gz("{\"icao\":")).is_err());
}

#[test]
fn addresses_group_by_namespace_and_reject_reserved_values() {
    assert_eq!(address_identity("4b1805"), Some((false, 0x4b1805)));
    assert_eq!(address_identity("~4b1805"), Some((true, 0x4b1805)));
    assert_eq!(address_identity("~ffffff"), Some((true, 0xffffff)));
    for invalid in ["", "ffffff", "000000", "xyz", "1234567"] {
        assert_eq!(address_identity(invalid), None, "{invalid}");
    }
}

#[test]
fn retained_points_carry_their_callsign_changes_forward() {
    let raw = gz(r#"{"icao":"49d328","timestamp":0,"trace":[
            [0,50.0,14.0,1000.0,250.0,90.0,0,0,{"flight":"AAA"}],
            [1,50.0,14.0,1000.0,250.0,90.0,0,0,{"flight":"BBB"}],
            [2,50.0,14.0,1000.0,250.0,90.0,0,0],
            [3,50.0,14.0,1000.0,250.0,90.0,0,0,{"flight":"CCC"}]
        ]}"#);
    let mut trace = parse_trace(&raw).unwrap().unwrap();
    trace.retain_points(|index, _| index != 1 && index != 3);
    let changes: Vec<_> = trace
        .callsigns
        .iter()
        .map(|c| (c.point_index, c.callsign.as_str()))
        .collect();
    assert_eq!(changes, [(0, "AAA"), (1, "BBB")]);
}
