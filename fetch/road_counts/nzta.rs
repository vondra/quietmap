//! Convert NZTA carriageway estimates and Auckland street cross-sections without inventing missing heavy counts.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

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
    pub motorcycle_share: f64,
    pub medium_share_of_heavy: f64,
    pub search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    let ramps = regex::Regex::new(r"-R\d+\b|\bRMP\b")?;
    for (index, feature) in records::pages(path, &recipe.files)?.into_iter().enumerate() {
        let fields = &feature["properties"];
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
        row.measured = !estimate.is_some_and(|v| v.round() > 0.0);
        row.total = Some(total.round());
        row.vehicle_classes = records::split_heavy(
            total.round(),
            heavy.map(|percent| total.round() * percent / 100.0),
            recipe.motorcycle_share,
            recipe.medium_share_of_heavy,
        );
        row.estimated_classes = 15;
        row.road_class_mask = records::class_mask(
            rank,
            recipe.carriageways && ramps.is_match(&records::text(&fields["roadName"])),
        );
        row.geometry = vec![vec![point]];
        row.search_radius_m = recipe.search_radius_m;
        rows.push(row);
    }
    Ok(rows)
}
