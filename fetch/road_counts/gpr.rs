//! Normalize GDDKiA measured sections and explicitly derived provincial reference medians.

use super::{Source, records};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{Identity, IdentityField},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    search_radius_m: f64,
}

fn keys(reference: &str) -> Result<BTreeSet<String>> {
    let lettered = regex::Regex::new(r"^([A-Z]*[0-9]+)[A-Z]$")?;
    let mut keys = BTreeSet::new();
    for key in reference
        .split(';')
        .map(|v| v.split_whitespace().collect::<String>().to_uppercase())
        .filter(|v| !v.is_empty())
    {
        if let Some(captures) = lettered.captures(&key) {
            keys.insert(captures[1].into());
        }
        keys.insert(key);
    }
    Ok(keys)
}

fn target(key: &str, national: bool) -> Identity {
    let alternative = key
        .strip_prefix("DK")
        .map(|v| format!("(?:DK)?{}", regex::escape(v)))
        .or_else(|| {
            key.strip_prefix("DW")
                .map(|v| format!("(?:DW)?{}", regex::escape(v)))
        })
        .unwrap_or_else(|| regex::escape(key));
    Identity {
        field: IdentityField::Reference,
        pattern: format!("(?:^|[;,]){alternative}(?:[;,]|$)"),
        road_class_mask: 31,
        priority: u16::from(national),
        prefer_first_match: !national,
    }
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    let mut provincial = BTreeMap::<String, Vec<Observation>>::new();
    for value in raw
        .as_array()
        .ok_or_else(|| anyhow!("GPR needs a section array"))?
    {
        let mut row = recipe.source.observation(
            inputs,
            records::text(&value["nr2020"]),
            CountBasis::BothDirections,
        )?;
        for (class, field) in ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .iter()
            .enumerate()
        {
            row.vehicle_classes[class] = records::number(&value[field])?;
        }
        if row.vehicle_classes.iter().flatten().sum::<f64>() == 0.0 {
            continue;
        }
        row.total = row
            .vehicle_classes
            .iter()
            .copied()
            .collect::<Option<Vec<_>>>()
            .map(|v| v.iter().sum());
        let reference = value["ref"]
            .as_str()
            .ok_or_else(|| anyhow!("missing GPR reference"))?;
        let national = !value["isProvincial"]
            .as_bool()
            .ok_or_else(|| anyhow!("missing GPR scope"))?;
        row.road_class_mask = 31;
        if national {
            row.geometry = vec![serde_json::from_value(value["coords"].clone())?];
            row.search_radius_m = recipe.search_radius_m;
            row.target.identities = keys(reference)?
                .iter()
                .map(|key| target(key, true))
                .collect();
            rows.push(row);
        } else {
            row.measured = false;
            row.estimated_classes = 15;
            for key in keys(reference)? {
                provincial.entry(key).or_default().push(row.clone());
            }
        }
    }
    for (key, mut group) in provincial {
        group.sort_by(|a, b| {
            a.total
                .unwrap_or(0.0)
                .total_cmp(&b.total.unwrap_or(0.0))
                .then(a.identity.cmp(&b.identity))
        });
        let mut median = group[(group.len() - 1) / 2].clone();
        median.identity = format!("{}/median/{key}", median.identity);
        median.target.identities = vec![target(&key, false)];
        rows.push(median);
    }
    Ok(rows)
}
