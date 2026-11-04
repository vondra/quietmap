//! Decode declared JSON point and line geometry into multipart WGS84 coordinates.

use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum GeometryFormat {
    Geojson,
    Arcgis,
    Point { longitude: String, latitude: String },
}

pub(super) fn geometry(
    value: &Value,
    properties: &Value,
    format: &GeometryFormat,
) -> Result<Vec<Vec<[f64; 2]>>> {
    let lines: Vec<Value> = match format {
        GeometryFormat::Point {
            longitude,
            latitude,
        } => vec![serde_json::json!([[
            properties[longitude],
            properties[latitude]
        ]])],
        GeometryFormat::Arcgis if value["geometry"]["x"].is_number() => vec![serde_json::json!([
            [value["geometry"]["x"], value["geometry"]["y"]]
        ])],
        GeometryFormat::Arcgis => value["geometry"]["paths"]
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("missing ArcGIS paths"))?,
        GeometryFormat::Geojson => match value["geometry"]["type"].as_str() {
            Some("Point") => vec![serde_json::json!([value["geometry"]["coordinates"]])],
            Some("MultiPoint") => value["geometry"]["coordinates"]
                .as_array()
                .ok_or_else(|| anyhow!("missing point coordinates"))?
                .iter()
                .map(|p| serde_json::json!([p]))
                .collect(),
            Some("LineString") => vec![value["geometry"]["coordinates"].clone()],
            Some("MultiLineString") => value["geometry"]["coordinates"]
                .as_array()
                .cloned()
                .ok_or_else(|| anyhow!("missing line parts"))?,
            _ => return Err(anyhow!("road observation requires point or line geometry")),
        },
    };
    lines
        .into_iter()
        .map(|line| {
            line.as_array()
                .ok_or_else(|| anyhow!("invalid road line"))?
                .iter()
                .map(|point| {
                    let coordinates = point
                        .as_array()
                        .ok_or_else(|| anyhow!("invalid road coordinate"))?;
                    ensure!(coordinates.len() >= 2, "short road coordinate");
                    Ok([
                        coordinates[0]
                            .as_f64()
                            .ok_or_else(|| anyhow!("invalid longitude"))?,
                        coordinates[1]
                            .as_f64()
                            .ok_or_else(|| anyhow!("invalid latitude"))?,
                    ])
                })
                .collect()
        })
        .collect()
}
