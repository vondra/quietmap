//! TSK profile rows become length-weighted street cross-sections within their declared municipality.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_target::{Identity, IdentityField},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use gdal::{Dataset, DatasetOptions, GdalOpenFlags, vector::LayerAccess};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    sheet: String,
    street_aliases: BTreeMap<String, String>,
    pub coverage_input: String,
    coverage_property: String,
    coverage_name: String,
    road_class_mask: u16,
    weekday_to_model_daily: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let coverage = super::records::coverage(
        inputs,
        &recipe.coverage_input,
        &recipe.coverage_property,
        &recipe.coverage_name,
    )?;
    let dataset = Dataset::open_ex(
        path,
        DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_VECTOR,
            open_options: Some(&["FIELD_TYPES=STRING", "HEADERS=DISABLE"]),
            ..Default::default()
        },
    )?;
    let mut sheet = dataset.layer_by_name(&recipe.sheet)?;
    let mut streets = BTreeMap::<String, (f64, [f64; 3])>::new();
    for feature in sheet.features() {
        let number = |index| -> Result<Option<f64>> {
            Ok(feature
                .field_as_string(index)?
                .and_then(|v| v.trim().parse::<f64>().ok()))
        };
        let (Some(start), Some(end)) = (number(0)?, number(1)?) else {
            continue;
        };
        ensure!(
            start.is_finite() && end.is_finite(),
            "invalid TSK section identity"
        );
        let street = feature
            .field_as_string(2)?
            .ok_or_else(|| anyhow!("missing TSK street name"))?;
        ensure!(!street.trim().is_empty(), "empty TSK street name");
        let count = |index| -> Result<f64> {
            let value = number(index)?.ok_or_else(|| anyhow!("missing TSK count/length"))?;
            ensure!(
                value.is_finite() && value >= 0.0,
                "invalid TSK count/length"
            );
            Ok(value)
        };
        let length = count(5)?;
        ensure!(length > 0.0, "zero TSK section length");
        let counts = [count(6)?, count(9)?, count(7)?];
        if counts.iter().sum::<f64>() == 0.0 {
            continue;
        }
        let entry = streets.entry(street.trim().into()).or_default();
        entry.0 += length;
        for (sum, count) in entry.1.iter_mut().zip(counts) {
            *sum += count * length;
        }
    }
    let mut rows = Vec::new();
    for (street, (length, counts)) in streets {
        let name = recipe.street_aliases.get(&street).unwrap_or(&street);
        let mut row =
            recipe
                .source
                .observation(inputs, street.clone(), CountBasis::StreetCrossSection)?;
        let counts = counts.map(|v| (v / length).round());
        row.vehicle_classes = [Some(counts[0]), Some(counts[1]), Some(counts[2]), Some(0.0)];
        row.total = Some(counts.iter().sum());
        row.time_basis = TimeBasis::WeekdayDaily;
        row.daily_factor = recipe.weekday_to_model_daily;
        row.estimated_classes = 8;
        row.road_class_mask = recipe.road_class_mask;
        row.target.coverage = coverage.clone();
        row.target.identities = vec![Identity {
            field: IdentityField::Name,
            pattern: format!(
                "^{}$",
                regex::escape(&crate::roads::canonical_road_name(name))
            ),
            road_class_mask: recipe.road_class_mask,
            priority: 0,
            prefer_first_match: false,
        }];
        rows.push(row);
    }
    ensure!(!rows.is_empty(), "TSK workbook has no usable profiles");
    Ok(rows)
}
