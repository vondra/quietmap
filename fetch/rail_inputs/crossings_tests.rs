//! Unit tests for FRA crossing verification and parser defect guards.
use super::*;
use crate::year::Input;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Write};

const FRA_HEADER: &str = "\
Revision Date,Crossing ID,Crossing Type Code,Crossing Purpose Code,Crossing Position Code,\
Crossing Closed,Whistleban Code,Latitude,Longitude,Total Daylight Thru Trains,\
Total Nighttime Thru Trains,Total Switching Trains,Total Transit Trains,Trains Per Week,\
Number Passenger Train Per Day,Less Than One Passenger Train Per Day,Number Of Main Tracks,\
Type Of Train Service IDs,Maximum Timetable Speed,Typical Maximum Speed Over Crossing,\
Reason Description,Movements Per Day Code\n";

fn parse_fra_rows(rows: &[&str]) -> BTreeMap<String, Crossing> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fra.csv");
    let mut file = File::create(&path).unwrap();
    file.write_all(FRA_HEADER.as_bytes()).unwrap();
    for row in rows {
        file.write_all(row.as_bytes()).unwrap();
        file.write_all(b"\n").unwrap();
    }
    parse_fra(&path, 111)
        .unwrap()
        .into_iter()
        .map(|crossing| (crossing.id.clone(), crossing))
        .collect()
}

#[test]
fn fra_input_verification_rejects_bad_checksum_and_future_revision() {
    let dir = tempfile::tempdir().unwrap();
    let csv_path = dir.path().join("fra.csv");
    let body = format!(
        "{FRA_HEADER}2024-12-31,100001A,3,1,1,No,0,41.0,-87.0,10,4,0,0,0,0,No,1,11,50,50,Change in Data,2\n"
    );
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
    let rows = parse_verified_fra(&csv_path, &make_spec(&sha, "2024-12-31"), 111).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        parse_verified_fra(&csv_path, &make_spec(&"0".repeat(64), "2024-12-31"), 111)
            .unwrap_err()
            .contains("checksum")
    );
    assert!(
        parse_verified_fra(&csv_path, &make_spec(&sha, "2024-12-30"), 111)
            .unwrap_err()
            .contains("exceeds manifest cutoff")
    );
}

#[test]
fn fra_parser_keeps_blanks_grade_separated_closed_and_negative_half_day_out_of_zero() {
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
        // Real 020355F: switching + weekly count is switching traffic, never through freight.
        "2024-06-01,020355F,3,1,1,No,0,34.0,-118.0,0,0,10,0,50,0,No,1,11,25,25,Change in Data,2",
        // Real 807483S: corrupt negative nighttime (-4) must not fabricate 4/day with 0 night.
        "2024-06-01,807483S,3,1,1,No,0,41.0,-87.0,4,-4,0,0,0,0,No,1,11,40,40,Change in Data,2",
        // Blank half-day (day=4, night blank) must not fabricate a measured 4/day with 0 night.
        "2024-06-01,BLKHLF1,3,1,1,No,0,41.0,-87.0,4,,0,0,0,0,No,1,11,40,40,Change in Data,2",
        // Real 097085Y: Movements Per Day Code 1 (<1/day) with 999/week must produce neither flow nor horn.
        "2024-06-01,097085Y,3,1,1,No,0,41.0,-87.0,0,0,0,0,999,0,No,1,11,40,40,Change in Data,1",
        // Explicit zero: open at-grade with 0 day, 0 night, 0 switching, 0 transit, No Train Traffic.
        "2024-06-01,ZERO01A,3,1,1,No,0,41.0,-87.0,0,0,0,0,0,0,No,1,11,25,25,No Train Traffic,1",
    ]);
    assert_eq!(parsed.len(), 1);
    let zero = &parsed["ZERO01A"];
    assert_eq!(zero.soundings, [0.0; 3]);
    assert_eq!(zero.total_periods, Some([0.0; 3]));
    assert_eq!(zero.passenger_daily, Some(0.0));
    assert!(!zero.passenger_lt1);
    assert!(zero.measured_periods);
}

