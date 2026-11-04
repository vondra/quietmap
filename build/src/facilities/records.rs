//! One Arrow facility schema for mapped sites, leisure and registry observations, in WGS84 WKB.

use anyhow::{Context, Result, ensure};
use arrow_array::{
    Array, ArrayRef, BinaryArray, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt8Array, UInt16Array,
};
use arrow_ipc::{reader::FileReader, writer::FileWriter};
use arrow_schema::{DataType, Field, Schema};
use std::{collections::HashMap, fs::File, io::BufReader, path::Path, sync::Arc};

pub const INDUSTRY: u8 = 0;
pub const LEISURE: u8 = 1;
pub const REGISTRY_POINT: u8 = 2;
pub const REGISTRY_AREA: u8 = 3;

#[derive(Clone, Debug, Default)]
pub struct Facility {
    pub identity: String,
    pub evidence_identity: String,
    pub operation_identity: String,
    pub role: u8,
    pub longitude: f64,
    pub latitude: f64,
    pub geometry: Option<Vec<u8>>,
    pub geometry_kind: u8,
    pub area_m2: Option<f32>,
    pub osm_id: i64,
    pub osm_kind: String,
    pub name: Option<String>,
    pub osm_tags: String,
    pub source_type: u8,
    pub site_subtype: u8,
    pub activity: u16,
    pub capacity: Option<f64>,
    pub capacity_unit: String,
    pub operating_from: Option<String>,
    pub operating_until: Option<String>,
    pub observed_on: String,
    pub operating_state: u8,
    pub classification: u8,
    pub classification_admitted: u8,
    pub source: String,
    pub source_id: u16,
    pub authority_rank: u8,
    pub authority_edition: u16,
    pub country: u16,
    pub suppressed: u8,
}

macro_rules! facility_fields {
    ($required:ident, $optional:ident) => {
        $required!(identity, StringArray, Utf8);
        $required!(evidence_identity, StringArray, Utf8);
        $required!(operation_identity, StringArray, Utf8);
        $required!(role, UInt8Array, UInt8);
        $required!(longitude, Float64Array, Float64);
        $required!(latitude, Float64Array, Float64);
        $optional!(geometry, BinaryArray, Binary);
        $required!(geometry_kind, UInt8Array, UInt8);
        $optional!(area_m2, Float32Array, Float32);
        $required!(osm_id, Int64Array, Int64);
        $required!(osm_kind, StringArray, Utf8);
        $optional!(name, StringArray, Utf8);
        $required!(osm_tags, StringArray, Utf8);
        $required!(source_type, UInt8Array, UInt8);
        $required!(site_subtype, UInt8Array, UInt8);
        $required!(activity, UInt16Array, UInt16);
        $optional!(capacity, Float64Array, Float64);
        $required!(capacity_unit, StringArray, Utf8);
        $optional!(operating_from, StringArray, Utf8);
        $optional!(operating_until, StringArray, Utf8);
        $required!(observed_on, StringArray, Utf8);
        $required!(operating_state, UInt8Array, UInt8);
        $required!(classification, UInt8Array, UInt8);
        $required!(classification_admitted, UInt8Array, UInt8);
        $required!(source, StringArray, Utf8);
        $required!(source_id, UInt16Array, UInt16);
        $required!(authority_rank, UInt8Array, UInt8);
        $required!(authority_edition, UInt16Array, UInt16);
        $required!(country, UInt16Array, UInt16);
        $required!(suppressed, UInt8Array, UInt8);
    };
}

