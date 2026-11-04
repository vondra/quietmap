//! Convert Vienna's declared calendar-year monthly all-day counts into station cross-sections with missing classes intact.

use super::{Source, records};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{DistanceRule, StreetAssignment},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub sites_input: String,
    pub coverage: records::Coverage,
    pub minimum_months: usize,
    pub road_class_mask: u16,
    pub search_radius_m: f64,
    pub point_band_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let (first, last) = recipe
        .source
        .input(inputs)?
        .observed
        .bounds()
        .map_err(anyhow::Error::msg)?;
    ensure!(
        first.year() == last.year(),
        "Vienna edition must name one calendar year"
    );
    let year = first.year();
    let coverage = records::coverage(
        inputs,
        &recipe.coverage.input_id,
        &recipe.coverage.property,
        &recipe.coverage.name,
    )?;
    let locations: Value = serde_json::from_slice(&std::fs::read(
        inputs
            .by_id(&recipe.sites_input)
            .map_err(anyhow::Error::msg)?,
    )?)?;
    let mut sites = BTreeMap::new();
    for feature in locations["features"]
        .as_array()
        .ok_or_else(|| anyhow!("Vienna locations missing"))?
    {
        let id = feature["properties"]["ZST_ID"]
            .as_i64()
            .ok_or_else(|| anyhow!("invalid Vienna station identity"))?;
        let geometry = super::fields::geometry(
            feature,
            &Value::Null,
            &super::fields::GeometryFormat::Geojson,
        )?;
        ensure!(
            sites.insert(id, geometry).is_none(),
            "duplicate Vienna station"
        );
    }
    let bytes = std::fs::read(path)?;
    let text = encoding_rs::WINDOWS_1252.decode(&bytes).0;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .from_reader(text.as_bytes());
    let headers = reader.headers()?.clone();
    let mut stations = BTreeMap::<i64, (String, BTreeMap<u32, [Option<f64>; 2]>)>::new();
    let months = [
        "JAN.", "FEB.", "MÄRZ", "APRIL", "MAI", "JUNI", "JULI", "AUG.", "SEP.", "OKT", "NOV",
        "DEZ.",
    ];
    for record in reader.records() {
        let record = record?;
        let value = |name: &str| {
            headers
                .iter()
                .position(|v| v == name)
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .trim()
        };
        if value("JAHR").parse::<i32>().ok() != Some(year) || value("RINAME") != "Gesamt" {
            continue;
        }
        let (Some(id), Some(count)) = (
            value("ZNR").parse::<i64>().ok(),
            value("DTVMS")
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0),
        ) else {
            continue;
        };
        let Some(month) = months.iter().position(|m| *m == value("MONAT")) else {
            continue;
        };
        let category = match value("FZTYP") {
            "Kfz" => 0,
            "LkwÄ" => 1,
            _ => continue,
        };
        let station = stations
            .entry(id)
            .or_insert((value("ZNAME").into(), BTreeMap::new()));
        let cell = station.1.entry(month as u32 + 1).or_default();
        ensure!(
            cell[category].is_none(),
            "duplicate Vienna station-month-class"
        );
        cell[category] = Some(count);
    }
    let mut rows = Vec::new();
    for (id, (name, months)) in stations {
        let Some(geometry) = sites.remove(&id) else {
            continue;
        };
        let months = months
            .into_iter()
            .filter(|(_, values)| values[0].is_some_and(|v| v > 0.0))
            .collect::<Vec<_>>();
        if months.len() < recipe.minimum_months {
            continue;
        }
        let (mut days, mut total, mut heavy) = (0.0, 0.0, Some(0.0));
        for (month, values) in months {
            let start = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
            let end = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            }
            .unwrap();
            let weight = (end - start).num_days() as f64;
            days += weight;
            total += values[0].unwrap() * weight;
            heavy = heavy
                .zip(values[1])
                .map(|(sum, value)| sum + value * weight);
        }
        total /= days;
        heavy = heavy.map(|v| v / days);
        if heavy.is_some_and(|v| v > total) {
            continue;
        }
        let mut row =
            recipe
                .source
                .observation(inputs, id.to_string(), CountBasis::StreetCrossSection)?;
        row.geometry = geometry;
        row.total = Some(total.round());
        super::records::observe_heavy_group(&mut row, heavy.map(f64::round));
        row.estimated_classes = 15;
        row.road_class_mask = recipe.road_class_mask;
        row.search_radius_m = recipe.search_radius_m;
        row.target.coverage = coverage.clone();
        row.target.distance_rule = DistanceRule::PointToPiece;
        row.target.street_assignment = Some(StreetAssignment {
            candidate_classes: 31 | (1 << 5) | (1 << 9) | (7 << 10),
            point_band_m: recipe.point_band_m,
            line_near_m: 0.0,
            street_hint: name,
        });
        rows.push(row);
    }
    Ok(rows)
}
