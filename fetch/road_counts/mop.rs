//! Admit separately identified Chilean census legs without transferring the busiest leg to other routes.

use super::{
    Source,
    fields::{GeometryFormat, geometry},
    records,
};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::ReferenceRule,
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub arcgis: bool,
    pub year_field: String,
    pub search_radius_m: f64,
}

fn unambiguous_legs(properties: &Value) -> Result<BTreeMap<String, f64>> {
    let mut legs = BTreeMap::<String, Vec<f64>>::new();
    for i in 1..=4 {
        let reference = records::text(&properties[format!("ROL_RAMA_{i}")]);
        let Some(count) =
            records::number(&properties[format!("TMDA_RAMA_{i}")])?.filter(|n| *n > 0.0)
        else {
            continue;
        };
        if !reference.trim().is_empty() && reference != "-" {
            legs.entry(reference).or_default().push(count);
        }
    }
    Ok(legs
        .into_iter()
        .filter_map(|(reference, counts)| {
            counts
                .iter()
                .all(|n| *n == counts[0])
                .then_some((reference, counts[0]))
        })
        .collect())
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    for feature in raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("MOP features missing"))?
    {
        let p = &feature[if recipe.arcgis {
            "attributes"
        } else {
            "properties"
        }];
        let Some(year) = records::number(&p[&recipe.year_field])? else {
            continue;
        };
        for (reference, total) in unambiguous_legs(p)? {
            let mut row = recipe.source.observation(
                inputs,
                format!("{}:{reference}", records::text(&p["OBJECTID"])),
                CountBasis::BothDirections,
            )?;
            if !records::survey_year(&mut row, &recipe.source, inputs, year as i32)? {
                continue;
            }
            row.geometry = geometry(
                feature,
                p,
                &if recipe.arcgis {
                    GeometryFormat::Arcgis
                } else {
                    GeometryFormat::Geojson
                },
            )?;
            row.reference = reference;
            row.reference_required = true;
            row.total = Some(total);
            row.road_class_mask = 31;
            row.search_radius_m = recipe.search_radius_m;
            row.target.reference_rule = ReferenceRule::Whole;
            rows.push(row);
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    #[test]
    fn two_unequal_legs_of_one_route_cannot_be_identified_by_the_station_point() {
        let rows=super::unambiguous_legs(&serde_json::json!({"ROL_RAMA_1":"J-40","TMDA_RAMA_1":7582,"ROL_RAMA_2":"J-40","TMDA_RAMA_2":5235,"ROL_RAMA_3":"J-300-I","TMDA_RAMA_3":4142})).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["J-300-I"], 4142.0);
    }
}
