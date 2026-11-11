//! Each count publisher's sampling design, declared from its documentation, set on its observations.

use crate::roads::observation::Observation;
use crate::roads::observation_sample::SamplingDesign;
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

/// How the publishers of `inputs` chose where to count, with the document that says so
/// (`stated` false where no reachable document states it and `statement` gives the assumption).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Declaration {
    inputs: Vec<String>,
    design: SamplingDesign,
    /// Observations of these publisher road categories (their class masks) follow another design:
    /// Czechia's census counts every motorway, class I and II road but only some class III roads.
    #[serde(default)]
    exceptions: Vec<Exception>,
    citation: String,
    statement: String,
    stated: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exception {
    road_class_mask: u16,
    design: SamplingDesign,
}

/// Every observation takes the design its input declares; an input without one is refused.
pub(super) fn apply(declarations: &[Declaration], rows: &mut [Observation]) -> Result<()> {
    let mut by_input = BTreeMap::new();
    for declaration in declarations {
        ensure!(
            declaration.design != SamplingDesign::Undeclared
                && declaration
                    .exceptions
                    .iter()
                    .all(|e| e.design != SamplingDesign::Undeclared)
                && declaration.citation.starts_with("https://")
                && !declaration.statement.is_empty(),
            "a sampling declaration needs a design, a cited document and its statement"
        );
        for input in &declaration.inputs {
            ensure!(
                by_input.insert(input.as_str(), declaration).is_none(),
                "{input}: two sampling declarations"
            );
        }
    }
    let assumed: Vec<&str> = declarations
        .iter()
        .filter(|d| !d.stated)
        .flat_map(|d| d.inputs.iter().map(String::as_str))
        .collect();
    if !assumed.is_empty() {
        eprintln!("road-counts: sampling design assumed, not stated, for {assumed:?}");
    }
    for row in rows {
        let declaration = by_input
            .get(row.input_id.as_str())
            .ok_or_else(|| anyhow!("{}: no declared sampling design", row.input_id))?;
        row.sample.design = declaration
            .exceptions
            .iter()
            .find(|e| e.road_class_mask == row.road_class_mask)
            .map_or(declaration.design, |e| e.design);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roads::observation::{CountBasis, TimeBasis};

    fn row(input_id: &str, road_class_mask: u16) -> Observation {
        Observation {
            identity: "1".into(),
            input_id: input_id.into(),
            source_id: 20,
            source_rank: 1,
            measured: true,
            country: u16::from_le_bytes(*b"CZ"),
            observed_start: "2020-01-01".into(),
            observed_end: "2020-12-31".into(),
            count_basis: CountBasis::BothDirections,
            time_basis: TimeBasis::AnnualDaily,
            direction: 0,
            reference: String::new(),
            geometry: Vec::new(),
            road_class_mask,
            search_radius_m: 0.0,
            reference_required: false,
            vehicle_groups: Vec::new(),
            vehicle_classes: [None; 4],
            total: Some(1_000.0),
            estimated_classes: 15,
            daily_factor: 1.0,
            target: Default::default(),
            sample: Default::default(),
        }
    }

    fn declarations(json: &str) -> Vec<Declaration> {
        serde_json::from_str(json).unwrap()
    }

    /// Czechia's census counts every class II road and some class III roads; a count of a
    /// publisher nobody declared, a second declaration and one without its document are refused.
    #[test]
    fn every_count_takes_its_publishers_declared_design() {
        let declared = declarations(
            r#"[{"inputs":["census"],"design":"whole-network",
                "exceptions":[{"road_class_mask":24,"design":"selected-sites"}],
                "citation":"https://example.org/census","statement":"some class III roads","stated":true},
              {"inputs":["city"],"design":"selected-sites","citation":"https://example.org/city",
                "statement":"assumed: a city's counted streets","stated":false}]"#,
        );
        let mut rows = vec![row("census", 28), row("census", 24), row("city", 4)];
        apply(&declared, &mut rows).unwrap();
        let designs: Vec<SamplingDesign> = rows.iter().map(|r| r.sample.design).collect();
        assert_eq!(
            designs,
            [
                SamplingDesign::WholeNetwork,
                SamplingDesign::SelectedSites,
                SamplingDesign::SelectedSites
            ]
        );
        assert!(apply(&declared, &mut [row("survey", 4)]).is_err());
        let twice = declarations(
            r#"[{"inputs":["census"],"design":"whole-network","citation":"https://a.org","statement":"s","stated":true},
              {"inputs":["census"],"design":"random-sample","citation":"https://b.org","statement":"s","stated":true}]"#,
        );
        assert!(apply(&twice, &mut [row("census", 4)]).is_err());
        let uncited = declarations(
            r#"[{"inputs":["census"],"design":"whole-network","citation":"","statement":"s","stated":true}]"#,
        );
        assert!(apply(&uncited, &mut [row("census", 4)]).is_err());
    }

    /// Both years' committed recipes parse, and every converter's publisher declares its design.
    #[test]
    fn every_committed_count_publisher_declares_its_sampling_design() {
        for year in ["2024", "2026"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../years/tables/{year}/road-conversion.json"));
            let recipe: super::super::Recipe =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            apply(&recipe.sampling, &mut []).unwrap();
            for object in &recipe.objects {
                let input = object.input_id();
                assert!(
                    recipe.sampling.iter().any(|d| d.inputs.contains(input)),
                    "{year}: {input}"
                );
            }
        }
    }
}
