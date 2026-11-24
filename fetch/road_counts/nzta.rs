//! Convert NZTA carriageway estimates and Auckland street cross-sections without inventing missing heavy counts.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn count(value: &serde_json::Value) -> Result<Option<f64>> {
    // A negative published percentage cannot describe vehicles; keep its total and missing split.
    if value.as_f64().is_some_and(|v| v < 0.0) {
        Ok(None)
    } else {
        records::number(value)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub files: Vec<PathBuf>,
    pub carriageways: bool,
    pub search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let input = recipe.source.input(inputs)?;
    let (first, last) = input.observed.bounds().map_err(anyhow::Error::msg)?;
    let cutoff = NaiveDate::parse_from_str(&inputs.manifest.as_of, "%Y-%m-%d")?;
    let mut rows = Vec::new();
    let mut auckland_latest =
        BTreeMap::<(i64, i64, String), (NaiveDate, usize, Observation)>::new();
    let ramps = regex::Regex::new(r"-R\d+\b|\bRMP\b")?;
    for (index, feature) in records::pages(path, &recipe.files)?.into_iter().enumerate() {
        let fields = &feature["properties"];
        let observed_day = if !recipe.carriageways {
            let raw = records::text(&fields["count_date"]);
            let date_part = raw.split('T').next().unwrap_or(raw.as_str()).trim();
            let day = NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
                .map_err(|error| anyhow!("invalid Auckland count_date {raw:?}: {error}"))?;
            if !records::admitted_date(
                day,
                day,
                first,
                last,
                cutoff,
                input.fallback_vintage.is_some(),
            ) {
                continue;
            }
            Some(day)
        } else {
            None
        };
        let estimate = if recipe.carriageways {
            count(&fields["trafficADTEst"])?
        } else {
            None
        };
        let observed = count(
            &fields[if recipe.carriageways {
                "trafficADTCount"
            } else {
                "adt"
            }],
        )?;
        let Some(total) = estimate
            .filter(|v| v.round() > 0.0)
            .or(observed.filter(|v| v.round() > 0.0))
        else {
            continue;
        };
        let rank = if recipe.carriageways {
            Some(match fields["ONRC"].as_str() {
                Some("High Volume" | "National") => 0,
                Some("Regional") => 1,
                Some("Arterial") => 2,
                Some("Primary Collector") => 3,
                Some("Secondary Collector") => 4,
                _ => continue,
            })
        } else {
            None
        };
        let point = records::centroid(&feature)?;
        let heavy = count(
            &fields[if recipe.carriageways {
                "loadingPcHeavy"
            } else {
                "pcheavy"
            }],
        )?
        .filter(|v| *v <= 100.0);
        let mut row = recipe.source.observation(
            inputs,
            format!("{}:{index}", recipe.source.input_id),
            if recipe.carriageways {
                CountBasis::BothDirections
            } else {
                CountBasis::StreetCrossSection
            },
        )?;
        if let Some(day) = observed_day {
            row.observed_start = day.to_string();
            row.observed_end = day.to_string();
        }
        row.measured = !estimate.is_some_and(|v| v.round() > 0.0);
        row.total = Some(total.round());
        records::observe_heavy_group(
            &mut row,
            heavy.map(|percent| total.round() * percent / 100.0),
        );
        row.estimated_classes = 15;
        row.road_class_mask = records::class_mask(
            rank,
            recipe.carriageways && ramps.is_match(&records::text(&fields["roadName"])),
        );
        row.geometry = vec![vec![point]];
        row.search_radius_m = recipe.search_radius_m;
        if let Some(day) = observed_day {
            // Multiple historical survey years at the same Auckland site tie in road matching;
            // keep only the latest eligible observation for each (coordinate, road_name) site.
            let key = (
                (point[0] * 1e6).round() as i64,
                (point[1] * 1e6).round() as i64,
                records::text(&fields["road_name"]).trim().to_uppercase(),
            );
            let replace = auckland_latest
                .get(&key)
                .is_none_or(|(prev_day, prev_idx, _)| (day, index) > (*prev_day, *prev_idx));
            if replace {
                auckland_latest.insert(key, (day, index, row));
            }
        } else {
            rows.push(row);
        }
    }
    if !auckland_latest.is_empty() {
        let mut selected: Vec<(usize, Observation)> = auckland_latest
            .into_values()
            .map(|(_, idx, row)| (idx, row))
            .collect();
        selected.sort_by_key(|(idx, _)| *idx);
        rows.extend(selected.into_iter().map(|(_, row)| row));
    }
    Ok(rows)
}
