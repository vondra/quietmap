//! Normalize Finnish KVL line centroids and preserve missing heavy-vehicle measurements.

use super::{Source, hpms, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    files: Vec<PathBuf>,
    search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    root: &Path,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    for feature in records::pages(root, &recipe.files)? {
        let p = &feature["properties"];
        let Some(total) = records::number(&p["kvl"])?.filter(|v| *v > 0.0) else {
            continue;
        };
        let Some(road) = records::number(&p["alkusijainti_tie"])?.filter(|v| *v > 0.0) else {
            continue;
        };
        let rank = if road < 100.0 || road == 101.0 || road == 102.0 {
            0
        } else if road < 1000.0 {
            2
        } else {
            4
        };
        let heavy = records::number(&p["kvl_raskas"])?;
        if heavy.is_some_and(|v| v > total) {
            continue;
        }
        let mut row = recipe.source.observation(
            inputs,
            hpms::identity(&feature),
            CountBasis::BothDirections,
        )?;
        row.geometry = vec![vec![records::centroid(&feature)?]];
        row.road_class_mask = records::class_mask(Some(rank), false);
        row.search_radius_m = recipe.search_radius_m;
        records::observe_heavy_group(&mut row, heavy);
        row.total = Some(total);
        row.estimated_classes = 15;
        rows.push(row);
    }
    Ok(rows)
}
