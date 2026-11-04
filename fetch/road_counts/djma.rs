//! Normalize Quebec's explicitly dated annual fields, never treating a blank truck percentage as zero.

use super::{Source, hpms, records};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{DistanceRule, ReferenceRewrite, ReferenceRule},
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
    annual_fields: Vec<String>,
    motorcycle_share: f64,
    medium_share_of_heavy: f64,
    missing_heavy_percent: Option<f64>,
    search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let total_pattern = regex::Regex::new(r"DJMA\s*:\s*([0-9]+)")?;
    let heavy_pattern = regex::Regex::new(r"%cam\s*:\s*([0-9.]+)")?;
    let year_pattern = regex::Regex::new(r"^\s*([0-9]{4})\s")?;
    let mut rows = Vec::new();
    for feature in raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("missing DJMA features"))?
    {
        let p = &feature["properties"];
        let route = p["rtss_debut"]
            .as_str()
            .and_then(|s| s.get(..5))
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(0);
        if route == 0 || route >= 1000 {
            continue;
        }
        let mut selected: Option<Observation> = None;
        for field in &recipe.annual_fields {
            let Some(value) = p[field].as_str() else {
                continue;
            };
            let Some(total) = total_pattern
                .captures(value)
                .and_then(|v| v[1].parse::<f64>().ok())
                .filter(|v| *v > 0.0)
            else {
                continue;
            };
            let Some(year) = year_pattern
                .captures(value)
                .and_then(|v| v[1].parse::<i32>().ok())
            else {
                return Err(anyhow!("undated DJMA annual value"));
            };
            let mut row = recipe.source.observation(
                inputs,
                hpms::identity(feature),
                CountBasis::BothDirections,
            )?;
            if !records::survey_year(&mut row, &recipe.source, inputs, year)? {
                continue;
            }
            let measured_heavy = heavy_pattern
                .captures(value)
                .and_then(|v| v[1].parse::<f64>().ok());
            let heavy = measured_heavy.or(recipe.missing_heavy_percent);
            if heavy.is_some_and(|v| !(0.0..=100.0).contains(&v)) {
                continue;
            }
            row.total = Some(total);
            row.vehicle_classes = records::split_heavy(
                total,
                heavy.map(|v| total * v / 100.0),
                recipe.motorcycle_share,
                recipe.medium_share_of_heavy,
            );
            row.estimated_classes = 15;
            if selected
                .as_ref()
                .is_none_or(|previous| previous.observed_end < row.observed_end)
            {
                selected = Some(row);
            }
        }
        let Some(mut row) = selected else { continue };
        row.reference = route.to_string();
        row.reference_required = true;
        row.target.reference_rule = ReferenceRule::FirstValidToken;
        row.target.reference_rewrites = vec![ReferenceRewrite {
            pattern: r"(?i)^(?:A-|R-)?0*([0-9]{1,3})(?:\s+(?:Est|Ouest|Nord|Sud))?$".into(),
            replacement: "$1".into(),
        }];
        row.target.distance_rule = DistanceRule::GreatCircle;
        row.geometry = vec![vec![records::centroid(feature)?]];
        let rank = if route <= 99 || route >= 400 {
            0
        } else if route <= 199 {
            1
        } else if route <= 299 {
            2
        } else {
            3
        };
        row.road_class_mask = records::class_mask(Some(rank), false);
        row.search_radius_m = recipe.search_radius_m;
        rows.push(row);
    }
    Ok(rows)
}
