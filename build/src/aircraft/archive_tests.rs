//! Archive cases: split parts, end markers, corrupt members, alternative exports, the catalog.

use super::*;
use crate::aircraft::catalog::primary_day_parts;
use crate::aircraft::readsb::tests::gz;
use std::path::Path;

pub fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("qm-aircraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// A TAR of named members.
pub fn tar_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap()
}

pub fn trace_json(address: &str, callsign: &str, latitudes: &[f64]) -> Vec<u8> {
    let rows: Vec<String> = latitudes
        .iter()
        .enumerate()
        .map(|(i, lat)| {
            format!(
                r#"[{},{lat},14.0,1000.0,100.0,90.0,0,0,{{"flight":"{callsign}"}}]"#,
                i * 10
            )
        })
        .collect();
    gz(&format!(
        r#"{{"icao":"{address}","t":"C172","timestamp":1711065600,"trace":[{}]}}"#,
        rows.join(",")
    ))
}

fn read(directory: &Path) -> Result<DayArchive, String> {
    read_archive(&directory_archive_parts(directory)?)
}

/// A broken archive fails the day; a corrupt member is recorded, recovered only when another
/// export holds its address.
#[test]
fn broken_archives_fail_and_corrupt_members_are_recorded() {
    let directory = scratch_directory("broken");
    assert!(read(&directory).is_err(), "no archive");
    std::fs::write(
        directory.join("a.tar"),
        tar_bytes(&[("traces/23/trace_full_abc123.json", b"corrupt gzip")]),
    )
    .unwrap();
    let day = read(&directory).unwrap();
    assert!(day.traces.is_empty());
    assert!(!day.corrupt_members[0].recovered);
    let intact = trace_json("abc123", "OK", &[50.0, 50.1]);
    std::fs::write(
        directory.join("b.tar"),
        tar_bytes(&[("trace_full_abc123.json.gz", &intact)]),
    )
    .unwrap();
    let day = read(&directory).unwrap();
    assert_eq!(day.traces.len(), 1);
    assert!(day.corrupt_members[0].recovered);
    std::fs::remove_file(directory.join("a.tar")).unwrap();
    let bytes = std::fs::read(directory.join("b.tar")).unwrap();
    std::fs::remove_file(directory.join("b.tar")).unwrap();
    let cut = bytes.len() / 2;
    std::fs::write(directory.join("c.tar.aa"), &bytes[..cut]).unwrap();
    std::fs::write(directory.join("c.tar.ac"), &bytes[cut..]).unwrap();
    assert!(read(&directory).is_err(), "missing middle part");
    std::fs::rename(directory.join("c.tar.ac"), directory.join("c.tar.ab")).unwrap();
    assert_eq!(read(&directory).unwrap().traces.len(), 1);
    std::fs::write(directory.join("c.tar.ab"), &bytes[cut..bytes.len() - 1]).unwrap();
    assert!(read(&directory).is_err(), "truncated stream");
    std::fs::write(directory.join("c.tar.ab"), &bytes[cut..]).unwrap();
    std::fs::write(directory.join("c.tar.partial"), b"x").unwrap();
    assert!(read(&directory).is_err(), "unfinished part");
}

/// Of alternative exports of one address the one with more sane points wins, whole; ties keep the
/// split export (read first); addresses without identity never collapse.
#[test]
fn alternative_exports_keep_one_whole_trace_per_address() {
    let directory = scratch_directory("alternatives");
    let whole = tar_bytes(&[
        (
            "trace_full_abc123.json",
            &trace_json("abc123", "SHORT", &[50.0, 91.0, 50.1]),
        ),
        (
            "trace_full_def456.json",
            &trace_json("def456", "TIE_WHOLE", &[51.0, 51.1]),
        ),
        (
            "trace_full_unknown.json",
            &trace_json("", "UNKNOWN_A", &[54.0, 54.1]),
        ),
    ]);
    let split = tar_bytes(&[
        (
            "trace_full_abc123.json",
            &trace_json("abc123", "LONG", &[50.2, 50.3, 50.4]),
        ),
        (
            "trace_full_def456.json",
            &trace_json("def456", "TIE_SPLIT", &[51.2, 51.3]),
        ),
        (
            "trace_full_unknown.json",
            &trace_json("", "UNKNOWN_B", &[55.0, 55.1]),
        ),
    ]);
    std::fs::write(directory.join("export.tar"), &whole).unwrap();
    std::fs::write(directory.join("export.tar.aa"), &split[..512]).unwrap();
    std::fs::write(directory.join("export.tar.ab"), &split[512..]).unwrap();
    let traces = read(&directory).unwrap().traces;
    let callsign = |address: &str| {
        traces
            .iter()
            .find(|t| t.address == address)
            .map(|t| t.callsigns[0].callsign.clone())
            .unwrap()
    };
    assert_eq!(callsign("abc123"), "LONG");
    assert_eq!(callsign("def456"), "TIE_SPLIT");
    assert_eq!(traces.iter().filter(|t| t.address.is_empty()).count(), 2);
    assert_eq!(traces.len(), 4);
}

