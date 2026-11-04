//! Normalize Mastra station-years, retaining directional road parts and the latest admitted survey per site.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    files: Vec<PathBuf>,
    coordinate_epsg: u32,
    search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    root: &Path,
) -> Result<Vec<Observation>> {
    let transform = records::projection(recipe.coordinate_epsg)?;
    let mut rows = BTreeMap::<String, Observation>::new();
    for feature in records::pages_latin1(root, &recipe.files)? {
        let p = &feature["properties"];
        if !p["KOERETOEJSART"].is_null() && p["KOERETOEJSART"] != "MOTORKTJ" {
            continue;
        }
        let Some(total) = published_count(&p["AADT"])?.filter(|v| *v > 0.0) else {
            continue;
        };
        let Some(year) = records::number(&p["AAR"])? else {
            continue;
        };
        let Some(road) = records::number(&p["VEJNR"])?.filter(|v| *v > 0.0) else {
            continue;
        };
        let Some(km) = records::number(&p["KILOMETER"])? else {
            continue;
        };
        let part = records::text(&p["VEJDEL"]);
        let state = records::text(&p["VEJBESTYRER"]).trim() == "0";
        let number = if state {
            part.parse::<u8>().unwrap_or(0)
        } else {
            0
        };
        let identity = format!("{road}/{part}/{km}");
        let mut row = recipe.source.observation(
            inputs,
            identity.clone(),
            if [1, 2].contains(&number) {
                CountBasis::Directional
            } else {
                CountBasis::BothDirections
            },
        )?;
        if !records::survey_year(&mut row, &recipe.source, inputs, year as i32)? {
            continue;
        }
        let coordinates = feature["geometry"]["coordinates"]
            .as_array()
            .ok_or_else(|| anyhow!("missing Mastra location"))?;
        let point = [
            coordinates[0]
                .as_f64()
                .ok_or_else(|| anyhow!("invalid Mastra easting"))?,
            coordinates[1]
                .as_f64()
                .ok_or_else(|| anyhow!("invalid Mastra northing"))?,
        ];
        row.geometry = vec![vec![records::project(&transform, point)?]];
        row.road_class_mask = records::class_mask(Some(if state { 1 } else { 4 }), number >= 3);
        row.search_radius_m = recipe.search_radius_m;
        row.total = Some(total.round());
        records::observe_heavy_group(&mut row, published_count(&p["LBIL_AADT"])?);
        row.estimated_classes = 15;
        if row.count_basis == CountBasis::Directional {
            row.target.way_assignment = Some(crate::roads::observation_target::WayAssignment {
                maximum_distance_m: recipe.search_radius_m,
                clearance_m: recipe.search_radius_m,
                require_class_clearance: false,
                extend_along_way: false,
            });
        }
        if rows
            .get(&identity)
            .is_none_or(|previous| previous.observed_end < row.observed_end)
        {
            rows.insert(identity, row);
        }
    }
    Ok(rows.into_values().collect())
}

fn published_count(value: &serde_json::Value) -> Result<Option<f64>> {
    // Mastra retains invalid negative traffic estimates; they are neither counts nor measured zero.
    if value.as_f64().is_some_and(|n| n < 0.0) {
        Ok(None)
    } else {
        records::number(value)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_estimate_does_not_become_a_measured_zero() {
        assert_eq!(
            super::published_count(&serde_json::json!(-1)).unwrap(),
            None
        );
        assert_eq!(
            super::published_count(&serde_json::json!(0)).unwrap(),
            Some(0.0)
        );
        assert_eq!(
            super::published_count(&serde_json::json!(120)).unwrap(),
            Some(120.0)
        );
    }
}
