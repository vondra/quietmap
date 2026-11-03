//! Synthetic admission failures and the actual native weather writer, including its grid sampling.
use super::*;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};

fn archive(path: &Path, nodes: usize, receipt: serde_json::Value, counts: [u32; PERIODS]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (member, dtype, shape, width) in [
        ("c.npy", "<u4", vec![nodes, PERIODS], 4),
        ("f.npy", "<u4", vec![nodes, PERIODS, SECTORS], 4),
        ("a.npy", "<f8", vec![nodes, PERIODS, BANDS], 8),
    ] {
        zip.start_file(member, options).unwrap();
        let dimensions = shape
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let header =
            format!("{{'descr': '{dtype}', 'fortran_order': False, 'shape': ({dimensions}), }}\n");
        zip.write_all(b"\x93NUMPY\x01\x00").unwrap();
        zip.write_all(&(header.len() as u16).to_le_bytes()).unwrap();
        zip.write_all(header.as_bytes()).unwrap();
        let size: usize = shape.iter().product();
        for start in (0..size).step_by(65536) {
            let mut bytes = Vec::with_capacity(65536 * width);
            for index in start..size.min(start + 65536) {
                if member == "c.npy" {
                    bytes.extend(counts[index % PERIODS].to_le_bytes());
                } else if member == "f.npy" {
                    bytes.extend(1u32.to_le_bytes());
                } else {
                    let node = index / (PERIODS * BANDS);
                    let row = node / COLUMNS;
                    let column = node % COLUMNS;
                    let value = row as f64 / 10.0
                        + column as f64 / 100.0
                        + (index % (PERIODS * BANDS)) as f64;
                    bytes.extend(value.to_le_bytes());
                }
            }
            zip.write_all(&bytes).unwrap();
        }
    }
    zip.start_file("done.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&receipt).unwrap())
        .unwrap();
    zip.finish().unwrap();
}

fn fixture(label: &str, nodes: usize) -> (PathBuf, serde_json::Value) {
    let root = std::env::temp_dir().join(format!("qm-climate-{label}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("2000")).unwrap();
    archive(&root.join("2000/statistics"), nodes, receipt(), [4, 1, 3]);
    let manifest = json!({"year":"2000","as_of":"2001-01-01","built":false,
        "flows":{"start":"2000-01-01","end":"2001-01-01"},"unavailable":[],"inputs":[{
        "id":"statistics","kind":"prepare-weather-statistics","region":"world","publisher":"Test",
        "url":"https://example.org/climate","observed":{"start":"2000-01-01","end":"2000-01-01"},
        "published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"Measured states.",
        "conversion":{"sha256":"","sampling_hours":3,"counts":"c.npy","favourable":"f.npy","means":"a.npy","receipt":"done.json"}}]});
    (root, manifest)
}
fn receipt() -> serde_json::Value {
    json!({"next_step":8,"identity":{"contract":{"climatology_years":"2000-2000","sampling_hours":"3"}}})
}
fn inputs(root: &Path, manifest: &mut serde_json::Value) -> YearInputs {
    manifest["inputs"][0]["conversion"]["sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(std::fs::read(root.join("2000/statistics")).unwrap())
    ));
    let path = root.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(manifest).unwrap()).unwrap();
    YearInputs::read(&path, root).unwrap()
}
#[test]
fn statistics_admission_checks_dates_completion_identity_digest_and_node_counts() {
    let (root, mut manifest) = fixture("admission", 1);
    Statistics::read(&inputs(&root, &mut manifest)).unwrap();
    let mut changed = manifest.clone();
    changed["inputs"][0]["observed"]["end"] = json!("2001-01-01");
    assert!(Statistics::read(&inputs(&root, &mut changed)).is_err());
    let mut admitted = inputs(&root, &mut manifest);
    admitted.manifest.inputs[0].conversion.as_mut().unwrap()["sha256"] = json!("bad");
    assert!(
        Statistics::read(&admitted)
            .err()
            .unwrap()
            .contains("checksum")
    );
    for (field, value, message) in [
        ("next_step", json!(7), "incomplete"),
        (
            "identity",
            json!({"contract":{"climatology_years":"1999-1999","sampling_hours":"3"}}),
            "identify",
        ),
    ] {
        let mut modified = receipt();
        modified[field] = value;
        archive(&root.join("2000/statistics"), 1, modified, [4, 1, 3]);
        assert!(
            Statistics::read(&inputs(&root, &mut manifest))
                .err()
                .unwrap()
                .contains(message)
        );
    }
    archive(&root.join("2000/statistics"), 1, receipt(), [4, 1, 2]);
    assert!(
        Statistics::read(&inputs(&root, &mut manifest))
            .err()
            .unwrap()
            .contains("node 0")
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_weather_build_selects_every_second_source_row_and_column() {
    let (root, mut manifest) = fixture("writer", ROWS * COLUMNS);
    let out = root.join("weather");
    crate::weather::build(&inputs(&root, &mut manifest), &out).unwrap();
    let table = physics::weather::WeatherTable::read(&out).unwrap();
    for (row, column) in [(0, 0), (123, 456), (360, 719)] {
        let place = table.place(90.0 - row as f64 / 2.0, column as f64 / 2.0);
        assert_eq!(place.favourable.by_sector[0], [0.25; SECTORS]);
        for band in 0..BANDS {
            let value = row as f64 * 2.0 / 10.0 + column as f64 * 2.0 / 100.0 + band as f64;
            let expected = ((12.0 * f64::from(value as f32)
                + 4.0 * f64::from((value + 8.0) as f32)
                + 8.0 * f64::from((value + 16.0) as f32))
                / 24.0
                * 100.0)
                .round()
                / 100.0;
            assert_eq!(place.alpha_db_per_km[band], expected);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
