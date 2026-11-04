//! Decode MITMA's data assignment as JSON and retain real multipart section geometry.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    hpms, records,
};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{Identity, IdentityField},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    search_radius_m: f64,
    road_class_mask: u16,
}

pub(super) fn convert(recipe: &Recipe, inputs: &YearInputs, raw: &str) -> Result<Vec<Observation>> {
    let start = raw
        .find('{')
        .ok_or_else(|| anyhow!("missing MITMA data object"))?;
    ensure!(
        regex::Regex::new(r"^\s*var\s+tramos_data\s*=\s*$")?.is_match(&raw[..start]),
        "unknown MITMA assignment"
    );
    let value: Value = serde_json::from_str(raw[start..].trim().trim_end_matches(';'))?;
    let features = value["features"]
        .as_array()
        .ok_or_else(|| anyhow!("missing MITMA features"))?;
    ensure!(
        value["totalFeatures"].as_u64() == Some(features.len() as u64),
        "incomplete MITMA feature set"
    );
    let mut rows = Vec::new();
    for feature in features {
        let p = &feature["properties"];
        let number = |field| -> Result<Option<f64>> {
            if p[field] == "NULL" {
                Ok(None)
            } else {
                records::number(&p[field])
            }
        };
        let (Some(total), Some(light), Some(heavy)) =
            (number("imdtot")?, number("imdlig")?, number("imdpes")?)
        else {
            continue;
        };
        if total <= 0.0 || light + heavy != total {
            continue;
        }
        let Some(reference) = p["via"].as_str() else {
            continue;
        };
        let reference = reference
            .split(';')
            .next()
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_uppercase();
        if reference.is_empty() || !reference.bytes().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let mut row = recipe.source.observation(
            inputs,
            hpms::identity(feature),
            CountBasis::BothDirections,
        )?;
        row.geometry = geometry(feature, p, &GeometryFormat::Geojson)?;
        row.road_class_mask = recipe.road_class_mask;
        let pattern = reference
            .chars()
            .map(|c| regex::escape(&c.to_string()))
            .collect::<Vec<_>>()
            .join("-*");
        row.target.identities = vec![Identity {
            field: IdentityField::Reference,
            pattern: format!("^-*{pattern}-*(?:;|$)"),
            road_class_mask: recipe.road_class_mask,
            priority: 0,
            prefer_first_match: false,
        }];
        row.search_radius_m = recipe.search_radius_m;
        row.total = Some(total);
        records::observe_heavy_group(&mut row, Some(heavy));
        row.estimated_classes = 15;
        rows.push(row);
    }
    Ok(rows)
}
