//! Aircraft Arrow schemas: scratch inputs, popup payloads, metadata and contract gates.

use crate::SCHEMA_VERSION;
use arrow::datatypes::{DataType, Field, Fields, Schema};
pub use square_store::aircraft_contract::{
    AIRBORNE_CONTRACT, AIRPORT_SUMMARY_CONTRACT, AIRPORT_TRAFFIC_CONTRACT, CRUISE_CONTRACT,
};
use std::{collections::HashMap, sync::Arc};
mod airport;
mod popup;
mod scratch;
pub use airport::*;
pub use popup::*;
pub use scratch::*;
#[cfg(test)]
mod tests;

pub const CRUISE_TOP_K: usize = 50;
pub const GEOMETRY_KIND_LINE: u8 = 0;
pub const GEOMETRY_KIND_AREA_GRID_POINT: u8 = 1;
pub const GEOMETRY_KIND_SYNTHETIC: u8 = 2;
pub const NUM_GSE_CLASSES: i32 = noise_compute::emission::gse::NUM_GSE_CLASSES as i32;
pub const NUM_OPS_KINDS: i32 = 3;

fn base_metadata(extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut md = HashMap::new();
    md.insert("schema_version".to_string(), SCHEMA_VERSION.to_string());
    for (k, v) in extra {
        md.insert(k.to_string(), v.to_string());
    }
    md
}

pub fn with_n_days(schema: Arc<Schema>, n_days: u16) -> Arc<Schema> {
    let mut md = schema.metadata().clone();
    md.insert("n_days".to_string(), n_days.to_string());
    Arc::new((*schema).clone().with_metadata(md))
}

pub fn sample_days_by_class_vector(n_days: u16, ga_n_days: u16) -> String {
    use noise_compute::emission::aircraft::{is_ga_sampled_class, NUM_CLASSES};
    let ga = if ga_n_days == 0 { n_days } else { ga_n_days };
    (0..NUM_CLASSES)
        .map(|c| {
            if is_ga_sampled_class(c as u8) {
                ga
            } else {
                n_days
            }
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn with_n_days_and_windows(schema: Arc<Schema>, n_days: u16, ga_n_days: u16) -> Arc<Schema> {
    use noise_compute::emission::aircraft::SAMPLE_DAYS_BY_CLASS_KEY;
    let mut md = schema.metadata().clone();
    md.insert("n_days".to_string(), n_days.to_string());
    if ga_n_days > 0 {
        md.insert("ga_n_days".to_string(), ga_n_days.to_string());
    }
    md.insert(
        SAMPLE_DAYS_BY_CLASS_KEY.to_string(),
        sample_days_by_class_vector(n_days, ga_n_days),
    );
    Arc::new((*schema).clone().with_metadata(md))
}

fn assert_stamp(
    metadata: &HashMap<String, String>,
    key: &str,
    expected: &str,
) -> anyhow::Result<()> {
    let actual = metadata.get(key).map(String::as_str);
    anyhow::ensure!(
        actual == Some(expected),
        "{key} mismatch: expected {expected}, got {actual:?}"
    );
    Ok(())
}

pub fn assert_schema_version(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
    assert_stamp(metadata, "schema_version", SCHEMA_VERSION)
}

pub fn assert_airborne_contract(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
    assert_stamp(metadata, "airborne_contract", AIRBORNE_CONTRACT)
}

pub fn assert_cruise_contract(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
    assert_stamp(metadata, "cruise_contract", CRUISE_CONTRACT)
}

pub fn assert_airport_traffic_contract(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
    assert_stamp(
        metadata,
        "airport_traffic_contract",
        AIRPORT_TRAFFIC_CONTRACT,
    )
}

pub fn assert_airport_summary_contract(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
    assert_stamp(
        metadata,
        "airport_summary_contract",
        AIRPORT_SUMMARY_CONTRACT,
    )
}
