//! Admit the manifest-pinned ERA5 counts and means, with their completed climatology receipt.

#[path = "numpy.rs"]
pub mod numpy;

use crate::year::{Kind, YearInputs};
use physics::bands::{BANDS, PERIODS};
use physics::weather::SECTORS;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek};

pub const COLUMNS: usize = 1440;
pub const ROWS: usize = 721;
pub const DENSITY: usize = 4;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatisticsObject {
    sha256: String,
    sampling_hours: usize,
    counts: String,
    favourable: String,
    means: String,
    receipt: String,
}

pub struct Statistics {
    pub counts: numpy::Array,
    pub favourable: numpy::Array,
    pub means: numpy::Array,
}

impl Statistics {
    pub fn read(inputs: &YearInputs) -> Result<Self, String> {
        let path = inputs.required(Kind::PrepareWeatherStatistics)?;
        let input = inputs
            .manifest
            .inputs
            .iter()
            .find(|input| {
                input.kind == Kind::PrepareWeatherStatistics
                    && inputs.manifest.admitted(&input.id).unwrap_or(false)
            })
            .ok_or("missing admitted weather statistics")?;
        let object: StatisticsObject = serde_json::from_value(
            input
                .conversion
                .clone()
                .ok_or("weather statistics need decoding parameters")?,
        )
        .map_err(|e| e.to_string())?;
        let (start, last) = input.observed.bounds()?;
        let as_of = chrono::NaiveDate::parse_from_str(&inputs.manifest.as_of, "%Y-%m-%d")
            .map_err(|e| e.to_string())?;
        if last >= as_of || object.sampling_hours == 0 || 24 % object.sampling_hours != 0 {
            return Err("invalid or future weather climatology".into());
        }
        let steps = ((last - start).num_days() + 1) as usize * 24 / object.sampling_hours;
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        loop {
            let length = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        if object.sha256 != format!("{:x}", digest.finalize()) {
            return Err("weather statistics checksum differs from the manifest".into());
        }
        file.rewind().map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let receipt: serde_json::Value = serde_json::from_reader(
            archive
                .by_name(&object.receipt)
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if receipt["next_step"].as_u64() != Some(steps as u64) {
            return Err("ERA5 statistics are incomplete for the declared climatology".into());
        }
        let contract = &receipt["identity"]["contract"];
        if contract["climatology_years"].as_str()
            != Some(&format!("{}-{}", start.format("%Y"), last.format("%Y")))
            || contract["sampling_hours"].as_str() != Some(&object.sampling_hours.to_string())
        {
            return Err("ERA5 receipt does not identify the declared climatology".into());
        }
        let statistics = Self {
            counts: numpy::Array::read(&mut archive, &object.counts)?,
            favourable: numpy::Array::read(&mut archive, &object.favourable)?,
            means: numpy::Array::read(&mut archive, &object.means)?,
        };
        let nodes = *statistics
            .counts
            .shape
            .first()
            .ok_or("ERA5 counts have no nodes")?;
        statistics.counts.require("<u4", &[nodes, PERIODS])?;
        statistics
            .favourable
            .require("<u4", &[nodes, PERIODS, SECTORS])?;
        statistics.means.require("<f8", &[nodes, PERIODS, BANDS])?;
        for node in 0..nodes {
            let sum: f64 = (0..PERIODS)
                .map(|period| statistics.counts.value(node * PERIODS + period))
                .sum();
            if sum != steps as f64 {
                return Err(format!("ERA5 node {node}: {sum} states, expected {steps}"));
            }
        }
        Ok(statistics)
    }
}

#[cfg(test)]
#[path = "era5_tests.rs"]
mod tests;
