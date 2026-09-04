//! Parser, prefix probe, and complete TAR regression fixtures.

use super::archive::ConcatReader;
use super::typecode_probe::*;
use super::*;
use flate2::{write::GzEncoder, Compression};
use std::io::BufReader;
use std::io::Write;
use std::path::Path;

fn gz(json: &str) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(json.as_bytes()).unwrap();
    e.finish().unwrap()
}

#[test]
fn parses_ground_altitude_marker() {
    let raw = gz(r#"{"icao":"49d261","t":"PC12","timestamp":1000,"trace":[
            [10,50.0,14.0,"ground",12.0,90.0,0,0],
            [20,50.001,14.001,600.0,80.0,120.0,0,0]
        ]}"#);
    let t = parse_trace(raw.as_slice()).unwrap().unwrap();
    assert_eq!(t.points.len(), 2);
    assert!(t.points[0].alt_is_ground());
    assert!(t.points[0].on_ground_raw()); // implied by alt_is_ground
    assert!(t.points[0].alt_ft.is_nan());
    assert!(t.points[0].airborne_alt_ft().is_none());
    assert!(!t.points[1].alt_is_ground());
    assert_eq!(t.points[1].alt_ft, 600.0);
}

#[test]
fn parses_bitfield_on_ground() {
    let raw = gz(r#"{"icao":"49c083","t":"WT9","timestamp":2000,"trace":[
            [10,50.0,14.0,300.0,40.0,90.0,1,0],
            [20,50.001,14.001,400.0,60.0,120.0,0,0]
        ]}"#);
    let t = parse_trace(raw.as_slice()).unwrap().unwrap();
    assert!(t.points[0].on_ground_raw());
    assert!(!t.points[0].alt_is_ground());
    assert!(!t.points[1].on_ground_raw());
}

#[test]
fn drops_traces_with_fewer_than_two_points() {
    let raw = gz(
        r#"{"icao":"abc123","t":"B738","timestamp":1,"trace":[[1,50.0,14.0,1000.0,250.0,90.0,0,0]]}"#,
    );
    assert!(parse_trace(raw.as_slice()).unwrap().is_none());
}

#[test]
fn extracts_callsign_from_point_metadata() {
    // adsb.lol col 8 is sometimes a meta object carrying `flight`,
    // sometimes null. The parser records each value transition;
    // duplicates from re-emitted meta blocks must coalesce.
    let raw = gz(r#"{"icao":"49d328","t":"A320","timestamp":1000,"trace":[
            [10,50.0,14.0,1000.0,250.0,90.0,0,0,null],
            [20,50.001,14.001,1100.0,250.0,90.0,0,0,{"flight":"TVS100P  "}],
            [30,50.002,14.002,1200.0,250.0,90.0,0,0,{"flight":"TVS100P  "}],
            [40,50.003,14.003,1300.0,250.0,90.0,0,0,{"flight":"TVS200X  "}]
        ]}"#);
    let t = parse_trace(raw.as_slice()).unwrap().unwrap();
    assert_eq!(t.callsigns.len(), 2);
    assert_eq!(t.callsigns[0].point_idx, 1);
    assert_eq!(t.callsigns[0].value, "TVS100P");
    assert_eq!(t.callsigns[1].point_idx, 3);
    assert_eq!(t.callsigns[1].value, "TVS200X");
}

#[test]
fn callsign_metadata_optional() {
    let raw = gz(r#"{"icao":"49d262","t":"PC12","timestamp":1000,"trace":[
            [10,50.0,14.0,1000.0,250.0,90.0,0,0],
            [20,50.001,14.001,1100.0,250.0,90.0,0,0]
        ]}"#);
    let t = parse_trace(raw.as_slice()).unwrap().unwrap();
    assert!(t.callsigns.is_empty());
}

#[test]
fn missing_baro_rate_defaults_to_zero() {
    // 7-element row (no baro_rate column) should still parse.
    let raw = gz(r#"{"icao":"49d262","t":"PC12","timestamp":1000,"trace":[
            [10,50.0,14.0,500.0,80.0,90.0,0],
            [20,50.001,14.001,600.0,80.0,90.0,0]
        ]}"#);
    let t = parse_trace(raw.as_slice()).unwrap().unwrap();
    assert_eq!(t.points[0].baro_rate_fpm, 0.0);
}

#[test]
fn concat_reader_glues_two_streams() {
    let a = b"first half".to_vec();
    let b = b" second half".to_vec();
    let concat = ConcatReader::new(vec![a.as_slice(), b.as_slice()]);
    let mut out = Vec::new();
    BufReader::new(concat).read_to_end(&mut out).unwrap();
    assert_eq!(&out, b"first half second half");
}

