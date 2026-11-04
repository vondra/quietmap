//! Convert the ANAS point census while retaining its observed monthly daily-average basis.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    records,
};
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_target::{ReferenceRewrite, ReferenceRule},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub search_radius_m: f64,
    pub period_to_model_daily: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let syntax = regex::Regex::new(
        r"(?i)^(A|RA|SS|NSA)\s*0*(\d+)\s*(?:dir(?:-[a-z])?|bis|ter|quater|radd)?$",
    )?;
    let mut rows = Vec::new();
    for (index, feature) in raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("ANAS features missing"))?
        .iter()
        .enumerate()
    {
        if feature["geometry"].is_null() {
            continue;
        }
        let fields = &feature["properties"];
        let reference = records::text(&fields["Strada"]);
        let Some(parts) = syntax.captures(reference.trim()) else {
            continue;
        };
        let Some(total) = records::number(&fields["TGM"])?.filter(|x| x.round() > 0.0) else {
            continue;
        };
        let mut row =
            recipe
                .source
                .observation(inputs, index.to_string(), CountBasis::BothDirections)?;
        row.reference = format!("{}{}", parts[1].to_uppercase(), &parts[2]);
        row.reference_required = true;
        row.geometry = geometry(feature, fields, &GeometryFormat::Geojson)?;
        row.road_class_mask = 31;
        row.search_radius_m = recipe.search_radius_m;
        row.time_basis = TimeBasis::PeriodDaily;
        row.daily_factor = recipe.period_to_model_daily;
        row.total = Some(total.round());
        row.target.reference_rule = ReferenceRule::FirstValidToken;
        row.target.reference_rewrites = vec![
            ReferenceRewrite {
                pattern: r"(?i)^(?:SR|SS|S\.S\.)\s*0*(\d+)$".into(),
                replacement: "SS$1".into(),
            },
            ReferenceRewrite {
                pattern: syntax.as_str().into(),
                replacement: "$1$2".into(),
            },
        ];
        rows.push(row);
    }
    Ok(rows)
}
