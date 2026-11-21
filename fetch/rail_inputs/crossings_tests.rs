//! Unit tests for FRA crossing verification and parser defect guards.
use super::*;
use crate::year::Input;
use serde_json::json;
use std::io::Write;

const FRA_HEADER: &str = "\
Revision Date,Crossing ID,Crossing Type Code,Crossing Purpose Code,Crossing Position Code,\
Crossing Closed,Whistleban Code,Latitude,Longitude,Total Daylight Thru Trains,\
Total Nighttime Thru Trains,Total Switching Trains,Total Transit Trains,Trains Per Week,\
Number Passenger Train Per Day,Less Than One Passenger Train Per Day,Number Of Main Tracks,\
Type Of Train Service IDs,Maximum Timetable Speed,Typical Maximum Speed Over Crossing,\
Reason Description,Movements Per Day Code\n";

fn parse_fra_rows(rows: &[&str]) -> Vec<Crossing> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fra.csv");
    let mut file = File::create(&path).unwrap();
    file.write_all(FRA_HEADER.as_bytes()).unwrap();
    for row in rows {
        file.write_all(row.as_bytes()).unwrap();
        file.write_all(b"\n").unwrap();
    }
    parse_fra(&path, 111).unwrap()
}

#[test]
fn fra_input_verification_rejects_bad_checksum_and_future_revision() {
    let dir = tempfile::tempdir().unwrap();
    let csv_path = dir.path().join("fra.csv");
    let body = "Revision Date,Crossing ID\n2024-12-31,100001A\n";
    let mut f = File::create(&csv_path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    let sha = format!("{:x}", Sha256::digest(body.as_bytes()));
    let make_spec = |sha256: &str, cutoff: &str| -> Input {
        serde_json::from_value(json!({
            "id": "us-fra-highway-rail-crossing-inventory",
            "kind": "prepare-crossings",
            "region": "US",
            "publisher": "US DOT FRA",
            "url": "https://data.transportation.gov/Railroads/Crossing-Inventory-Data-Form-71-Current/m2f8-22s6",
            "observed": { "start": "2024-12-31", "end": "2024-12-31" },
            "published": null,
            "licence": "Public Domain",
            "public_ok": true,
            "depends_on": [],
            "description": "FRA test",
            "conversion": { "sha256": sha256, "cutoff": cutoff }
        }))
        .unwrap()
    };
    verify_fra_input(&csv_path, &make_spec(&sha, "2024-12-31")).unwrap();
    assert!(
        verify_fra_input(&csv_path, &make_spec(&"0".repeat(64), "2024-12-31"))
            .unwrap_err()
            .contains("checksum")
    );
    assert!(
        verify_fra_input(&csv_path, &make_spec(&sha, "2024-12-30"))
            .unwrap_err()
            .contains("exceeds manifest cutoff")
    );
}

#[test]
fn fra_parser_keeps_blanks_grade_separated_and_closed_out_of_zero_counts() {
    let parsed = parse_fra_rows(&[
        // Defect 1: grade-separated (position 2) with blank/zero counts must never emit.
        "2024-06-01,GRADE2A,3,1,2,No,0,41.0,-87.0,0,0,0,0,0,0,No,2,11,60,60,No Train Traffic,1",
        // Defect 1: blank daily counts must never become zero.
        "2024-06-01,BLANK1A,3,1,1,No,0,41.0,-87.0,,,,,0,0,No,1,11,50,50,Change in Data,",
        // Defect 1: 0/0 without No Train Traffic stays missing, not explicit zero.
        "2024-06-01,UNCNT1A,3,1,1,No,0,41.0,-87.0,0,0,0,0,0,0,No,1,11,50,50,Change in Data,1",
        // Defect 3: real closed row 807808Y with non-Closed Reason Description must drop.
        "2022-06-01,807808Y,2,1,1,Yes,0,46.752819,-117.212256,0,0,0,,,0,,1,11,0,0,Change in Primary Operating RR,",
        // Defect 5: switching-only row must not become zero through traffic.
        "2024-06-01,SWONLY1,3,1,1,No,0,41.0,-87.0,0,0,6,0,0,0,No,1,11,25,25,No Train Traffic,1",
        // Explicit zero: open at-grade with 0 day, 0 night, 0 switching, 0 transit, No Train Traffic.
        "2024-06-01,ZERO01A,3,1,1,No,0,41.0,-87.0,0,0,0,0,0,0,No,1,11,25,25,No Train Traffic,1",
    ]);
    assert_eq!(parsed.len(), 1);
    let zero = &parsed[0];
    assert_eq!(zero.id, "ZERO01A");
    assert_eq!(zero.soundings, [0.0; 3]);
    assert_eq!(zero.total_periods, Some([0.0; 3]));
    assert_eq!(zero.passenger_daily, Some(0.0));
    assert_eq!(zero.main_tracks, Some(1));
    assert!(zero.freight_only);
}

#[test]
fn fra_parser_reads_weekly_trains_and_passenger_only_and_pax_exceeding_thru() {
    let parsed = parse_fra_rows(&[
        // Defect 2: real weekly-only private crossing 155954B (2 trains/week, no horn).
        "2023-10-24,155954B,2,1,1,No,0,36.699367,-83.287722,0,0,0,0,2,0,No,1,11,20,20,Change in Data,1",
        // Defect 4: real Amtrak row 545244R (thru=0, pax=10, intercity service 12).
        "2015-12-25,545244R,2,1,1,No,0,42.294903,-84.084198,0,0,0,,,10,,1,12,79,79,Change in Primary Operating RR,",
        // Defect 4: pax exceeds daylight+nighttime thru (1 day thru, 2 pax, mixed 11+12).
        "2024-05-01,003260D,3,1,1,No,0,38.0,-90.0,1,0,0,0,0,2,No,1,\"11,12\",79,79,Change in Data,1",
        // Passenger-only service (Metro-North Harlem 500532J: svc 13, no freight 11, dn=52, pax=41).
        "2024-05-01,500532J,3,1,1,No,0,41.4,-73.6,28,24,0,0,0,41,No,1,13,60,60,Change in Data,1",
    ]);
    assert_eq!(parsed.len(), 4);

    let weekly = &parsed[0];
    assert_eq!(weekly.id, "155954B");
    assert_eq!(weekly.soundings, [0.0; 3]);
    let weekly_total: f64 = weekly.total_periods.unwrap().iter().sum();
    assert!((weekly_total - 2.0 / 7.0).abs() < 1e-9);
    assert_eq!(weekly.passenger_daily, Some(0.0));
    assert!(weekly.freight_only);

    let amtrak = &parsed[1];
    assert_eq!(amtrak.id, "545244R");
    assert_eq!(amtrak.soundings, [0.0; 3]);
    let amtrak_total: f64 = amtrak.total_periods.unwrap().iter().sum();
    assert!((amtrak_total - 10.0).abs() < 1e-9);
    assert_eq!(amtrak.passenger_daily, Some(10.0));
    assert!(!amtrak.freight_only);

    let pax_gt_dn = &parsed[2];
    assert_eq!(pax_gt_dn.id, "003260D");
    let gt_sum: f64 = pax_gt_dn.total_periods.unwrap().iter().sum();
    assert!((gt_sum - 2.0).abs() < 1e-9);
    assert_eq!(pax_gt_dn.passenger_daily, Some(2.0));
    assert!(!pax_gt_dn.freight_only);

    let mnr = &parsed[3];
    assert_eq!(mnr.id, "500532J");
    let mnr_sum: f64 = mnr.total_periods.unwrap().iter().sum();
    assert!((mnr_sum - 52.0).abs() < 1e-9);
    assert_eq!(mnr.passenger_daily, Some(52.0));
    assert!(!mnr.freight_only);
}

#[test]
fn fra_parser_separates_horns_from_flows_for_quiet_zones_transit_zero_tracks_and_lt1() {
    let parsed = parse_fra_rows(&[
        // Quiet zone: silent horn, keeps full freight flow.
        "2024-05-01,QUIET01,3,1,1,No,1,41.8,-87.9,12,6,0,0,0,0,No,2,11,50,50,Change in Data,1",
        // Transit-only (svc 14): keeps public highway horn, emits no heavy-rail flow.
        "2024-05-01,TRAN01A,3,1,1,No,0,34.0,-118.2,40,20,0,60,0,0,No,2,14,45,45,Change in Data,1",
        // Zero main tracks (siding): keeps public highway horn, emits no main-track flow.
        "2024-05-01,SIDE01A,3,1,1,No,0,41.0,-87.0,4,2,4,0,0,0,No,0,11,15,15,Change in Data,1",
        // LT1=Yes ticked on freight-only (svc 11) means 0 passenger, not 0.5.
        "2024-05-01,FRTLT1A,3,1,1,No,0,41.0,-87.0,10,6,0,0,0,,Yes,1,11,50,50,Change in Data,1",
        // LT1=Yes on mixed freight+intercity (11,12) sets 0.5 passenger/day.
        "2024-05-01,MIXLT1A,3,1,1,No,0,41.0,-87.0,10,6,0,0,0,,Yes,1,\"11,12\",60,60,Change in Data,1",
        // Negative nighttime field (corrupt FRA entry) is treated as missing, not negative.
        "2024-05-01,NEGN01A,3,1,1,No,0,41.0,-87.0,6,-4,0,0,0,0,No,1,11,40,40,Change in Data,1",
    ]);
    assert_eq!(parsed.len(), 6);

    assert_eq!(parsed[0].soundings, [0.0; 3]);
    assert_eq!(parsed[0].total_periods, Some(fra_end_periods(12.0, 6.0)));
    assert_eq!(parsed[0].main_tracks, Some(2));
    assert!(parsed[0].freight_only);

    assert!(parsed[1].soundings.iter().sum::<f64>() > 59.9);
    assert_eq!(parsed[1].total_periods, None);

    assert!(parsed[2].soundings.iter().sum::<f64>() > 5.9);
    assert_eq!(parsed[2].total_periods, None);

    assert_eq!(parsed[3].passenger_daily, Some(0.0));
    assert!(parsed[3].freight_only);

    assert_eq!(parsed[4].passenger_daily, Some(0.5));
    assert!(!parsed[4].freight_only);

    assert_eq!(parsed[5].total_periods, Some(fra_end_periods(6.0, 0.0)));
}

#[test]
fn tc_parser_stays_horns_only_and_drops_blanks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tc.csv");
    let body = "\
TC Number,Access,Latitude,Longitude,Trains Daily,Train Max Speed (mph)
TC001,Public,43.65,-79.38,-,50
TC002,Public,43.65,-79.38,12,60
";
    File::create(&path)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let parsed = parse_tc(&path, 112).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "TC002");
    assert!((parsed[0].soundings.iter().sum::<f64>() - 12.0).abs() < 1e-9);
    assert_eq!(parsed[0].total_periods, None);
    assert_eq!(parsed[0].passenger_daily, None);
    assert_eq!(parsed[0].main_tracks, None);
    assert!(!parsed[0].freight_only);
}
