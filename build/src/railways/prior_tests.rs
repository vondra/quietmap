//! Compare the data transcription against independent evaluations of the frozen source classifiers.
use super::{
    model::RailModel,
    prior_rules::{Observation, classify},
};
use serde::Deserialize;
use std::{collections::BTreeMap, io::BufRead};

#[derive(Deserialize)]
struct Case {
    source: u16,
    point: [f64; 2],
    rail_type: u8,
    usage: u8,
    name: String,
    attributes: BTreeMap<String, String>,
    expected: Option<[f64; 2]>,
}

#[test]
#[ignore = "requires an explicit reference classifier receipt"]
fn frozen_corridor_classifier_cases() {
    let plan: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("QM_RAIL_PRIOR_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let model = RailModel::read(std::path::Path::new(plan["model"].as_str().unwrap())).unwrap();
    let file = std::fs::File::open(plan["cases"].as_str().unwrap()).unwrap();
    let mut total = 0;
    for line in std::io::BufReader::new(file).lines() {
        let case: Case = serde_json::from_str(&line.unwrap()).unwrap();
        let row = Observation {
            point: case.point,
            rail_type: case.rail_type,
            usage: case.usage,
            name: &case.name,
            attributes: &case.attributes,
        };
        assert_eq!(
            classify(&model.spatial_priors[&case.source], &row),
            case.expected,
            "source {}, point {:?}, type {}, usage {}, attributes {:?}",
            case.source,
            case.point,
            case.rail_type,
            case.usage,
            case.attributes
        );
        total += 1;
    }
    eprintln!("railways: {total} exact independent classifier cases");
}
