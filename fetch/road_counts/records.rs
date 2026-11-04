//! Shared publisher record primitives preserve nulls, declared object paths and survey intervals.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
};
use crate::{roads::observation::Observation, year::YearInputs};
use anyhow::{Result, anyhow, ensure};
use chrono::NaiveDate;
use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Coverage {
    pub input_id: String,
    pub property: String,
    pub name: String,
}

pub(super) fn coverage(
    inputs: &YearInputs,
    id: &str,
    property: &str,
    name: &str,
) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
    let input = inputs
        .manifest
        .inputs
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| anyhow!("missing municipal boundary input"))?;
    ensure!(
        matches!(
            input.kind,
            crate::year::Kind::PrepareBoundaries | crate::year::Kind::PrepareRegions
        ),
        "invalid municipal boundary kind"
    );
    let raw: Value = serde_json::from_slice(&std::fs::read(
        inputs.by_id(id).map_err(anyhow::Error::msg)?,
    )?)?;
    let matched = raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("municipal boundary needs features"))?
        .iter()
        .filter(|f| f["properties"][property].as_str() == Some(name))
        .collect::<Vec<_>>();
    ensure!(
        matched.len() == 1,
        "municipal boundary identity must select one feature"
    );
    let shape = &matched[0]["geometry"];
    match shape["type"].as_str() {
        Some("Polygon") => Ok(vec![serde_json::from_value(shape["coordinates"].clone())?]),
        Some("MultiPolygon") => Ok(serde_json::from_value(shape["coordinates"].clone())?),
        _ => Err(anyhow!("municipality needs polygon geometry")),
    }
}

pub(super) fn number(value: &Value) -> Result<Option<f64>> {
    if value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()) {
        return Ok(None);
    }
    let value = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .ok_or_else(|| anyhow!("invalid published number {value}"))?;
    ensure!(
        value.is_finite() && value >= 0.0,
        "negative or nonfinite published count"
    );
    Ok(Some(value))
}

pub(super) fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}

pub(super) fn csv(path: &Path, delimiter: u8) -> Result<Vec<Value>> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_path(path)?;
    let headers = reader
        .headers()?
        .iter()
        .map(|v| v.trim_start_matches('\u{feff}').to_owned())
        .collect::<Vec<_>>();
    reader
        .records()
        .map(|record| {
            let record = record?;
            Ok(Value::Object(
                headers
                    .iter()
                    .enumerate()
                    .map(|(i, h)| (h.clone(), Value::String(record.get(i).unwrap_or("").into())))
                    .collect(),
            ))
        })
        .collect()
}

pub(super) fn sheet(path: &Path, name: &str) -> Result<Vec<Vec<Option<String>>>> {
    use gdal::{Dataset, DatasetOptions, GdalOpenFlags, vector::LayerAccess};
    let dataset = Dataset::open_ex(
        path,
        DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_VECTOR,
            open_options: Some(&["FIELD_TYPES=STRING", "HEADERS=DISABLE"]),
            ..Default::default()
        },
    )?;
    let mut sheet = dataset.layer_by_name(name)?;
    sheet
        .features()
        .map(|feature| {
            (0..feature.field_count())
                .map(|i| feature.field_as_string(i).map_err(Into::into))
                .collect()
        })
        .collect()
}

pub(super) fn object_path(root: &Path, file: &Path) -> Result<PathBuf> {
    ensure!(
        file.components().all(|c| matches!(c, Component::Normal(_)))
            && !file.as_os_str().is_empty(),
        "publisher object must be relative"
    );
    Ok(root.join(file))
}

pub(super) fn pages(root: &Path, files: &[PathBuf]) -> Result<Vec<Value>> {
    read_pages(root, files, false)
}

pub(super) fn pages_latin1(root: &Path, files: &[PathBuf]) -> Result<Vec<Value>> {
    read_pages(root, files, true)
}

fn read_pages(root: &Path, files: &[PathBuf], latin1: bool) -> Result<Vec<Value>> {
    let paths = if files.is_empty() {
        ensure!(
            root.is_file(),
            "a directory needs declared publisher objects"
        );
        vec![root.to_path_buf()]
    } else {
        files
            .iter()
            .map(|file| object_path(root, file))
            .collect::<Result<Vec<_>>>()?
    };
    let mut features = Vec::new();
    for path in paths {
        let bytes = std::fs::read(path)?;
        let raw: Value = if latin1 {
            serde_json::from_str(&encoding_rs::WINDOWS_1252.decode(&bytes).0)?
        } else {
            serde_json::from_slice(&bytes)?
        };
        ensure!(raw.get("error").is_none(), "publisher service error");
        features.extend(
            raw["features"]
                .as_array()
                .ok_or_else(|| anyhow!("missing publisher features"))?
                .iter()
                .cloned(),
        );
    }
    Ok(features)
}

pub(super) fn centroid(feature: &Value) -> Result<[f64; 2]> {
    let parts = geometry(feature, &feature["properties"], &GeometryFormat::Geojson)?;
    let points = parts.iter().flatten().collect::<Vec<_>>();
    ensure!(!points.is_empty(), "empty count geometry");
    Ok([
        points.iter().map(|p| p[0]).sum::<f64>() / points.len() as f64,
        points.iter().map(|p| p[1]).sum::<f64>() / points.len() as f64,
    ])
}

pub(super) fn class_mask(rank: Option<u8>, ramp: bool) -> u16 {
    (0_u8..=12)
        .filter(|class| (*class >= 10) == ramp && *class <= if ramp { 12 } else { 4 })
        .filter(|class| {
            rank.is_none_or(|rank| {
                class
                    .saturating_sub(if ramp { 10 } else { 0 })
                    .abs_diff(rank)
                    <= 1
            })
        })
        .fold(0, |mask, class| mask | (1 << class))
}

pub(super) fn survey_year(
    row: &mut Observation,
    source: &Source,
    inputs: &YearInputs,
    year: i32,
) -> Result<bool> {
    let start = NaiveDate::from_ymd_opt(year, 1, 1).ok_or_else(|| anyhow!("invalid count year"))?;
    let end = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
    let (first, last) = source
        .input(inputs)?
        .observed
        .bounds()
        .map_err(anyhow::Error::msg)?;
    if start < first || end > last {
        return Ok(false);
    }
    row.observed_start = start.to_string();
    row.observed_end = end.to_string();
    Ok(true)
}

pub(super) fn projection(epsg: u32) -> Result<CoordTransform> {
    let mut source = SpatialRef::from_epsg(epsg)?;
    let mut target = SpatialRef::from_epsg(4326)?;
    source.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    target.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    Ok(CoordTransform::new(&source, &target)?)
}

pub(super) fn project(transform: &CoordTransform, point: [f64; 2]) -> Result<[f64; 2]> {
    let (mut x, mut y) = ([point[0]], [point[1]]);
    transform.transform_coords(&mut x, &mut y, &mut [])?;
    Ok([x[0], y[0]])
}

pub(super) fn split_heavy(
    total: f64,
    heavy: Option<f64>,
    motorcycle_share: f64,
    medium_share: f64,
) -> [Option<f64>; 4] {
    let moto = (total * motorcycle_share).round();
    match heavy {
        Some(large) => {
            let large = large.round().clamp(0.0, total - moto);
            let medium = (large * medium_share).round();
            [
                Some(total - moto - large),
                Some(medium),
                Some(large - medium),
                Some(moto),
            ]
        }
        None => [None, None, None, Some(moto)],
    }
}
