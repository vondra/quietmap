//! SICT segment totals join their measured vehicle composition without treating legacy status bits as missing data.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub composition_input: String,
    total_field: String,
    fallback_shares: [f64; 4],
    search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    ensure!(
        recipe
            .fallback_shares
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            && (recipe.fallback_shares.iter().sum::<f64>() - 1.0).abs() < 1e-12,
        "invalid SICT fallback composition"
    );
    let composition_path = inputs
        .by_id(&recipe.composition_input)
        .map_err(anyhow::Error::msg)?;
    let mut composition = BTreeMap::new();
    let mut csv = csv::Reader::from_path(composition_path)?;
    for record in csv.deserialize::<BTreeMap<String, String>>() {
        let record = record?;
        let id = record
            .get("segment_mongo_id")
            .ok_or_else(|| anyhow!("missing SICT composition identity"))?;
        let group = |keys: &[&str]| -> Option<f64> {
            let values = keys
                .iter()
                .map(|k| record.get(*k)?.trim().parse::<f64>().ok())
                .collect::<Option<Vec<_>>>()?;
            values
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
                .then(|| values.iter().sum())
        };
        let values = [
            group(&["comp_A", "comp_OTROS"]),
            group(&["comp_B", "comp_C2"]),
            group(&["comp_C3", "comp_T3S2", "comp_T3S3", "comp_T3S2R4"]),
            group(&["comp_M"]),
        ];
        let shares = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .and_then(|values| {
                let sum = values.iter().sum::<f64>();
                (sum > 0.0).then(|| std::array::from_fn::<_, 4, _>(|i| values[i] / sum))
            });
        composition.insert(id.clone(), shares);
    }
    let raw: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let features = raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("missing SICT features"))?;
    let mut rows: Vec<Observation> = Vec::new();
    let mut positions = BTreeMap::<String, usize>::new();
    for feature in features {
        let p = &feature["properties"];
        let Some(total) = p[&recipe.total_field]
            .as_f64()
            .or_else(|| p[&recipe.total_field].as_str()?.trim().parse().ok())
            .filter(|n| n.is_finite() && *n > 0.0)
        else {
            continue;
        };
        let total = total.round();
        let id = p["segment_mongo_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing SICT segment identity"))?;
        let Ok(lines) = geometry(feature, p, &GeometryFormat::Geojson) else {
            continue;
        };
        if lines.is_empty()
            || lines.iter().any(|p| {
                p.len() < 2
                    || p.iter()
                        .any(|p| !crate::roads::observation_target::coordinate(p))
            })
        {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, id.into(), CountBasis::BothDirections)?;
        let shares = composition
            .get(id)
            .and_then(|r| *r)
            .unwrap_or(recipe.fallback_shares);
        let [_, medium, heavy, motorcycle] = shares.map(|s| (total * s).round());
        row.vehicle_classes = [
            total - medium - heavy - motorcycle,
            medium,
            heavy,
            motorcycle,
        ]
        .map(Some);
        row.total = Some(total);
        row.geometry = lines;
        row.estimated_classes = if composition.get(id).is_some_and(Option::is_some) {
            0
        } else {
            15
        };
        row.search_radius_m = recipe.search_radius_m;
        row.road_class_mask = match (p["red_ok"].as_str(), p["operacion"].as_str()) {
            (Some("Federal"), Some("Cuota")) => 3,
            (Some("Federal"), _) => 6,
            _ => 12,
        };
        if let Some(&index) = positions.get(id) {
            let parts = std::mem::take(&mut row.geometry);
            row.geometry.clone_from(&rows[index].geometry);
            ensure!(
                row == rows[index],
                "conflicting SICT values for one observation identity"
            );
            rows[index].geometry.extend(parts);
        } else {
            positions.insert(id.into(), rows.len());
            rows.push(row);
        }
    }
    ensure!(!rows.is_empty(), "SICT input has no usable observations");
    Ok(rows)
}
