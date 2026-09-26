//! Admission repair: a merged increment candidate that fails admission is
//! rewritten from its primary archive alone, restoring suppressed echoes.

use super::*;
use aircraft_extract::provider_receipt::read_day_receipt;
use std::collections::BTreeMap;

const DAY: &str = "2026-05-01";
const MIDNIGHT: f64 = 1_777_593_600.0;

/// One gzipped `trace_full_<address>.json` member per entry, as in the
/// Stage-0 fixtures: `[offset_s, lat, lon, alt_ft, speed_kt, track_deg,
/// stale, baro_rate_fpm, {flight}]`.
fn write_day_archive(root: &Path, day: &str, traces: &[(String, String)]) {
    let dir = root.join(&day[..4]).join(day);
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join("subset.tar")).unwrap();
    let mut builder = tar::Builder::new(file);
    for (address, json) in traces {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, json.as_bytes()).unwrap();
        let body = encoder.finish().unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("traces/00/trace_full_{address}.json"),
                body.as_slice(),
            )
            .unwrap();
    }
    builder.finish().unwrap();
}

fn trace_json(icao: &str, typecode: &str, callsign: &str, offsets: &[f64]) -> (String, String) {
    let mut samples = Vec::new();
    for (index, offset) in offsets.iter().enumerate() {
        let lon = 14.0 + (offset / 5.0) * 0.001;
        let flight = if index == 0 {
            format!(r#",{{"flight":"{callsign}"}}"#)
        } else {
            String::new()
        };
        samples.push(format!("[{offset},50.5,{lon:.6},2000,200,90,0,0{flight}]"));
    }
    (
        icao.to_string(),
        format!(
            r#"{{"icao":"{icao}","t":"{typecode}","timestamp":{MIDNIGHT},"trace":[{}]}}"#,
            samples.join(",")
        ),
    )
}

/// A primary `~` echo riding a secondary-only address track for 90 s is
/// deleted by the merge; when admission rejects that day, the repair
/// rewrites the flights primary-only and the echo — with its segments —
/// comes back, while the secondary content receipt stays as provenance.
#[test]
fn repair_rewrites_a_rejected_day_primary_only_and_restores_the_echo() {
    let temp = tempfile::tempdir().unwrap();
    let echo_offsets: Vec<f64> = (0..30).map(|k| 1.0 + 3.0 * k as f64).collect();
    let address_offsets: Vec<f64> = (0..60).map(|k| 5.0 * k as f64).collect();
    write_day_archive(
        &temp.path().join("primary"),
        DAY,
        &[trace_json("~4ca011", "B738", "ECHO1", &echo_offsets)],
    );
    write_day_archive(
        &temp.path().join("secondary"),
        DAY,
        &[trace_json("4ca011", "B738", "ADDR1", &address_offsets)],
    );
    let square = grid::square_of(50.5, 14.0);
    let prepared = temp.path().join("prepared");
    let dem_dir = prepared.join(grid::square_name(square));
    std::fs::create_dir_all(&dem_dir).unwrap();
    let dem_len = grid::raster::RasterWindow::for_square(square).cell_count() * 2;
    std::fs::write(dem_dir.join("dem.i16be"), vec![0u8; dem_len]).unwrap();

    let work = temp.path().join("work");
    let flights_dir = work.join("flights");
    let segments_dir = work.join("segments");
    std::fs::create_dir_all(&flights_dir).unwrap();
    std::fs::create_dir_all(&segments_dir).unwrap();
    let primary = AdsbTarSource::new(temp.path().join("primary"));
    let secondary =
        AdsbTarSource::new(temp.path().join("secondary")).with_source_id(source_id::ADSB_EXCHANGE);
    run_stage_0(&primary, Some(&secondary), DAY, &flights_dir, &work, None).unwrap();
    let rasters = RealRasters::new(&prepared);
    run_stage_1(&flights_dir, &segments_dir, DAY, &rasters).unwrap();

    let providers = Providers {
        scope: None,
        primary_root: &temp.path().join("primary"),
        primary_receipts: None,
        secondary_root: None,
        increment_candidates: &BTreeSet::from([DAY.to_string()]),
    };
    let receipts: BTreeMap<String, DayReceipt> = [(
        DAY.to_string(),
        read_day_receipt(&work, DAY).unwrap().unwrap(),
    )]
    .into();
    assert_eq!(receipts[DAY].merge.anonymous_points_suppressed, 30);
    assert_eq!(receipts[DAY].merge.secondary_only_addresses, 1);

    // An admitted increment day is left alone.
    let untouched = repair_rejected_increment_merges(
        &receipts,
        &BTreeSet::from([DAY.to_string()]),
        &providers,
        &work,
        &rasters,
    )
    .unwrap();
    assert!(untouched.is_empty());
    let flights = aircraft_extract::stage_1::read_flights(&flights_dir.join(format!("{DAY}.arrow"))).unwrap();
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].callsign, "ADDR1");

    // A rejected day is rewritten primary-only: the echo flight returns with
    // segments, the merge counts reset, the secondary receipt stays.
    let repaired =
        repair_rejected_increment_merges(&receipts, &BTreeSet::new(), &providers, &work, &rasters)
            .unwrap();
    assert_eq!(repaired, [DAY.to_string()]);
    let flights = aircraft_extract::stage_1::read_flights(&flights_dir.join(format!("{DAY}.arrow"))).unwrap();
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].callsign, "ECHO1");
    let segments =
        aircraft_extract::arrow_io::read_segments(&segments_dir.join(format!("{DAY}.arrow"))).unwrap();
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|s| s.callsign == "ECHO1"));
    let rewritten = read_day_receipt(&work, DAY).unwrap().unwrap();
    assert!(rewritten.secondary.is_some());
    assert_eq!(rewritten.merge.secondary_points_kept, 0);
    assert_eq!(rewritten.merge.secondary_only_addresses, 0);
    assert_eq!(rewritten.merge.anonymous_points_suppressed, 0);

    // The repair is idempotent: the rewritten day needs no second pass.
    let receipts: BTreeMap<String, DayReceipt> =
        [(DAY.to_string(), rewritten)].into();
    let again =
        repair_rejected_increment_merges(&receipts, &BTreeSet::new(), &providers, &work, &rasters)
            .unwrap();
    assert!(again.is_empty());
}
