//! Facility receipts cover their dependency declarations, model bytes and normalized observations.

use crate::year::{Kind, YearInputs};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read};

pub fn observation_inputs(inputs: &YearInputs) -> Result<String> {
    let mut selected = BTreeSet::new();
    let mut pending: Vec<_> = inputs
        .manifest
        .inputs
        .iter()
        .filter(|i| {
            matches!(
                i.kind,
                Kind::FacilityModel | Kind::FacilityObservations | Kind::OsmGeography
            )
        })
        .map(|i| i.id.as_str())
        .collect();
    while let Some(id) = pending.pop() {
        if selected.insert(id) {
            let input = inputs
                .manifest
                .inputs
                .iter()
                .find(|i| i.id == id)
                .ok_or_else(|| anyhow!("missing facility dependency {id}"))?;
            pending.extend(input.depends_on.iter().map(String::as_str));
        }
    }
    let manifest: Value = serde_json::from_slice(&std::fs::read(&inputs.manifest_path)?)?;
    let mut declarations: Vec<_> = manifest["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["id"].as_str().is_some_and(|id| selected.contains(id)))
        .collect();
    declarations.sort_by_key(|i| i["id"].as_str().unwrap());
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(
        &json!({"year":inputs.manifest.year,"as_of":inputs.manifest.as_of,"inputs":declarations}),
    )?);
    hash.update(std::fs::read(
        inputs
            .required(Kind::FacilityModel)
            .map_err(anyhow::Error::msg)?,
    )?);
    Ok(format!("{:x}", hash.finalize()))
}

pub fn classified_inputs(inputs: &YearInputs) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(observation_inputs(inputs)?);
    let mut file = std::fs::File::open(
        inputs
            .required(Kind::FacilityObservations)
            .map_err(anyhow::Error::msg)?,
    )?;
    let mut buffer = [0; 65_536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