#[test]
fn fra_parser_reads_weekly_trains_horns_passenger_only_and_mixed_transit() {
    let parsed = parse_fra_rows(&[
        // Defect 2: real weekly-only private crossing 155954B (2 trains/week, no horn, unsplit).
        "2023-10-24,155954B,2,1,1,No,0,36.699367,-83.287722,0,0,0,0,2,0,No,1,11,20,20,Change in Data,1",
        // Real weekly-only public crossing 004896S: sounds weekly horn + unsplit weekly flow.
        "2024-05-01,004896S,3,1,1,No,0,38.0,-90.0,0,0,0,0,4,0,No,1,11,25,25,Change in Data,1",
        // Defect 4: real Amtrak row 545244R (thru=0, pax=10, intercity service 12).
        "2015-12-25,545244R,2,1,1,No,0,42.294903,-84.084198,0,0,0,,,10,,1,12,79,79,Change in Primary Operating RR,",
        // Real 058046C: public unbanned, dn=0, pax=1, svc=16 -> 1 flow AND 1 sounding/day.
        "2024-05-01,058046C,3,1,1,No,0,40.0,-80.0,0,0,0,,,1,,1,16,30,30,Change in Data,",
        // Defect 4: real 003260D: public unbanned, 1 day thru, 2 pax -> 2 flow AND 2 soundings/day.
        "2024-05-01,003260D,3,1,1,No,0,38.0,-90.0,1,0,0,0,0,2,No,1,\"11,12\",79,79,Change in Data,1",
        // Partial whistleban (code 2): scales evening to 3/4 and zeroes night on accepted flow.
        "2024-05-01,PARTBN2,3,1,1,No,2,38.0,-90.0,12,12,0,0,0,0,No,1,11,50,50,Change in Data,2",
        // Passenger-only service (Metro-North Harlem 500532J: svc 13, no freight 11, dn=52, pax=41).
        "2024-05-01,500532J,3,1,1,No,0,41.07,-73.52,28,24,0,0,0,41,No,1,13,60,60,Change in Data,2",
        // Real 067230N: svc=12, dn=23, pax blank, LT1=Yes -> 23 total, unknown pax with LT1 bound.
        "2024-05-01,067230N,3,1,1,No,1,41.0,-87.0,12,11,0,0,0,,Yes,2,12,60,60,Change in Data,2",
        // Real 415237X & 415240F: svc=15 only, dn=4, transit=2, pax=0 -> 4 total, 0 heavy pax.
        "2024-05-01,415237X,3,1,1,No,0,41.0,-87.0,1,3,7,2,0,0,No,1,15,40,40,Change in Data,2",
        "2024-05-01,415240F,3,1,1,No,0,41.0,-87.0,1,3,7,2,0,0,No,1,15,40,40,Change in Data,2",
        // Real 943872U: svc=11,15, dn=2, pax blank, LT1=Yes -> freight-only (0 heavy pax).
        "2024-05-01,943872U,2,2,1,No,0,41.0,-87.0,0,2,0,0,0,,Yes,1,\"11,15\",40,40,Admin Correction,2",
        // Real San Diego 026852D: 53 heavy thru + 150 transit, pax=196 -> 46 heavy pax, 7 freight.
        "2024-05-01,026852D,3,1,1,No,0,32.755,-117.200,30,23,0,150,0,196,No,2,\"11,12,13,14\",79,79,Change in Data,2",
        // Real Utah 255374L: 4 night freight + 140 transit, pax=140 -> 0 heavy pax, 4 freight.
        "2024-05-01,255374L,3,1,1,No,0,40.573,-112.021,0,4,0,140,0,140,No,1,\"11,14\",40,40,Change in Data,2",
        // Real San Diego 967101J: 44 thru + 132 transit, pax=182 (182-132=50 > 44) -> unknown split.
        "2024-05-01,967101J,3,1,1,No,0,32.717,-117.170,24,20,0,132,0,182,No,2,\"11,12,13,14\",60,60,Change in Data,2",
        // Real San Diego Trolley 661798L: 209 thru == 209 transit == 209 pax (svc 11,14) -> no heavy-rail flow.
        "2024-05-01,661798L,3,1,1,No,0,32.698,-117.147,115,94,0,209,0,209,No,2,\"11,14\",55,55,Admin Correction,2",
        // Real 661803F: 210 thru, 209 transit, 209 pax (svc 11,14) -> suppress transit-contaminated flow.
        "2024-05-01,661803F,3,1,1,No,0,32.699,-117.147,115,95,0,209,0,209,No,2,\"11,14\",55,55,Admin Correction,2",
        // Real 661802Y: 209 thru, 215 transit, 209 pax (svc 11,14) -> dn < transit yet still transit-contaminated.
        "2024-05-01,661802Y,3,1,1,No,0,32.692,-117.134,115,94,0,215,0,209,No,2,\"11,14\",55,55,Admin Correction,2",
        // Real 661929M: 146 thru, 147 transit, 147 pax (svc 11,14) -> 1-off night count still transit-contaminated.
        "2024-05-01,661929M,3,1,1,No,0,32.705,-117.155,85,61,0,147,0,147,No,2,\"11,14\",55,55,Admin Correction,2",
        // Real Wood Dale IL 372184D: 60 thru, 70 transit, 70 pax without svc 14/15 -> keep 60 total, unknown split.
        "2024-05-01,372184D,3,1,1,No,0,41.975,-88.032,42,18,4,70,0,70,No,2,\"11,13\",70,70,Change in Data,2",
        // Real Santa Fe Southern 013800Y: 1/week, svc 11+16, LT1=Yes -> unknown passenger split with LT1 bound.
        "2024-05-01,013800Y,3,1,1,No,0,35.6,-106.0,0,0,0,0,1,,Yes,1,\"11,16\",25,25,Change in Data,1",
        // Real 028285V: 18 thru, svc 11+12, LT1=Yes -> unknown passenger split with LT1 bound.
        "2024-05-01,028285V,3,1,1,No,1,41.0,-95.0,9,9,0,0,0,,Yes,2,\"11,12\",60,60,Change in Data,2",
    ]);
    assert_eq!(parsed.len(), 21);

    let weekly = &parsed["155954B"];
    assert_eq!(weekly.soundings, [0.0; 3]);
    assert!((weekly.total_periods.unwrap().iter().sum::<f64>() - 2.0 / 7.0).abs() < 1e-9);
    assert_eq!(weekly.passenger_daily, Some(0.0));
    assert!(!weekly.measured_periods);

    let pub_wk = &parsed["004896S"];
    assert!((pub_wk.soundings.iter().sum::<f64>() - 4.0 / 7.0).abs() < 1e-9);
    assert!((pub_wk.total_periods.unwrap().iter().sum::<f64>() - 4.0 / 7.0).abs() < 1e-9);
    assert!(!pub_wk.measured_periods);

    let amtrak = &parsed["545244R"];
    assert_eq!(amtrak.soundings, [0.0; 3]);
    assert!((amtrak.total_periods.unwrap().iter().sum::<f64>() - 10.0).abs() < 1e-9);
    assert_eq!(amtrak.passenger_daily, Some(10.0));
    assert!(!amtrak.measured_periods);

    let pax_only_horn = &parsed["058046C"];
    assert!((pax_only_horn.total_periods.unwrap().iter().sum::<f64>() - 1.0).abs() < 1e-9);
    assert_eq!(pax_only_horn.passenger_daily, Some(1.0));
    assert!((pax_only_horn.soundings.iter().sum::<f64>() - 1.0).abs() < 1e-9);

    let pax_gt_dn = &parsed["003260D"];
    assert!((pax_gt_dn.total_periods.unwrap().iter().sum::<f64>() - 2.0).abs() < 1e-9);
    assert_eq!(pax_gt_dn.passenger_daily, Some(2.0));
    assert!((pax_gt_dn.soundings.iter().sum::<f64>() - 2.0).abs() < 1e-9);
    assert!(pax_gt_dn.measured_periods);

    let partial = &parsed["PARTBN2"];
    assert!((partial.soundings[0] - 12.0).abs() < 1e-9);
    assert!((partial.soundings[1] - 3.0).abs() < 1e-9);
    assert_eq!(partial.soundings[2], 0.0);

    let mnr = &parsed["500532J"];
    assert!((mnr.total_periods.unwrap().iter().sum::<f64>() - 52.0).abs() < 1e-9);
    assert_eq!(mnr.passenger_daily, Some(52.0));

    let lt1_svc12 = &parsed["067230N"];
    assert!((lt1_svc12.total_periods.unwrap().iter().sum::<f64>() - 23.0).abs() < 1e-9);
    assert_eq!(lt1_svc12.passenger_daily, None);
    assert!(lt1_svc12.passenger_lt1);

    for id in ["415237X", "415240F"] {
        let row = &parsed[id];
        assert!((row.total_periods.unwrap().iter().sum::<f64>() - 4.0).abs() < 1e-9);
        assert_eq!(row.passenger_daily, Some(0.0));
        assert!(!row.passenger_lt1);
    }

    let frt_shared_transit = &parsed["943872U"];
    assert!(
        (frt_shared_transit
            .total_periods
            .unwrap()
            .iter()
            .sum::<f64>()
            - 2.0)
            .abs()
            < 1e-9
    );
    assert_eq!(frt_shared_transit.passenger_daily, Some(0.0));
    assert!(!frt_shared_transit.passenger_lt1);

    let sd = &parsed["026852D"];
    assert!((sd.total_periods.unwrap().iter().sum::<f64>() - 53.0).abs() < 1e-9);
    assert_eq!(sd.passenger_daily, Some(46.0));
    assert!(sd.measured_periods);

    let ut = &parsed["255374L"];
    assert!((ut.total_periods.unwrap().iter().sum::<f64>() - 4.0).abs() < 1e-9);
    assert_eq!(ut.passenger_daily, Some(0.0));

    let irrec = &parsed["967101J"];
    assert!((irrec.total_periods.unwrap().iter().sum::<f64>() - 44.0).abs() < 1e-9);
    assert_eq!(irrec.passenger_daily, None);

    let trolley = &parsed["661798L"];
    assert_eq!(trolley.total_periods, None);
    assert_eq!(trolley.passenger_daily, None);
    assert!((trolley.soundings.iter().sum::<f64>() - 209.0).abs() < 1e-9);

    for (id, expected_horn) in [("661803F", 210.0), ("661802Y", 209.0), ("661929M", 146.0)] {
        let transit_contaminated = &parsed[id];
        assert_eq!(transit_contaminated.total_periods, None);
        assert_eq!(transit_contaminated.passenger_daily, None);
        assert!(!transit_contaminated.passenger_lt1);
        assert!((transit_contaminated.soundings.iter().sum::<f64>() - expected_horn).abs() < 1e-9);
    }

    let metra = &parsed["372184D"];
    assert!((metra.total_periods.unwrap().iter().sum::<f64>() - 60.0).abs() < 1e-9);
    assert_eq!(metra.passenger_daily, None);

    let sfs = &parsed["013800Y"];
    assert!((sfs.total_periods.unwrap().iter().sum::<f64>() - 1.0 / 7.0).abs() < 1e-9);
    assert_eq!(sfs.passenger_daily, None);
    assert!(sfs.passenger_lt1);

    let bnsf_lt1 = &parsed["028285V"];
    assert!((bnsf_lt1.total_periods.unwrap().iter().sum::<f64>() - 18.0).abs() < 1e-9);
    assert_eq!(bnsf_lt1.passenger_daily, None);
    assert!(bnsf_lt1.passenger_lt1);
}

