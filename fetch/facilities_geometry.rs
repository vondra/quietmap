//! Decode publisher point and area geometry before the common facility contract.

use super::classify;
use crate::facilities::model::InputSpec;
use anyhow::Result;
use geos::{Geom, Geometry};
use serde_json::Value;

pub struct Location {
    pub center: [f64; 2],
    pub polygon: Option<Vec<u8>>,
}

pub fn location(feature: &Value, properties: &Value, spec: &InputSpec) -> Result<Option<Location>> {
    let number = |fields: &[&str]| classify::number(&classify::field(properties, fields));
    if spec.format == "gppd-csv" || spec.format == "eprtr-json" {
        let fields = if spec.format == "gppd-csv" {
            (["longitude"], ["latitude"])
        } else {
            (["x_4326"], ["y_4326"])
        };
        return Ok(number(&fields.0)
            .zip(number(&fields.1))
            .map(|(lon, lat)| Location {
                center: [lon, lat],
                polygon: None,
            }));
    }
    let g = &feature["geometry"];
    let kind = g["type"].as_str().unwrap_or("");
    if !spec.geometry_types.is_empty() && !spec.geometry_types.iter().any(|s| s == kind) {
        return Ok(None);
    }
    if kind == "Point" || kind == "MultiPoint" {
        let point = if kind == "Point" {
            &g["coordinates"]
        } else {
            &g["coordinates"][0]
        };
        let fallback = || point[0].as_f64().zip(point[1].as_f64());
        let values = if spec.format == "gem-tracker" {
            number(&["Longitude", "longitude"])
                .zip(number(&["Latitude", "latitude"]))
                .or_else(fallback)
        } else {
            fallback()
        };
        return Ok(values.map(|(x, y)| Location {
            center: [x, y],
            polygon: None,
        }));
    }
    if matches!(kind, "Polygon" | "MultiPolygon") {
        let geometry = Geometry::new_from_geojson(&g.to_string())?;
        let centre = geometry.get_centroid()?;
        return Ok(Some(Location {
            center: [centre.get_x()?, centre.get_y()?],
            polygon: Some(geometry.to_wkb()?),
        }));
    }
    Ok(None)
}
