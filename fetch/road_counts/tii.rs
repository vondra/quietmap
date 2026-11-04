//! Preserve the observed day and seven vehicle classes of TII station aggregates.

use super::{Source, records};
use crate::{
    roads::observation::{CountBasis, Observation, TimeBasis},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub sites_input: String,
    pub weekday_to_model_daily: f64,
    pub search_radius_m: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let sites: Value = serde_json::from_slice(&std::fs::read(
        inputs
            .by_id(&recipe.sites_input)
            .map_err(anyhow::Error::msg)?,
    )?)?;
    let syntax = regex::Regex::new(r"(?i)(?:^|\s)([MNRL])[- ]*0*(\d+)\b")?;
    let sites = sites
        .as_array()
        .ok_or_else(|| anyhow!("TII station array missing"))?
        .iter()
        .filter_map(|site| {
            let name = records::text(&site["name"]);
            let reference = syntax.captures(&name)?;
            Some((
                records::text(&site["cosit"]),
                (
                    format!("{}{}", reference[1].to_uppercase(), &reference[2]),
                    [
                        site["location"]["lng"].as_f64()?,
                        site["location"]["lat"].as_f64()?,
                    ],
                ),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut counts = BTreeMap::<String, [Option<f64>; 7]>::new();
    for values in records::csv(path, b',')? {
        let class =
            records::number(&values["class"])?.ok_or_else(|| anyhow!("TII class missing"))?;
        let count = records::number(&values["VehicleCount"])?
            .ok_or_else(|| anyhow!("TII count missing"))?;
        ensure!(
            class.fract() == 0.0 && count.fract() == 0.0,
            "nonintegral TII count or class"
        );
        if !(1.0..=7.0).contains(&class) {
            continue;
        }
        let classes = counts
            .entry(records::text(&values["cosit"]))
            .or_insert([None; 7]);
        let class = &mut classes[class as usize - 1];
        *class = Some(class.unwrap_or(0.0) + count);
    }
    let mut rows = Vec::new();
    for (identity, classes) in counts {
        let Some((reference, point)) = sites.get(&identity) else {
            continue;
        };
        // An omitted class is not a measured zero; without a published total it cannot be completed.
        if classes.iter().any(Option::is_none) {
            continue;
        }
        let classes = classes.map(Option::unwrap);
        let total: f64 = classes.iter().sum();
        if total < 100.0 {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, identity, CountBasis::BothDirections)?;
        row.time_basis = TimeBasis::WeekdayDaily;
        row.daily_factor = recipe.weekday_to_model_daily;
        row.reference = reference.clone();
        row.reference_required = true;
        row.geometry = vec![vec![*point]];
        row.road_class_mask = 31;
        row.search_radius_m = recipe.search_radius_m;
        row.target.distance_rule = crate::roads::observation_target::DistanceRule::GreatCircle;
        row.total = Some(total);
        row.vehicle_classes = [
            Some(classes[1] + classes[2]),
            Some(classes[3]),
            Some(classes[4] + classes[5] + classes[6]),
            Some(classes[0]),
        ];
        rows.push(row);
    }
    Ok(rows)
}
