//! Parser and complete TAR regression fixtures.

use super::archive::ConcatReader;
use super::*;
use flate2::{write::GzEncoder, Compression};
use std::io::BufReader;
use std::io::Write;
use std::path::Path;

pub(super) fn gz(json: &str) -> Vec<u8> {
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
    assert!(t.points[0].alt_ft.is_nan());
    assert!(t.points[0].airborne_alt_ft().is_none());
    assert!(!t.points[1].alt_is_ground());
    assert_eq!(t.points[1].alt_ft, 600.0);
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

pub(super) fn day_dir_with_tar(entries: &[(&str, &[u8])]) -> tempfile::TempDir {
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
    let traces = read_day_archive(&day).unwrap().traces;
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

/// A broken archive fails the day; a corrupt trace member is recorded, and
/// it counts as recovered only when another export holds that address.
#[test]
fn incomplete_archives_fail_loudly_and_corrupt_members_are_receipted() {
    let empty = tempfile::tempdir().unwrap();
    assert!(read_day_archive(empty.path()).is_err());
    let bad = day_dir_with_tar(&[("traces/bc/trace_full_abc123.json", b"corrupt gzip")]);
    let read = read_day_archive(bad.path()).unwrap();
    assert!(read.traces.is_empty());
    assert_eq!(read.corrupt_members.len(), 1);
    assert!(!read.corrupt_members[0].recovered);
    let json = gz(&trace_json("abc123", ""));
    std::fs::copy(
        day_dir_with_tar(&[("traces/bc/trace_full_abc123.json", &json)])
            .path()
            .join("subset.tar"),
        bad.path().join("intact.tar"),
    )
    .unwrap();
    let read = read_day_archive(bad.path()).unwrap();
    assert_eq!(read.traces.len(), 1);
    assert!(read.corrupt_members[0].recovered);
    let valid = day_dir_with_tar(&[("trace_full_abc123.json.gz", &json)]);
    assert_eq!(read_day_archive(valid.path()).unwrap().traces.len(), 1);
    let path = valid.path().join("subset.tar");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let cut = bytes.len() / 2;
    std::fs::write(valid.path().join("subset.tar.aa"), &bytes[..cut]).unwrap();
    std::fs::write(valid.path().join("subset.tar.ac"), &bytes[cut..]).unwrap();
    assert!(
        read_day_archive(valid.path()).is_err(),
        "missing middle split part"
    );
    std::fs::rename(
        valid.path().join("subset.tar.ac"),
        valid.path().join("subset.tar.ab"),
    )
    .unwrap();
    assert_eq!(read_day_archive(valid.path()).unwrap().traces.len(), 1);
    std::fs::write(
        valid.path().join("subset.tar.ab"),
        &bytes[cut..bytes.len() - 1],
    )
    .unwrap();
    assert!(read_day_archive(valid.path()).is_err(), "truncated stream");
}

#[test]
fn identical_trace_exports_are_selected_once() {
    let json = gz(&trace_json("abc123", ""));
    let dir = day_dir_with_tar(&[("trace_full_abc123.json", &json)]);
    std::fs::copy(dir.path().join("subset.tar"), dir.path().join("second.tar")).unwrap();
    assert_eq!(read_day_archive(dir.path()).unwrap().traces.len(), 1);
}
