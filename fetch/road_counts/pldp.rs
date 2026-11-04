//! Normalize DRSI section counts and their independently declared counting-site coordinates.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub sites_input: String,
    search_radius_m: f64,
}

fn count(value: &Value) -> Option<f64> {
    value
        .as_str()?
        .trim()
        .replace(['.', ' '], "")
        .parse::<u64>()
        .ok()
        .map(|v| v as f64)
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let transform = records::projection(3794)?;
    let mut sites = HashMap::new();
    for p in records::csv(
        &inputs
            .by_id(&recipe.sites_input)
            .map_err(anyhow::Error::msg)?,
        b';',
    )? {
        let site = records::text(&p["Števno mesto"]).trim().to_owned();
        if site.is_empty() || !site.bytes().all(|v| v.is_ascii_digit()) {
            continue;
        }
        let coordinate = |key| p[key].as_str().and_then(|s| s.trim().parse::<f64>().ok());
        let (Some(east), Some(north)) = (coordinate("Koordinata E"), coordinate("Koordinata N"))
        else {
            continue;
        };
        sites.insert(site, records::project(&transform, [east, north])?);
    }
    let mut rows = Vec::new();
    for p in records::csv(path, b';')? {
        let Some(total) = count(&p["Vsa vozila (PLDP)"]).filter(|v| *v > 0.0) else {
            continue;
        };
        let kind = p["Tip   štetja"].as_str().unwrap_or("").trim();
        let estimated = match kind {
            "P" | "Prazno" | "" => continue,
            "MWTC1" | "QLTC44" | "QLT44" => 15,
            "QLTC10" | "QLTC8" | "QLTC" | "QLTC*" | "ROČNO" | "CIMATIC D2" => 0,
            _ => return Err(anyhow!("unknown DRSI counter method {kind}")),
        };
        let rank = match p["Kat. ceste"].as_str().unwrap_or("").trim() {
            "AC" => 0,
            "HC" | "G1" => 1,
            "G2" => 2,
            "R1" | "R2" => 3,
            "R3" | "RT" => 4,
            _ => continue,
        };
        let site = records::text(&p["Števno mesto"]).trim().to_owned();
        let Some(point) = sites.get(&site) else {
            continue;
        };
        let classes = [
            "Motorji",
            "Osebna vozila",
            "Avtobusi",
            "Lah. tov.  < 3,5t",
            "Sr. tov.  3,5-7t",
            "Tež. tov. nad 7t",
            "Tov. s prik.",
            "Vlačilci",
        ]
        .map(|field| count(&p[field]));
        let grouped = [
            classes[1].zip(classes[3]).map(|(a, b)| a + b),
            classes[2].zip(classes[4]).map(|(a, b)| a + b),
            classes[5]
                .zip(classes[6])
                .zip(classes[7])
                .map(|((a, b), c)| a + b + c),
            classes[0],
        ];
        ensure!(
            classes.iter().any(Option::is_none) || classes.iter().flatten().sum::<f64>() == total,
            "DRSI class total mismatch at {site}"
        );
        let mut row = recipe
            .source
            .observation(inputs, site, CountBasis::BothDirections)?;
        row.geometry = vec![vec![*point]];
        row.road_class_mask = records::class_mask(Some(rank), false);
        row.search_radius_m = recipe.search_radius_m;
        row.vehicle_classes = grouped;
        row.total = Some(total);
        row.estimated_classes = estimated;
        rows.push(row);
    }
    Ok(rows)
}
