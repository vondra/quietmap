//! Normalize Trafikverket link-part counts with their measured class periods and travel direction.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    records,
};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use gdal::{Dataset, vector::LayerAccess};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    layer: String,
    search_radius_m: f64,
    minimum_alignment_cosine: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let dataset = Dataset::open(path.canonicalize()?)?;
    let mut layer = dataset.layer_by_name(&recipe.layer)?;
    ensure!(
        layer
            .spatial_ref()
            .ok_or_else(|| anyhow!("missing NVDB CRS"))?
            .auth_code()?
            == 3006,
        "NVDB Trafik must use SWEREF 99 TM"
    );
    let transform = records::projection(3006)?;
    let mut rows = Vec::new();
    for feature in layer.features() {
        let string = |name: &str| -> Result<Option<String>> {
            Ok(feature.field_as_string(feature.field_index(name)?)?)
        };
        let number = |name: &str| -> Result<Option<f64>> {
            Ok(feature.field_as_double(feature.field_index(name)?)?)
        };
        if !matches!(
            string("Matmetod")?.as_deref(),
            Some("Stickprovsmätning" | "Helårsmätning")
        ) {
            continue;
        }
        let Some(total) = number("Adt_samtliga_fordon")?.filter(|v| *v > 0.0) else {
            continue;
        };
        let mut counts = [None; 4];
        for (class, category) in ["latta", "medeltunga", "tunga"].iter().enumerate() {
            let values = ["06_18", "18_22", "22_06"]
                .map(|period| number(&format!("Adt_{category}_fordon_{period}")))
                .into_iter()
                .collect::<Result<Vec<_>>>()?;
            if values.iter().all(Option::is_some) {
                counts[class] = Some(values.iter().flatten().sum::<f64>());
            }
        }
        if counts[..3].iter().any(Option::is_none) {
            continue;
        }
        if (counts.iter().flatten().sum::<f64>() - total).abs() > 4.0 {
            continue;
        }
        let directional = matches!(
            string("ROLE")?.as_deref(),
            Some("Syskon fram" | "Syskon bak")
        );
        let direction = string("DIRECTION")?;
        if directional && !matches!(direction.as_deref(), Some("Med" | "Mot")) {
            continue;
        }
        let Some(raw_geometry) = feature.geometry() else {
            continue;
        };
        let transformed = raw_geometry.transform(&transform)?;
        let raw: Value = serde_json::from_str(&transformed.json()?)?;
        let mut parts = geometry(
            &json!({"geometry":raw}),
            &Value::Null,
            &GeometryFormat::Geojson,
        )?;
        if directional && direction.as_deref() == Some("Mot") {
            for part in &mut parts {
                part.reverse();
            }
        }
        let identity = feature
            .field_index("id")
            .ok()
            .map(|index| feature.field_as_string(index))
            .transpose()?
            .flatten()
            .or_else(|| feature.fid().map(|v| v.to_string()))
            .ok_or_else(|| anyhow!("missing NVDB identity"))?;
        let mut row = recipe.source.observation(
            inputs,
            identity,
            if directional {
                CountBasis::Directional
            } else {
                CountBasis::BothDirections
            },
        )?;
        let Some(period) = number("Matarsperiod")? else {
            continue;
        };
        let year = (period as i32) / 100;
        if !records::survey_year(&mut row, &recipe.source, inputs, year)? {
            continue;
        }
        row.geometry = parts;
        row.direction = u8::from(directional);
        row.road_class_mask = records::class_mask(None, false);
        row.search_radius_m = recipe.search_radius_m;
        row.target.minimum_alignment_cosine = recipe.minimum_alignment_cosine;
        // Trafik DPS defines light as all vehicles below 3.5 t; its separate motorcycle series is summer-only.
        let light_and_motorcycles = total - counts[1].unwrap() - counts[2].unwrap();
        if light_and_motorcycles < 0.0 {
            continue;
        }
        row.total = Some(total);
        row.vehicle_classes = [None, counts[1], counts[2], None];
        row.vehicle_groups = vec![crate::roads::observation_groups::VehicleGroup {
            classes: 9,
            count: light_and_motorcycles,
        }];
        row.estimated_classes = 9;
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        a.identity
            .parse::<i64>()
            .unwrap()
            .cmp(&b.identity.parse::<i64>().unwrap())
    });
    Ok(rows)
}