pub fn write(path: &Path, rows: &[Facility], year: &str, input_hash: &str) -> Result<()> {
    for row in rows {
        validate(row)?;
    }
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    macro_rules! required {
        ($field:ident, $array:ident, $kind:ident) => {
            fields.push(Field::new(stringify!($field), DataType::$kind, false));
            columns.push(Arc::new(
                rows.iter()
                    .map(|r| Some(r.$field.clone()))
                    .collect::<$array>(),
            ));
        };
    }
    macro_rules! optional {
        ($field:ident, $array:ident, $kind:ident) => {
            fields.push(Field::new(stringify!($field), DataType::$kind, true));
            columns.push(Arc::new(
                rows.iter().map(|r| r.$field.clone()).collect::<$array>(),
            ));
        };
    }
    facility_fields!(required, optional);
    let schema = Schema::new_with_metadata(
        fields,
        HashMap::from([
            ("facility_contract".into(), "1".into()),
            ("crs".into(), "EPSG:4326".into()),
            ("geometry_encoding".into(), "WKB".into()),
            ("year".into(), year.into()),
            ("input_sha256".into(), input_hash.into()),
        ]),
    );
    let batch = RecordBatch::try_new(Arc::new(schema), columns)?;
    std::fs::create_dir_all(path.parent().context("facility output has no parent")?)?;
    let mut writer = FileWriter::try_new(File::create_new(path)?, &batch.schema())?;
    writer.write(&batch)?;
    writer.finish()?;
    Ok(())
}

pub fn read(path: &Path, year: &str, input_hash: &str, role: Option<u8>) -> Result<Vec<Facility>> {
    let reader = FileReader::try_new(
        BufReader::new(File::open(path).with_context(|| path.display().to_string())?),
        None,
    )?;
    for (key, value) in [
        ("facility_contract", "1"),
        ("crs", "EPSG:4326"),
        ("geometry_encoding", "WKB"),
        ("year", year),
        ("input_sha256", input_hash),
    ] {
        ensure!(
            reader.schema().metadata().get(key).map(String::as_str) == Some(value),
            "{}: invalid {key}",
            path.display()
        );
    }
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let roles =
            crate::dev4::column::<UInt8Array>(&batch, "role").map_err(anyhow::Error::msg)?;
        let indices: Vec<_> = (0..batch.num_rows())
            .filter(|&i| role.is_none_or(|role| roles.value(i) == role))
            .collect();
        let mut part = vec![Facility::default(); indices.len()];
        macro_rules! required {
            ($field:ident, $array:ident, $kind:ident) => {
                let values = crate::dev4::column::<$array>(&batch, stringify!($field))
                    .map_err(anyhow::Error::msg)?;
                ensure!(
                    values.null_count() == 0,
                    "null required facility column {}",
                    stringify!($field)
                );
                for (&i, row) in indices.iter().zip(&mut part) {
                    row.$field = values.value(i).to_owned();
                }
            };
        }
        macro_rules! optional {
            ($field:ident, $array:ident, $kind:ident) => {
                let values = crate::dev4::column::<$array>(&batch, stringify!($field))
                    .map_err(anyhow::Error::msg)?;
                for (&i, row) in indices.iter().zip(&mut part) {
                    row.$field = values.is_valid(i).then(|| values.value(i).to_owned());
                }
            };
        }
        facility_fields!(required, optional);
        for row in &part {
            validate(row)?;
        }
        rows.extend(part);
    }
    Ok(rows)
}

fn validate(row: &Facility) -> Result<()> {
    ensure!(
        !row.identity.is_empty() && !row.source.is_empty(),
        "missing facility identity/source"
    );
    ensure!(
        row.latitude.is_finite()
            && row.longitude.is_finite()
            && row.latitude.abs() <= 90.0
            && row.longitude.abs() <= 180.0,
        "invalid facility location"
    );
    ensure!(
        row.role <= REGISTRY_AREA
            && row.operating_state <= 2
            && row.classification <= 2
            && row.classification_admitted <= 1
            && row.suppressed <= 1,
        "invalid facility classification"
    );
    ensure!(
        row.area_m2.is_none_or(|v| v.is_finite() && v >= 0.0)
            && row.capacity.is_none_or(|v| v.is_finite() && v >= 0.0),
        "invalid facility capacity or area"
    );
    Ok(())
}
