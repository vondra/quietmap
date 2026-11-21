//! Unit tests for FRA crossing verification and parser defect guards.
use super::*;
use crate::year::Input;
use serde_json::json;
use std::io::Write;

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
