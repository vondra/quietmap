//! Read publisher CSV and GeoJSON records without copying the decoded JSON array.

use anyhow::{Result, ensure};
use serde_json::Value;
use std::path::Path;

pub fn features(path: &Path, format: &str) -> Result<Vec<Value>> {
    if format == "gppd-csv" {
        let mut reader = csv::ReaderBuilder::new().from_path(path)?;
        let headers = reader.headers()?.clone();
        ensure!(
            ["latitude", "longitude", "primary_fuel"]
                .iter()
                .all(|key| headers.iter().any(|h| h == *key)),
            "missing GPPD columns"
        );
        return reader
            .records()
            .map(|row| {
                let row = row?;
                Ok(Value::Object(
                    headers
                        .iter()
                        .zip(row.iter())
                        .map(|(k, v)| (k.into(), Value::String(v.into())))
                        .collect(),
                ))
            })
            .collect();
    }
    let mut value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let key = if format == "eprtr-json" {
        "results"
    } else {
        ensure!(
            value["type"] == "FeatureCollection",
            "expected facility FeatureCollection"
        );
        "features"
    };
    match value[key].take() {
        Value::Array(rows) => Ok(rows),
        _ => anyhow::bail!("missing facility records"),
    }
}
