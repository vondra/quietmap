//! Normalize ASTRA annual station blocks, retaining unavailable vehicle classes separately from measured zero.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub sites_input: String,
    count_sheet: String,
    site_sheet: String,
    motorcycle_share: f64,
    search_radius_m: f64,
}

fn cell(row: &[Option<String>], index: usize) -> &str {
    row.get(index)
        .and_then(Option::as_deref)
        .unwrap_or("")
        .trim()
}
fn key(value: &str) -> Option<String> {
    value.parse::<u32>().ok().map(|v| v.to_string())
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let transform = records::projection(2056)?;
    let mut sites = HashMap::new();
    for p in records::sheet(
        &inputs
            .by_id(&recipe.sites_input)
            .map_err(anyhow::Error::msg)?,
        &recipe.site_sheet,
    )? {
        let Some(site) = key(cell(&p, 0)) else {
            continue;
        };
        let (Ok(east), Ok(north)) = (cell(&p, 6).parse::<f64>(), cell(&p, 7).parse::<f64>()) else {
            continue;
        };
        sites.insert(site, records::project(&transform, [east, north])?);
    }
    let raw = records::sheet(path, &recipe.count_sheet)?;
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0;
    while index < raw.len() {
        let head = &raw[index];
        if cell(head, 5) != "DTV" {
            index += 1;
            continue;
        }
        let site = key(cell(head, 0)).ok_or_else(|| anyhow!("missing ASTRA station identity"))?;
        ensure!(seen.insert(site.clone()), "duplicate ASTRA station {site}");
        let block = raw
            .get(index..index + 6)
            .ok_or_else(|| anyhow!("truncated ASTRA block"))?;
        ensure!(
            block
                .iter()
                .zip(["DTV", "DTV SV", "DTV SGF", "DWV", "DWV SV", "DWV SGF"])
                .all(|(row, label)| cell(row, 5) == label),
            "invalid ASTRA station block"
        );
        index += 6;
        let number = |i: usize| {
            cell(&block[i], 18)
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        let Some(total) = number(0).filter(|v| *v > 0.0) else {
            continue;
        };
        let Some(point) = sites.get(&site) else {
            continue;
        };
        let reference = cell(head, 4).to_uppercase();
        let rank = if reference.starts_with('A') {
            0
        } else if reference.starts_with('H') {
            1
        } else {
            continue;
        };
        let (large, heavy) = (number(1), number(2));
        let valid = large
            .zip(heavy)
            .filter(|(large, heavy)| *large <= total && *heavy <= *large);
        let mut row = recipe
            .source
            .observation(inputs, site, CountBasis::BothDirections)?;
        row.geometry = vec![vec![*point]];
        row.road_class_mask = records::class_mask(Some(rank), false);
        row.search_radius_m = recipe.search_radius_m;
        row.total = Some(total.round());
        if let Some((large, heavy)) = valid {
            let medium = (large - heavy).round();
            let heavy = heavy.round();
            let moto = (total * recipe.motorcycle_share).round();
            let light = total.round() - medium - heavy - moto;
            if light < 0.0 {
                continue;
            }
            row.vehicle_classes = [Some(light), Some(medium), Some(heavy), Some(moto)];
            row.estimated_classes = 1 | 8;
        } else {
            row.estimated_classes = 15;
        }
        rows.push(row);
    }
    Ok(rows)
}
