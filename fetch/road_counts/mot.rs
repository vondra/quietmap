//! Preserve Saudi MoT station locations and 24-hour totals instead of spreading their route mean nationwide.

use super::{Source, records};
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_target::{DistanceRule, ReferenceRewrite, ReferenceRule, WayAssignment},
    },
    year::YearInputs,
};
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub search_radius_m: f64,
    pub period_to_model_daily: f64,
    pub way_assignment: WayAssignment,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    for (index, p) in records::csv(path, b',')?.into_iter().enumerate() {
        let text = |key: &str| records::text(&p[key]);
        let number = |key: &str| {
            text(key)
                .replace([',', '"'], "")
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
        };
        let (Some(total), Some(lat), Some(lon)) = (
            number("24 Hour Total"),
            number("Latitude"),
            number("Longitude"),
        ) else {
            continue;
        };
        if total <= 0.0 || text("Road No.").trim().is_empty() {
            continue;
        }
        let mut row =
            recipe
                .source
                .observation(inputs, index.to_string(), CountBasis::BothDirections)?;
        row.reference = text("Road No.").trim().into();
        row.reference_required = true;
        row.geometry = vec![vec![[lon, lat]]];
        row.total = Some(total);
        row.time_basis = TimeBasis::PeriodDaily;
        row.daily_factor = recipe.period_to_model_daily;
        row.road_class_mask = 31;
        row.search_radius_m = recipe.search_radius_m;
        row.target.distance_rule = DistanceRule::PointToPiece;
        row.target.way_assignment = Some(recipe.way_assignment.clone());
        row.target.reference_rule = ReferenceRule::FirstValidToken;
        row.target.reference_rewrites = vec![ReferenceRewrite {
            pattern: "^[A-Za-z]*([0-9]+)$".into(),
            replacement: "$1".into(),
        }];
        rows.push(row);
    }
    Ok(rows)
}