#[test]
fn fra_parser_deduplicates_repeated_ids_and_separates_horns_from_flows() {
    let parsed = parse_fra_rows(&[
        // Duplicate ID (as 022128G in 2026 snapshot): newer revision replaces older, votes once.
        "2023-01-01,022128G,3,1,1,No,0,41.0,-87.0,4,2,0,0,0,0,No,1,11,50,50,Change in Data,2",
        "2024-08-01,022128G,3,1,1,No,0,41.0,-87.0,10,8,0,0,0,0,No,1,11,50,50,Change in Data,2",
        // Newer closed duplicate overrides earlier open row.
        "2023-01-01,CLSDUP1,3,1,1,No,0,41.0,-87.0,10,4,0,0,0,0,No,1,11,50,50,Change in Data,2",
        "2024-09-01,CLSDUP1,3,1,1,Yes,0,41.0,-87.0,0,0,0,0,0,0,No,0,11,0,0,Closed,1",
        // Quiet zone: silent horn, keeps full freight flow.
        "2024-05-01,QUIET01,3,1,1,No,1,41.8,-87.9,12,6,0,0,0,0,No,2,11,50,50,Change in Data,2",
        // Transit-only (svc 14): keeps public highway horn, emits no heavy-rail flow.
        "2024-05-01,TRAN01A,3,1,1,No,0,34.0,-118.2,40,20,0,60,0,0,No,2,14,45,45,Change in Data,2",
        // Zero main tracks (siding): keeps public highway horn, emits no main-track flow.
        "2024-05-01,SIDE01A,3,1,1,No,0,41.0,-87.0,4,2,4,0,0,0,No,0,11,15,15,Change in Data,2",
        // LT1=Yes ticked on freight-only (svc 11) means 0 passenger.
        "2024-05-01,FRTLT1A,3,1,1,No,0,41.0,-87.0,10,6,0,0,0,,Yes,1,11,50,50,Change in Data,2",
        // LT1=Yes on mixed freight+intercity (11,12) leaves passenger split unknown.
        "2024-05-01,MIXLT1A,3,1,1,No,0,41.0,-87.0,10,6,0,0,0,,Yes,1,\"11,12\",60,60,Change in Data,2",
    ]);
    assert_eq!(parsed.len(), 6);
    assert!(!parsed.contains_key("CLSDUP1"));
    assert!((parsed["022128G"].total_periods.unwrap().iter().sum::<f64>() - 18.0).abs() < 1e-9);

    assert_eq!(parsed["QUIET01"].soundings, [0.0; 3]);
    assert_eq!(
        parsed["QUIET01"].total_periods,
        Some(fra_end_periods(12.0, 6.0))
    );

    assert!(parsed["TRAN01A"].soundings.iter().sum::<f64>() > 59.9);
    assert_eq!(parsed["TRAN01A"].total_periods, None);

    assert!(parsed["SIDE01A"].soundings.iter().sum::<f64>() > 5.9);
    assert_eq!(parsed["SIDE01A"].total_periods, None);

    assert_eq!(parsed["FRTLT1A"].passenger_daily, Some(0.0));
    assert!(!parsed["FRTLT1A"].passenger_lt1);
    assert_eq!(parsed["MIXLT1A"].passenger_daily, None);
    assert!(parsed["MIXLT1A"].passenger_lt1);
}

