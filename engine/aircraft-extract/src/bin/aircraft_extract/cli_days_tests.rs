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
    std::fs::write(dem_dir.join("dem.u16le"), vec![0u8; dem_len]).unwrap();

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

/// Stopping after Stage 1 and resuming at shuffle seals exactly what a
/// continuous run seals: the rejected-increment repair is an admission
/// prerequisite, not a Stage 0/1 step. Three days share one primary `~`
/// echo; the secondary address duplicates it fully on two days and covers
/// only hour 00 on the first, so admission rejects that increment day.
/// Externally reused shards cannot be repaired in place and fail closed.
#[test]
fn resumed_shuffle_repairs_rejected_days_like_a_continuous_run() {
    use crate::cli_run_all::{run_all, RunAllRequest};
    use crate::FromStage;

    const DAYS: [&str; 3] = ["2026-05-01", "2026-05-02", "2026-05-03"];
    const MIDNIGHTS: [f64; 3] = [MIDNIGHT, MIDNIGHT + 86_400.0, MIDNIGHT + 172_800.0];

    fn hourly_trace(
        icao: &str,
        callsign: &str,
        midnight: f64,
        hours: std::ops::Range<u32>,
        lat_offset: f32,
    ) -> (String, String) {
        let mut samples = Vec::new();
        for h in hours {
            for i in 0..7 {
                let offset = f64::from(h * 3600 + i * 10);
                let lon = 14.0 + f64::from(i) * 0.001;
                let flight = if h == 0 && i == 0 {
                    format!(r#",{{"flight":"{callsign}"}}"#)
                } else {
                    String::new()
                };
                samples.push(format!(
                    "[{offset},{},{lon:.6},4000,180,90,0,0{flight}]",
                    50.5 + lat_offset
                ));
            }
        }
        (
            icao.to_string(),
            format!(
                r#"{{"icao":"{icao}","t":"B738","timestamp":{midnight},"trace":[{}]}}"#,
                samples.join(",")
            ),
        )
    }

    fn request(
        primary: &Path,
        secondary: &Path,
        temp: &Path,
        work: &Path,
        from: FromStage,
        until: FromStage,
    ) -> RunAllRequest {
        RunAllRequest {
            primary_cache: primary.into(),
            secondary_cache: Some(secondary.into()),
            prepared_year_dir: temp.join("prepared-year"),
            prepared_dir: temp.join("prepared"),
            work_dir: work.into(),
            reused_segments_dirs: Vec::new(),
            days: DAYS.iter().map(|d| d.to_string()).collect(),
            increment_days: DAYS.iter().map(|d| d.to_string()).collect(),
            scope_bbox: None,
            from_stage: from,
            until_stage: until,
            cruise_phase: aircraft_extract::stage_2b::CruisePhase::All,
            cruise_spill_disk_budget_bytes: None,
        }
    }

    /// Decoded segment rows as order-insensitive comparable tuples.
    fn rows_of(shards: &[std::path::PathBuf]) -> Vec<(u64, String, [u32; 8], u8)> {
        let mut rows = Vec::new();
        for shard in shards {
            for s in aircraft_extract::arrow_io::read_segments(shard).unwrap() {
                rows.push((
                    s.flight_id,
                    s.callsign.clone(),
                    [
                        s.start_lat.to_bits(),
                        s.start_lon.to_bits(),
                        s.start_alt_m.to_bits(),
                        s.end_lat.to_bits(),
                        s.end_lon.to_bits(),
                        s.end_alt_m.to_bits(),
                        s.speed_kt.to_bits(),
                        s.length_m.to_bits(),
                    ],
                    s.flags,
                ));
            }
        }
        rows.sort();
        rows
    }

    fn shards_under(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "arrow") {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    let secondary = temp.path().join("secondary");
    for (day, midnight) in DAYS.iter().zip(MIDNIGHTS) {
        write_day_archive(
            &primary,
            day,
            &[hourly_trace("~4ca011", "ECHO1", midnight, 0..24, 0.0)],
        );
        // The first secondary day covers only hour 00 (rejected); its
        // positions sit 11 m off the primary echo, inside the 450 m match.
        let hours = if *day == DAYS[0] { 0..1 } else { 0..24 };
        write_day_archive(
            &secondary,
            day,
            &[hourly_trace("4ca011", "ADDR1", midnight, hours, 0.0001)],
        );
    }
    let square = grid::square_of(50.5, 14.0);
    let prepared = temp.path().join("prepared");
    let dem_dir = prepared.join(grid::square_name(square));
    std::fs::create_dir_all(&dem_dir).unwrap();
    let dem_len = grid::raster::RasterWindow::for_square(square).cell_count() * 2;
    std::fs::write(dem_dir.join("dem.u16le"), vec![0u8; dem_len]).unwrap();
    std::fs::create_dir_all(temp.path().join("prepared-year")).unwrap();

    // Two-command workflow: Stage 0/1, then a fresh resume at shuffle.
    let resumed = temp.path().join("resumed");
    run_all(request(
        &primary,
        &secondary,
        temp.path(),
        &resumed,
        FromStage::Stage0,
        FromStage::Stage1,
    ))
    .unwrap();
    run_all(request(
        &primary,
        &secondary,
        temp.path(),
        &resumed,
        FromStage::Shuffle,
        FromStage::Shuffle,
    ))
    .unwrap();
    // Continuous workflow for the same inputs.
    let continuous = temp.path().join("continuous");
    run_all(request(
        &primary,
        &secondary,
        temp.path(),
        &continuous,
        FromStage::Stage0,
        FromStage::Shuffle,
    ))
    .unwrap();

    for day in DAYS {
        let file = format!("{day}.arrow");
        let a = rows_of(&[resumed.join("segments").join(&file)]);
        let b = rows_of(&[continuous.join("segments").join(&file)]);
        assert_eq!(a, b, "day shard {day}");
    }
    let sealed_a = rows_of(&shards_under(&resumed.join("segments_by_square/z9")));
    let sealed_b = rows_of(&shards_under(&continuous.join("segments_by_square/z9")));
    assert_eq!(sealed_a, sealed_b);
    assert_eq!(sealed_a.len(), 432, "3 days × 144 hourly segments");

    // The rejected day's resumed shard is primary-only, like the repair's.
    let rejected =
        aircraft_extract::arrow_io::read_segments(&resumed.join("segments/2026-05-01.arrow"))
            .unwrap();
    assert!(rejected.iter().all(|s| !s.is_secondary_only()));
    assert!(rejected.iter().all(|s| s.callsign == "ECHO1"));

    // Externally reused shards fail closed while they hold rejected content.
    let external = temp.path().join("external");
    run_all(request(
        &primary,
        &secondary,
        temp.path(),
        &external,
        FromStage::Stage0,
        FromStage::Stage1,
    ))
    .unwrap();
    let mut reuse = request(
        &primary,
        &secondary,
        temp.path(),
        &temp.path().join("reuse"),
        FromStage::Shuffle,
        FromStage::Shuffle,
    );
    reuse.reused_segments_dirs = vec![external.join("segments")];
    let error = run_all(reuse).unwrap_err();
    assert!(
        error.to_string().contains("rejected secondary content"),
        "{error}"
    );
}

/// A Stage 0 shard extracted under a scope cannot be sealed under another:
/// resuming it globally fails closed instead of certifying deleted traffic
/// as observed silence. The same scope resumes, and global shards narrow
/// to a scope.
#[test]
fn scoped_shards_refuse_a_foreign_reuse_scope() {
    use crate::cli_run_all::{run_all, RunAllRequest};
    use crate::FromStage;

    const FAR: &str = "27,-18.5,29.5,-13";

    fn request(
        primary: &Path,
        temp: &Path,
        work: &Path,
        scope: Option<&str>,
        from: FromStage,
        until: FromStage,
    ) -> RunAllRequest {
        RunAllRequest {
            primary_cache: primary.into(),
            secondary_cache: None,
            prepared_year_dir: temp.join("prepared-year"),
            prepared_dir: temp.join("prepared"),
            work_dir: work.into(),
            reused_segments_dirs: Vec::new(),
            days: vec![DAY.to_string()],
            increment_days: Vec::new(),
            scope_bbox: scope.map(str::to_string),
            from_stage: from,
            until_stage: until,
            cruise_phase: aircraft_extract::stage_2b::CruisePhase::All,
            cruise_spill_disk_budget_bytes: None,
        }
    }

    fn sealed_rows(work: &Path) -> usize {
        let mut rows = 0;
        let mut stack = vec![work.join("segments_by_square/z9")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "arrow") {
                    rows += aircraft_extract::arrow_io::read_segments(&path).unwrap().len();
                }
            }
        }
        rows
    }

    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    // One hour of primary track near Prague: 7 samples, 6 segments.
    let mut samples = Vec::new();
    for i in 0..7 {
        samples.push(format!(
            "[{},50.5,{:.6},4000,180,90,0,0]",
            i * 10,
            14.0 + f64::from(i) * 0.001
        ));
    }
    write_day_archive(
        &primary,
        DAY,
        &[(
            "4ca001".to_string(),
            format!(
                r#"{{"icao":"4ca001","t":"B738","timestamp":{MIDNIGHT},"trace":[{}]}}"#,
                samples.join(",")
            ),
        )],
    );
    let square = grid::square_of(50.5, 14.0);
    let prepared = temp.path().join("prepared");
    let dem_dir = prepared.join(grid::square_name(square));
    std::fs::create_dir_all(&dem_dir).unwrap();
    let dem_len = grid::raster::RasterWindow::for_square(square).cell_count() * 2;
    std::fs::write(dem_dir.join("dem.u16le"), vec![0u8; dem_len]).unwrap();
    std::fs::create_dir_all(temp.path().join("prepared-year")).unwrap();

    // A scoped extract drops the Prague track, then refuses a global seal.
    let scoped = temp.path().join("scoped");
    run_all(request(
        &primary,
        temp.path(),
        &scoped,
        Some(FAR),
        FromStage::Stage0,
        FromStage::Stage1,
    ))
    .unwrap();
    let day_shard = scoped.join("segments").join(format!("{DAY}.arrow"));
    assert!(aircraft_extract::arrow_io::read_segments(&day_shard).unwrap().is_empty());
    let error = run_all(request(
        &primary,
        temp.path(),
        &scoped,
        None,
        FromStage::Shuffle,
        FromStage::Shuffle,
    ))
    .unwrap_err();
    assert!(error.to_string().contains("extracted under scope"), "{error}");
    // The same scope resumes and seals the complete scoped sample: nothing.
    run_all(request(
        &primary,
        temp.path(),
        &scoped,
        Some(FAR),
        FromStage::Shuffle,
        FromStage::Shuffle,
    ))
    .unwrap();
    assert_eq!(sealed_rows(&scoped), 0);

    // A global extract seals globally, and narrows to a scope.
    let global = temp.path().join("global");
    run_all(request(
        &primary,
        temp.path(),
        &global,
        None,
        FromStage::Stage0,
        FromStage::Stage1,
    ))
    .unwrap();
    run_all(request(
        &primary,
        temp.path(),
        &global,
        None,
        FromStage::Shuffle,
        FromStage::Shuffle,
    ))
    .unwrap();
    assert_eq!(sealed_rows(&global), 6);
    let narrowed = temp.path().join("narrowed");
    run_all(request(
        &primary,
        temp.path(),
        &narrowed,
        None,
        FromStage::Stage0,
        FromStage::Stage1,
    ))
    .unwrap();
    run_all(request(
        &primary,
        temp.path(),
        &narrowed,
        Some(FAR),
        FromStage::Shuffle,
        FromStage::Shuffle,
    ))
    .unwrap();
    assert_eq!(sealed_rows(&narrowed), 0);
}
