//! DRR route codes carry the publisher's separate vehicle classes without erasing measured zeros.

use super::Source;
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    road_class_mask: u16,
    estimated_classes: u8,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &[u8],
) -> Result<Vec<Observation>> {
    let mut reader = csv::Reader::from_reader(raw);
    let mut rows = BTreeMap::new();
    for record in reader.deserialize::<BTreeMap<String, String>>() {
        let record = record?;
        let number = |name: &str| -> Result<Option<f64>> {
            let text = record
                .get(name)
                .ok_or_else(|| anyhow!("missing DRR field {name}"))?
                .trim();
            if text.is_empty() {
                return Ok(None);
            }
            let number: f64 = text.parse()?;
            ensure!(
                number.is_finite() && number >= 0.0,
                "invalid DRR count {name}"
            );
            Ok(Some(number))
        };
        let Some(total) = number("sum_AADT")? else {
            continue;
        };
        if total == 0.0 {
            continue;
        }
        let code = record
            .get("road_code")
            .ok_or_else(|| anyhow!("missing DRR road code"))?
            .trim();
        if code.is_empty() {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, code.into(), CountBasis::BothDirections)?;
        for (class, fields) in [
            vec!["SV", "SVT"],
            vec!["TB2", "BD", "DRT"],
            vec!["TB3", "T4", "ART3", "ART4", "ART5", "ART6"],
            vec!["MC"],
        ]
        .iter()
        .enumerate()
        {
            let values = fields
                .iter()
                .map(|s| number(s))
                .collect::<Result<Vec<_>>>()?;
            row.vehicle_classes[class] = values
                .iter()
                .all(Option::is_some)
                .then(|| values.into_iter().flatten().sum::<f64>().round());
        }
        row.total = Some(total);
        row.reference = code.into();
        row.reference_required = true;
        row.target.reference_rule = crate::roads::observation_target::ReferenceRule::Whole;
        row.road_class_mask = recipe.road_class_mask;
        row.estimated_classes = recipe.estimated_classes;
        rows.insert(code.to_owned(), row);
    }
    ensure!(!rows.is_empty(), "DRR has no usable count records");
    Ok(rows.into_values().collect())
}