fn catalog(root: &Path, assets: &[(&str, &str, u64, &str)], checks: &[(&str, &str)]) {
    let database = rusqlite::Connection::open(root.join("catalog.sqlite")).unwrap();
    database
        .execute_batch(
            "CREATE TABLE assets (day TEXT, name TEXT, url TEXT UNIQUE, size INTEGER, sha256 TEXT, tag TEXT, PRIMARY KEY(day,name));
             CREATE TABLE archive_checks (tag TEXT PRIMARY KEY, inputs TEXT NOT NULL, error TEXT NOT NULL);",
        )
        .unwrap();
    for (day, name, size, tag) in assets {
        database
            .execute(
                "INSERT INTO assets VALUES (?1, ?2, ?2, ?3, '', ?4)",
                rusqlite::params![day, name, size, tag],
            )
            .unwrap();
    }
    for (tag, error) in checks {
        database
            .execute(
                "INSERT INTO archive_checks VALUES (?1, '', ?2)",
                [tag, error],
            )
            .unwrap();
    }
}

/// The catalog picks the publisher's export of a day, never a neighbouring one in the same
/// directory; a failed preferred export yields to its one intact alternative; MLAT-only is missing.
#[test]
fn the_catalog_selects_one_export_per_day() {
    let root = scratch_directory("catalog");
    let day = root.join("2025").join("2025-10-14");
    std::fs::create_dir_all(&day).unwrap();
    let staging = tar_bytes(&[(
        "trace_full_abc123.json",
        &trace_json("abc123", "STAGING", &[50.0, 50.1]),
    )]);
    let prod = tar_bytes(&[(
        "trace_full_abc123.json",
        &trace_json("abc123", "PROD", &[50.0, 50.1]),
    )]);
    std::fs::write(
        day.join("v2025.10.14-planes-readsb-staging-0.tar.aa"),
        &staging[..1024],
    )
    .unwrap();
    std::fs::write(
        day.join("v2025.10.14-planes-readsb-staging-0.tar.ab"),
        &staging[1024..],
    )
    .unwrap();
    std::fs::write(day.join("v2025.10.14-planes-readsb-prod-0.tar"), &prod).unwrap();
    let staging_tag = "v2025.10.14-planes-readsb-staging-0";
    catalog(
        &root,
        &[
            (
                "2025-10-14",
                "v2025.10.14-planes-readsb-staging-0.tar.aa",
                1024,
                staging_tag,
            ),
            (
                "2025-10-14",
                "v2025.10.14-planes-readsb-staging-0.tar.ab",
                staging.len() as u64 - 1024,
                staging_tag,
            ),
            (
                "2026-05-06",
                "v2026.05.06-planes-readsb-mlatonly-0.tar",
                10,
                "v2026.05.06-planes-readsb-mlatonly-0",
            ),
        ],
        &[],
    );
    let callsign = |parts: Vec<PathBuf>| {
        read_archive(&parts).unwrap().traces[0].callsigns[0]
            .callsign
            .clone()
    };
    let parts = primary_day_parts(&root, "2025-10-14").unwrap().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(callsign(parts), "STAGING");
    assert_eq!(primary_day_parts(&root, "2026-05-06").unwrap(), None);
    assert!(
        primary_day_parts(&root, "2025-10-15").is_err(),
        "outside the window"
    );
    std::fs::remove_file(root.join("catalog.sqlite")).unwrap();
    catalog(
        &root,
        &[(
            "2025-10-14",
            "v2025.10.14-planes-readsb-staging-0.tar.aa",
            1024,
            staging_tag,
        )],
        &[
            (staging_tag, "offset 2000011264: invalid header"),
            ("v2025.10.14-planes-readsb-prod-0", ""),
        ],
    );
    assert_eq!(
        callsign(primary_day_parts(&root, "2025-10-14").unwrap().unwrap()),
        "PROD"
    );
    std::fs::remove_file(root.join("catalog.sqlite")).unwrap();
    catalog(
        &root,
        &[(
            "2025-10-14",
            "v2025.10.14-planes-readsb-staging-0.tar.aa",
            999,
            staging_tag,
        )],
        &[],
    );
    assert!(
        primary_day_parts(&root, "2025-10-14").is_err(),
        "size differs from the catalog"
    );
}
