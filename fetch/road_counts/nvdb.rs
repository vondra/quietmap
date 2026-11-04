//! Normalize Norwegian NVDB point counts without assigning a later survey to an earlier year.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
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
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    let ramp = regex::Regex::new(r" [KS]D[0-9]+ ")?;
    for values in raw
        .as_array()
        .ok_or_else(|| anyhow!("NVDB needs a record array"))?
    {
        let reference = values["vegref"]
            .as_str()
            .ok_or_else(|| anyhow!("missing NVDB reference"))?;
        let rank = match reference.get(..2) {
            Some("EV" | "RV") => 1,
            Some("FV") => 2,
            _ => continue,
        };
        let Some(total) = records::number(&values["aadt"])?.filter(|n| *n > 0.0) else {
            continue;
        };
        let mut row = recipe.source.observation(
            inputs,
            records::text(&values["id"]),
            CountBasis::BothDirections,
        )?;
        let year = values["year"]
            .as_i64()
            .ok_or_else(|| anyhow!("missing NVDB observation year"))?;
        if !records::survey_year(&mut row, &recipe.source, inputs, i32::try_from(year)?)? {
            continue;
        }
        let (Some(lon), Some(lat)) = (values["midLon"].as_f64(), values["midLat"].as_f64()) else {
            continue;
        };
        let heavy = records::number(&values["heavyPct"])?;
        ensure!(heavy.is_none_or(|v| v <= 100.0), "invalid heavy percentage");
        row.geometry = vec![vec![[lon, lat]]];
        row.road_class_mask = records::class_mask(Some(rank), ramp.is_match(reference));
        row.search_radius_m = recipe.search_radius_m;
        row.total = Some(total);
        records::observe_heavy_group(&mut row, heavy.map(|v| total * v / 100.0));
        row.estimated_classes = 15;
        rows.push(row);
    }
    Ok(rows)
}
