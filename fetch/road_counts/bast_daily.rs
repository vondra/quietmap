//! Convert BASt permanent-counter annual CSVs, preserving absent vehicle percentages and their daily basis.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{DistanceRule, ReferenceRule},
    },
    year::YearInputs,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let bytes = std::fs::read(path)?;
    let text = encoding_rs::WINDOWS_1252.decode(&bytes).0;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader.headers()?.clone();
    ensure!(
        ["DZ_Nr", "DTV_Kfz_MobisSo_Q", "Koor_WGS84_E", "Koor_WGS84_N"]
            .iter()
            .all(|name| headers.iter().any(|v| v == *name)),
        "BASt annual columns are missing"
    );
    let mut rows = Vec::new();
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
        let number = |name: &str| -> Option<f64> {
            value(name)
                .replace('.', "")
                .replace(',', ".")
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
        };
        let (Some(total), Some(lon), Some(lat)) = (
            number("DTV_Kfz_MobisSo_Q"),
            number("Koor_WGS84_E"),
            number("Koor_WGS84_N"),
        ) else {
            continue;
        };
        if total == 0.0 || value("DZ_Nr").is_empty() {
            continue;
        }
        let mut row = recipe.source.observation(
            inputs,
            format!("{}:{}", value("TK_Nr"), value("DZ_Nr")),
            CountBasis::BothDirections,
        )?;
        row.reference = format!("{}{}{}", value("Str_Kl"), value("Str_Nr"), value("Str_Zus"));
        row.reference_required = true;
        row.geometry = vec![vec![[lon, lat]]];
        row.total = Some(total);
        // BASt's TLS groups overlap: PLZ already includes vans and motorcycles, Lzg includes Sat.
        // See BASt Bestandsbandformat Verkehrsmengen, section 2.2.3.2 and its classification table.
        let motorcycle = number("pMot_MobisSo_Q");
        let medium = number("pLoA_MobisSo_Q")
            .zip(number("pBus_MobisSo_Q"))
            .map(|(a, b)| a + b);
        let heavy = number("pLzg_MobisSo_Q");
        let light = number("pPLZ_MobisSo_Q")
            .zip(number("pPmA_MobisSo_Q"))
            .zip(motorcycle)
            .filter(|_| number("pSon_MobisSo_Q") == Some(0.0))
            .map(|((plz, trailer), moto)| plz + trailer - moto)
            .filter(|v| *v >= 0.0);
        row.vehicle_classes = [light, medium, heavy, motorcycle]
            .map(|share| share.map(|v| (total * v / 100.0).round()));
        if row.vehicle_classes.iter().flatten().sum::<f64>() > total + 2.0 {
            continue;
        }
        row.road_class_mask = 31;
        row.search_radius_m = recipe.search_radius_m;
        row.target.distance_rule = DistanceRule::GreatCircle;
        row.target.reference_rule = ReferenceRule::Whole;
        row.sample.road_class = match value("Str_Kl") {
            "A" => Some(0),
            "B" => Some(2),
            "L" => Some(3),
            "K" => Some(4),
            _ => None,
        };
        rows.push(row);
    }
    ensure!(
        !rows.is_empty(),
        "BASt daily CSV contains no usable annual counts"
    );
    Ok(rows)
}
