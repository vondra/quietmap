//! Classify requested squares with neighbouring context and bind only completed outputs to their inputs.

use super::{matching, model::Model, osm_input, records};
use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub input_sha256: String,
    pub year: String,
    pub squares: Vec<[u32; 2]>,
}

pub fn build(inputs: &YearInputs, squares: &[Square], out: &Path) -> Result<()> {
    ensure!(!out.exists(), "facility output already exists");
    matching::validate_selection(squares)?;
    let hash = super::fingerprint::classified_inputs(inputs)?;
    let mut squares = squares.to_vec();
    squares.sort_unstable();
    squares.dedup();
    let model = Model::read(
        inputs
            .required(Kind::FacilityModel)
            .map_err(anyhow::Error::msg)?,
    )?;
    let observations = records::read(
        inputs
            .required(Kind::FacilityObservations)
            .map_err(anyhow::Error::msg)?,
        &inputs.manifest.year,
        &super::fingerprint::observation_inputs(inputs)?,
        None,
    )?;
    let context: BTreeSet<_> = squares
        .iter()
        .flat_map(|square| square.with_neighbours())
        .collect();
    let mapped = osm_input::read(inputs, &context.into_iter().collect::<Vec<_>>())?;
    let (owners, mut rows): (Vec<_>, Vec<_>) = mapped.into_iter().unzip();
    matching::classify(&mut rows, &observations, &model, &inputs.manifest.as_of)?;
    let mut by_square: BTreeMap<_, Vec<_>> = squares.iter().map(|s| (*s, Vec::new())).collect();
    for (square, row) in owners.into_iter().zip(rows) {
        if let Some(output) = by_square.get_mut(&square) {
            output.push(row);
        }
    }
    for (square, rows) in by_square {
        records::write(
            &out.join("z9")
                .join(square.x.to_string())
                .join(square.y.to_string())
                .join("facilities.arrow"),
            &rows,
            &inputs.manifest.year,
            &hash,
        )?;
        eprintln!("facilities: {}:{}: {} rows", square.x, square.y, rows.len());
    }
    let receipt = Receipt {
        input_sha256: hash,
        year: inputs.manifest.year.clone(),
        squares: squares.iter().map(|s| [s.x, s.y]).collect(),
    };
    std::fs::write(out.join("complete.json"), serde_json::to_vec(&receipt)?)?;
    Ok(())
}
