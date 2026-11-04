//! FHWA sections retain their measured scope and multipart lines; declared split tables supply missing classes.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    hpms_sections::{Section, class_mask, relabel_twins},
};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    files: Vec<PathBuf>,
    functional_class_max: u8,
    search_radius_m: f64,
    minimum_alignment_cosine: f64,
    twin_radius_m: f64,
    twin_total_ratio_tolerance: f64,
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(k, v)| format!(
                    "{}:{}",
                    serde_json::to_string(k).unwrap(),
                    canonical_json(v)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Number(v) if v.as_f64().is_some_and(|n| n.fract() == 0.0) => {
            format!("{:.0}", v.as_f64().unwrap())
        }
        _ => value.to_string(),
    }
}

pub(super) fn identity(feature: &Value) -> String {
    let p = &feature["properties"];
    for id in [
        &feature["id"],
        &p["id"],
        &p["OBJECTID"],
        &p["ObjectId"],
        &p["internal_id"],
    ] {
        if id.is_number() {
            return canonical_json(id);
        }
        if let Some(id) = id.as_str().filter(|s| !s.is_empty()) {
            return id.into();
        }
    }
    format!("{:x}", Sha256::digest(canonical_json(feature).as_bytes()))
}

fn number(value: &Value) -> Result<f64> {
    let n = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .ok_or_else(|| anyhow!("invalid HPMS number"))?;
    ensure!(n.is_finite() && n >= 0.0, "invalid HPMS number");
    Ok(n)
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    directory: &Path,
) -> Result<Vec<Observation>> {
    ensure!(!recipe.files.is_empty(), "no declared HPMS objects");
    ensure!(
        (1..=7).contains(&recipe.functional_class_max),
        "invalid HPMS class range"
    );
    let mut sections = Vec::new();
    let mut terminal = false;
    for file in &recipe.files {
        ensure!(!terminal, "declared HPMS page follows terminal page");
        ensure!(
            file.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "HPMS object must be relative"
        );
        let raw: Value = serde_json::from_slice(&std::fs::read(directory.join(file))?)?;
        ensure!(raw.get("error").is_none(), "HPMS service error");
        let features = raw["features"]
            .as_array()
            .ok_or_else(|| anyhow!("missing HPMS features"))?;
        terminal = features.is_empty();
        for feature in features {
            let p = &feature["properties"];
            let total = number(&p["AADT"])?.trunc();
            if total == 0.0 {
                continue;
            }
            let class = number(&p["F_SYSTEM"])?;
            ensure!(class.fract() == 0.0, "fractional HPMS functional class");
            if class < 1.0 || class > f64::from(recipe.functional_class_max) {
                continue;
            }
            let rank = class as u8 - 1;
            let facility = p
                .get("FACILITY_TYPE")
                .ok_or_else(|| anyhow!("HPMS omits FACILITY_TYPE"))?;
            let facility = if facility.is_null() {
                None
            } else {
                let n = number(facility)?;
                ensure!(
                    n.fract() == 0.0 && n <= u8::MAX as f64,
                    "invalid HPMS facility type"
                );
                Some(n as u8)
            };
            let basis = match facility {
                None => CountBasis::Unknown,
                Some(1 | 4) => CountBasis::Directional,
                _ => CountBasis::BothDirections,
            };
            let mut row = recipe
                .source
                .observation(inputs, identity(feature), basis)?;
            row.geometry = geometry(feature, p, &GeometryFormat::Geojson)?;
            let mut centroid = [0.0; 2];
            let mut vertices = 0;
            for point in row.geometry.iter().flatten() {
                centroid[0] += point[0];
                centroid[1] += point[1];
                vertices += 1;
            }
            ensure!(vertices > 0, "empty HPMS geometry");
            centroid.iter_mut().for_each(|v| *v /= vertices as f64);
            row.total = Some(total);
            row.estimated_classes = 15;
            row.search_radius_m = recipe.search_radius_m;
            row.target.minimum_alignment_cosine = recipe.minimum_alignment_cosine;
            row.target.distance_rule = crate::roads::observation_target::DistanceRule::NearestPiece;
            row.road_class_mask = class_mask(rank, facility);
            if row.road_class_mask == 0 {
                continue;
            }
            sections.push(Section {
                row,
                rank,
                facility,
                centroid,
            });
        }
    }
    ensure!(
        terminal && !sections.is_empty(),
        "HPMS needs its declared empty terminal page"
    );
    relabel_twins(
        &mut sections,
        recipe.twin_radius_m,
        recipe.twin_total_ratio_tolerance,
    );
    sections.sort_by(|a, b| a.row.identity.cmp(&b.row.identity));
    Ok(sections.into_iter().map(|s| s.row).collect())
}