#[test]
fn tc_parser_emits_mainline_flows_separates_spur_horns_and_marks_untimetabled_passenger_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tc.csv");
    let body = "\
TC Number,Railway Company,Access,Subdivision,Spur Name,Latitude,Longitude,Trains Daily,Train Max Speed (mph),Tracks
TC001,Canadian National Railway,Public,Kingston - CN,-,43.65,-79.38,-,50,2
TC002,Canadian National Railway,Public,Kingston - CN,-,43.65,-79.38,12,60,2
TC003,Canadian Pacific Railway,Private,Shuswap,-,50.80,-119.30,33,45,1
TC004,Big Sky Railway,Private,Elrose,-,51.20,-108.00,0.14,25,1
TC005,Canadian National Railway,Public,Dundas,Talbot,43.00,-81.20,6,15,1
TC006,Canadian National Railway,Private,Lulu Island Industrial,Queensboro,49.19,-122.94,4,10,1
TC007,Edmonton Transit System,Public,Lrt Line,-,53.57,-113.45,150,35,2
TC008,GO Transit,Public,Oakville - GO,-,43.45,-79.68,110,70,3
TC009,EXO,Public,Parc,-,45.65,-73.85,14,50,1
TC010,Alberta Prairie Railway,Public,Stettler,-,52.32,-112.71,1,25,1
";
    File::create(&path)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let parsed: BTreeMap<String, Crossing> = parse_tc(&path, 112)
        .unwrap()
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    assert_eq!(parsed.len(), 8);
    assert!(!parsed.contains_key("TC001"));
    assert!(!parsed.contains_key("TC006"));

    let cn = &parsed["TC002"];
    assert!((cn.soundings.iter().sum::<f64>() - 12.0).abs() < 1e-9);
    assert!((cn.total_periods.unwrap().iter().sum::<f64>() - 12.0).abs() < 1e-9);
    assert_eq!(cn.passenger_daily, Some(0.0));
    assert!(!cn.measured_periods);

    let cp_priv = &parsed["TC003"];
    assert_eq!(cp_priv.soundings, [0.0; 3]);
    assert!((cp_priv.total_periods.unwrap().iter().sum::<f64>() - 33.0).abs() < 1e-9);
    assert_eq!(cp_priv.passenger_daily, Some(0.0));

    let weekly = &parsed["TC004"];
    assert_eq!(weekly.soundings, [0.0; 3]);
    assert!((weekly.total_periods.unwrap().iter().sum::<f64>() - 0.14).abs() < 1e-9);
    assert_eq!(weekly.passenger_daily, Some(0.0));

    let spur_pub = &parsed["TC005"];
    assert!((spur_pub.soundings.iter().sum::<f64>() - 6.0).abs() < 1e-9);
    assert_eq!(spur_pub.total_periods, None);
    assert_eq!(spur_pub.passenger_daily, None);

    let lrt = &parsed["TC007"];
    assert!((lrt.soundings.iter().sum::<f64>() - 150.0).abs() < 1e-9);
    assert_eq!(lrt.total_periods, None);
    assert_eq!(lrt.passenger_daily, None);

    let go = &parsed["TC008"];
    assert!((go.total_periods.unwrap().iter().sum::<f64>() - 110.0).abs() < 1e-9);
    assert_eq!(go.passenger_daily, Some(0.0));

    let exo = &parsed["TC009"];
    assert!((exo.total_periods.unwrap().iter().sum::<f64>() - 14.0).abs() < 1e-9);
    assert_eq!(exo.passenger_daily, Some(14.0));

    let apr = &parsed["TC010"];
    assert!((apr.total_periods.unwrap().iter().sum::<f64>() - 1.0).abs() < 1e-9);
    assert_eq!(apr.passenger_daily, Some(1.0));
}
