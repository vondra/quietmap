//! Normalize harmonized municipal counts with published classes, weekday basis and street or carriageway support.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    hpms, records,
};
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_target::{Proximity, WayAssignment},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub weekday_to_model_daily: f64,
    pub modeled: bool,
    pub placeholder_zero_trucks: bool,
    pub search_radius_m: f64,
    pub minimum_alignment_cosine: f64,
}

fn first_number(fields: &Value, keys: &[&str]) -> Result<Option<f64>> {
    for key in keys {
        if let Some(value) = records::number(&fields[key])? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn road_class(value: &str) -> Option<u8> {
    match value {
        "motorway" => Some(0),
        "trunk" => Some(1),
        "primary" => Some(2),
        "secondary" | "secondary_link" => Some(3),
        "tertiary" | "tertiary_link" => Some(4),
        "residential" => Some(5),
        "living_street" => Some(6),
        "service" => Some(7),
        "track" => Some(8),
        "unclassified" => Some(9),
        "motorway_link" => Some(10),
        "trunk_link" => Some(11),
        "primary_link" => Some(12),
        _ => None,
    }
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let city = recipe
        .source
        .input(inputs)?
        .region
        .split(':')
        .nth(1)
        .ok_or_else(|| anyhow!("municipal input lacks its city"))?;
    let mut rows = Vec::new();
    let features = raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("city features missing"))?;
    for (index, feature) in features.iter().enumerate() {
        let p = &feature["properties"];
        let weekday = p["AADT"].is_null() && !p["AAWT"].is_null();
        let Some(total) = first_number(p, &["AADT", "AAWT"])? else {
            continue;
        };
        let percentage = first_number(p, &["TR_pct_AADT", "TR_pct_AAWT"])?;
        let mut truck = first_number(p, &["TR_AADT", "TR_AAWT"])?
            .or_else(|| percentage.map(|v| total * v / 100.0));
        if recipe.placeholder_zero_trucks && truck == Some(0.0) {
            truck = None;
        }
        let moto = first_number(p, &["2W_AADT", "2W_AAWT"])?;
        let factor = if weekday {
            recipe.weekday_to_model_daily
        } else {
            1.0
        };
        ensure!(
            factor.is_finite() && factor > 0.0,
            "invalid city weekday conversion"
        );
        if truck.unwrap_or(0.0) + moto.unwrap_or(0.0) > total || (total * factor).round() == 0.0 {
            continue;
        }
        // Independently rounded components can contradict the rounded total; reject that record.
        if truck.map_or(0.0, |v| (v * factor).round()) + moto.map_or(0.0, |v| (v * factor).round())
            > (total * factor).round()
        {
            continue;
        }
        let directional = p["raw_oneway"].as_bool() == Some(true);
        let mut row = recipe.source.observation(
            inputs,
            hpms::identity(&json!({"city":city,"feature":index,"observation":feature})),
            if directional {
                CountBasis::Directional
            } else {
                CountBasis::StreetCrossSection
            },
        )?;
        row.time_basis = if weekday {
            TimeBasis::WeekdayDaily
        } else {
            TimeBasis::AnnualDaily
        };
        row.daily_factor = factor;
        row.measured = !recipe.modeled;
        row.total = Some(if weekday { total } else { total.round() });
        row.vehicle_classes = [
            None,
            None,
            truck.map(|v| if weekday { v } else { v.round() }),
            moto.map(|v| if weekday { v } else { v.round() }),
        ];
        row.estimated_classes = if recipe.modeled
            || weekday
            || records::text(&p["raw_techno"])
                .to_lowercase()
                .contains("estimat")
        {
            15
        } else {
            3 | if truck.is_none() { 4 } else { 0 } | if moto.is_none() { 8 } else { 0 }
        };
        row.geometry = geometry(feature, p, &GeometryFormat::Geojson)?;
        for part in &mut row.geometry {
            part.dedup();
        }
        let point = row.geometry.len() == 1 && row.geometry[0].len() == 1;
        row.direction = u8::from(directional && !point);
        row.road_class_mask = (1 << 13) - 1;
        row.search_radius_m = recipe.search_radius_m;
        row.target.source_way = p["osmid"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0 && v.fract() == 0.0 && *v < i64::MAX as f64)
            .map(|v| v as i64);
        let compatible = road_class(p["osm_type"].as_str().unwrap_or("")).map_or(0, |class| {
            if class < 5 {
                records::class_mask(Some(class), false)
            } else if class < 10 {
                (1 << 5) | (1 << 9)
            } else {
                0
            }
        });
        let names = ["osm_name", "raw_name"]
            .iter()
            .map(|key| {
                records::text(&p[key])
                    .nfkc()
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|v| !v.is_empty())
            .collect();
        row.target.proximity = Some(Proximity {
            allowed_classes: 31 | (1 << 5) | (1 << 9),
            compatible_classes: compatible,
            street_names: names,
        });
        row.target.minimum_alignment_cosine = recipe.minimum_alignment_cosine;
        if directional && point {
            row.target.way_assignment = Some(WayAssignment {
                maximum_distance_m: recipe.search_radius_m,
                clearance_m: recipe.search_radius_m,
                require_class_clearance: false,
                extend_along_way: false,
            });
        }
        rows.push(row);
    }
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(rows)
}
