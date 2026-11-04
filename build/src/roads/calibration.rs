//! Fit road model inputs from manifest-admitted observations, retaining sample counts beside each result.

mod periods;
mod preparation;
mod priors;

use super::{observation::Observation, observation_io};
use crate::year::{Kind, YearInputs};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    minimum_class_samples: usize,
    minimum_hourly_days: u32,
    minimum_hourly_stations: usize,
    period_by_hour: [usize; 24],
    #[serde(default)]
    preparation: Option<Value>,
}

pub fn build(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "calibration output must be a new directory"
    );
    let path = inputs
        .required(Kind::RoadCalibration)
        .map_err(anyhow::Error::msg)?;
    let parameters: Parameters = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        parameters.minimum_class_samples > 0
            && parameters.minimum_hourly_days > 0
            && parameters.minimum_hourly_stations > 0
            && parameters.period_by_hour.iter().all(|p| *p < 3)
            && (0..3).all(|p| parameters.period_by_hour.contains(&p)),
        "invalid road calibration support"
    );
    let recipe = inputs
        .manifest
        .inputs
        .iter()
        .find(|r| r.kind == Kind::RoadCalibration)
        .unwrap();
    let mut rows = Vec::<Observation>::new();
    for input in inputs
        .manifest
        .inputs
        .iter()
        .filter(|r| r.kind == Kind::RoadObservations)
    {
        ensure!(
            recipe.depends_on.contains(&input.id),
            "calibration omits observation dependency {}",
            input.id
        );
        rows.extend(observation_io::read(
            &inputs.by_id(&input.id).map_err(anyhow::Error::msg)?,
            inputs,
        )?);
    }
    observation_io::validate(&rows, inputs)?;
    std::fs::create_dir(output)?;
    let priors = priors::fit(&rows, &parameters, output)?;
    let periods = periods::fit(&rows, &parameters, output)?;
    if let Some(template) = &parameters.preparation {
        preparation::write_model(
            template,
            inputs,
            output,
            &priors.section_classes,
            periods
                .weekday_factor
                .ok_or_else(|| anyhow::anyhow!("no measured weekday conversion"))?,
        )?;
    }
    write(
        output,
        "samples",
        &json!({"class_models":priors.sample_counts,"period_models":periods.sample_counts}),
    )
}

fn write(directory: &Path, name: &str, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    std::fs::write(directory.join(format!("{name}.json")), bytes)?;
    Ok(())
}

fn weighted_median(values: &mut [(f64, f64)]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let middle = values.iter().map(|v| v.1).sum::<f64>() / 2.0;
    let mut cumulative = 0.0;
    for &(value, weight) in values.iter() {
        cumulative += weight;
        if cumulative >= middle {
            return Some(value);
        }
    }
    None
}

fn country_row(country: u16, value: Value) -> Value {
    if country == 0 {
        json!({"countries":[],"value":value})
    } else {
        json!({"country":String::from_utf8(country.to_le_bytes().to_vec()).unwrap(),"value":value})
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn splitting_a_measured_section_does_not_change_its_weight() {
        let whole = super::weighted_median(&mut [(100.0, 10.0), (300.0, 20.0)]);
        let split = super::weighted_median(&mut [(100.0, 10.0), (300.0, 10.0), (300.0, 10.0)]);
        assert_eq!(whole, Some(300.0));
        assert_eq!(whole, split);
    }
}