/// Two-point trace JSON with the `"t"` field in readsb's normal
/// early-header position.
fn trace_json(icao: &str, typecode_field: &str) -> String {
    format!(
        r#"{{"icao":"{icao}",{typecode_field}"timestamp":1000,"trace":[
            [10,50.0,14.0,1000.0,250.0,90.0,0,0],
            [20,50.001,14.001,1100.0,250.0,90.0,0,0]
        ]}}"#
    )
}

#[test]
fn scan_json_typecode_fixtures() {
    assert_eq!(
        scan_json_typecode(br#"{"metadata":{"t":"B738"},"t":"C172"}"#),
        Some("C172".into())
    );
    assert_eq!(scan_json_typecode(br#"{"metadata":{"t":"B738"}}"#), None);
    // Normal compact readsb form.
    assert_eq!(
        scan_json_typecode(br#"{"icao":"a","r":"OK-ABC","t":"B738","trace":[]}"#),
        Some("B738".to_string())
    );
    // Pretty-printed: whitespace around the colon + newlines.
    assert_eq!(
        scan_json_typecode(b"{\n  \"icao\": \"b\",\n  \"t\" : \"C172\",\n}"),
        Some("C172".to_string())
    );
    // Empty string value is a valid HIT (blank typecode = FALLBACK).
    assert_eq!(scan_json_typecode(br#"{"t":""}"#), Some(String::new()));
    // No "t" key at all (noRegData TIS-B shape) → miss.
    assert_eq!(
        scan_json_typecode(br#"{"icao":"c","noRegData":true}"#),
        None
    );
    // Non-string value → miss (full parse decides).
    assert_eq!(scan_json_typecode(br#"{"t":null}"#), None);
    // Value cut off by the probe window → miss, NOT a partial hit.
    assert_eq!(scan_json_typecode(br#"{"icao":"d","t":"C17"#), None);
    // `"t"` as a string VALUE (not a key) must not match; the real
    // key later in the buffer still hits.
    assert_eq!(
        scan_json_typecode(br#"{"r":"t","t":"R44"}"#),
        Some("R44".to_string())
    );
    // Escaped quotes inside an earlier value can't false-match.
    assert_eq!(
        scan_json_typecode(br#"{"desc":"say \"t\": hi","t":"EC35"}"#),
        Some("EC35".to_string())
    );
}

#[test]
fn probe_typecode_prefix_inflates_only_the_window() {
    // "t" within the first 512 decompressed bytes → hit.
    let early = gz(&trace_json("aaa111", r#""t":"B738","#));
    assert_eq!(probe_typecode_prefix(&early), Some("B738".to_string()));
    // "t" pushed past the probe window by a long desc → miss.
    let pad = "x".repeat(TYPECODE_PROBE_DECOMPRESSED_BYTES + 64);
    let late = gz(&trace_json(
        "bbb222",
        &format!(r#""desc":"{pad}","t":"C172","#),
    ));
    assert_eq!(probe_typecode_prefix(&late), None);
    // Not gzip at all → miss (full parse path decides).
    assert_eq!(probe_typecode_prefix(b"plain bytes, not gzip"), None);
}

fn day_dir_with_tar(entries: &[(&str, &[u8])]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(tmp.path().join("subset.tar")).unwrap();
    let mut builder = tar::Builder::new(file);
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.finish().unwrap();
    tmp
}

/// End-to-end probe semantics:
/// probe hits skip rejected traces pre-parse; probe misses (late
/// `"t"`, absent `"t"`) ALWAYS full-parse and are filtered on the
/// parsed typecode — a trace is never classified by absence.
#[test]
fn prefilter_skips_on_hit_and_full_parses_on_miss() {
    let b738 = gz(&trace_json("aaa111", r#""t":"B738","#));
    let c172_pretty = gz(
        "{\n  \"icao\": \"bbb222\",\n  \"t\": \"C172\",\n  \"timestamp\": 1000,\n  \"trace\": [\n    [10,50.0,14.0,1000.0,250.0,90.0,0,0],\n    [20,50.001,14.001,1100.0,250.0,90.0,0,0]\n  ]\n}",
    );
    let pad = "x".repeat(TYPECODE_PROBE_DECOMPRESSED_BYTES + 64);
    let c172_late = gz(&trace_json(
        "ccc333",
        &format!(r#""desc":"{pad}","t":"C172","#),
    ));
    let no_typecode = gz(&trace_json("ddd444", ""));
    let dir = day_dir_with_tar(&[
        ("traces/11/trace_full_aaa111.json", b738.as_slice()),
        ("traces/22/trace_full_bbb222.json", c172_pretty.as_slice()),
        ("traces/33/trace_full_ccc333.json", c172_late.as_slice()),
        ("traces/44/trace_full_ddd444.json", no_typecode.as_slice()),
    ]);
    let keep_ga = |tc: &str| tc == "C172";
    let (traces, stats) = read_day_traces_filtered(dir.path(), Some(&keep_ga)).unwrap();
    let mut kept: Vec<&str> = traces.iter().map(|t| t.icao24.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        ["bbb222", "ccc333"],
        "early + late C172 kept; B738 skipped; blank filtered post-parse"
    );
    assert!(traces.iter().all(|t| t.aircraft_type == "C172"));
    assert_eq!(stats.probe_hits, 2, "B738 + pretty C172");
    assert_eq!(
        stats.skipped_pre_parse, 1,
        "B738 dropped without full parse"
    );
    assert_eq!(
        stats.probe_misses, 2,
        "late-t + no-t fell back to full parse"
    );

    // Without a prefilter the same tar yields every trace and zero
    // probe activity — the default path is untouched.
    let (all, no_stats) = read_day_traces_filtered(dir.path(), None).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(no_stats, TypecodeProbeStats::default());
}

/// Smoke test against real cached data when available — proves the
/// parser handles the actual adsb.lol layout, not just synthetic
/// fixtures. Skips unless QM_FLIGHTS_CACHE points at a radius cache root
/// with the year-nested 2025/2025-01-21 day dir.
#[test]
fn smoke_real_praha_cache() {
    let Ok(root) = std::env::var("QM_FLIGHTS_CACHE") else {
        return;
    };
    let day = Path::new(&root).join("2025/2025-01-21");
    if !day.exists() {
        return;
    }
    let traces = read_day_traces(&day).unwrap();
    assert!(traces.len() > 100, "got only {} traces", traces.len());
    let total_pts: usize = traces.iter().map(|t| t.points.len()).sum();
    assert!(total_pts > 50_000, "got only {total_pts} pts");
    // At least some have a 4-character ICAO typecode (most common case).
    let typed = traces.iter().filter(|t| t.aircraft_type.len() >= 3).count();
    assert!(typed > traces.len() / 2);
    // ≥30 % of traces must carry a callsign. Robust to GA-heavy
    // days where many Mode-S aircraft never broadcast `flight`.
    let with_callsign = traces.iter().filter(|t| !t.callsigns.is_empty()).count();
    assert!(
        with_callsign * 10 > traces.len() * 3,
        "expected ≥30% traces to carry a callsign, got {with_callsign}/{}",
        traces.len()
    );
}

#[test]
fn incomplete_archives_and_corrupt_traces_fail_loudly() {
    let empty = tempfile::tempdir().unwrap();
    assert!(read_day_traces(empty.path()).is_err());
    let bad = day_dir_with_tar(&[("trace_full_bad.json", b"corrupt gzip")]);
    assert!(read_day_traces(bad.path()).is_err());
    let json = gz(&trace_json("abc123", ""));
    let valid = day_dir_with_tar(&[("trace_full_abc123.json.gz", &json)]);
    assert_eq!(read_day_traces(valid.path()).unwrap().len(), 1);
    let path = valid.path().join("subset.tar");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let cut = bytes.len() / 2;
    std::fs::write(valid.path().join("subset.tar.aa"), &bytes[..cut]).unwrap();
    std::fs::write(valid.path().join("subset.tar.ac"), &bytes[cut..]).unwrap();
    assert!(
        read_day_traces(valid.path()).is_err(),
        "missing middle split part"
    );
    std::fs::rename(
        valid.path().join("subset.tar.ac"),
        valid.path().join("subset.tar.ab"),
    )
    .unwrap();
    assert_eq!(read_day_traces(valid.path()).unwrap().len(), 1);
    std::fs::write(
        valid.path().join("subset.tar.ab"),
        &bytes[cut..bytes.len() - 1],
    )
    .unwrap();
    assert!(read_day_traces(valid.path()).is_err(), "truncated stream");
}

#[test]
fn multiple_complete_tar_files_are_all_read() {
    let json = gz(&trace_json("abc123", ""));
    let dir = day_dir_with_tar(&[("trace_full_abc123.json", &json)]);
    std::fs::copy(dir.path().join("subset.tar"), dir.path().join("second.tar")).unwrap();
    assert_eq!(read_day_traces(dir.path()).unwrap().len(), 2);
}
