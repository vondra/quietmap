//! Length-weighted road priors and class fractions use observed totals and independently measured classes only.

use super::{Parameters, country_row, weighted_median, write};
use crate::roads::observation::{CountBasis, Observation, TimeBasis};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

type Samples = BTreeMap<(u16, u8, u8, u8), Vec<(f64, f64)>>;

pub struct ClassModel {
    pub sample_counts: Value,
    pub section_classes: [[f64; 3]; 6],
}

pub fn fit(rows: &[Observation], parameters: &Parameters, directory: &Path) -> Result<ClassModel> {
    let mut samples = Samples::new();
    for row in rows.iter().filter(|r| {
        r.measured
            && !r.sample.fit_only
            && r.time_basis == TimeBasis::AnnualDaily
            && r.count_basis == CountBasis::BothDirections
    }) {
        let (Some(class), Some(total), Some(length)) =
            (row.sample.road_class, row.total, row.sample.length_m)
        else {
            continue;
        };
        if total <= 0.0 {
            continue;
        }
        let environments = if row.sample.environment == 0 {
            vec![0]
        } else {
            vec![0, row.sample.environment]
        };
        for country in [0, row.country] {
            for &environment in &environments {
                samples
                    .entry((country, class, environment, 0))
                    .or_default()
                    .push((total, length));
                for vehicle in [1, 2] {
                    if row.estimated_classes & (1 << vehicle) != 0 {
                        continue;
                    }
                    if let Some(count) = row.vehicle_classes[vehicle] {
                        samples
                            .entry((country, class, environment, vehicle as u8))
                            .or_default()
                            .push((100.0 * count / total, length));
                    }
                }
            }
        }
    }
    let countries = samples
        .keys()
        .map(|k| k.0)
        .collect::<std::collections::BTreeSet<_>>();
    let counts = samples
        .iter()
        .map(|(key, values)| {
            (
                format!("{}/{}/{}/{}", key.0, key.1, key.2, key.3),
                values.len(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let medians = samples
        .into_iter()
        .filter(|(_, v)| v.len() >= parameters.minimum_class_samples)
        .map(|(key, mut values)| (key, weighted_median(&mut values).unwrap()))
        .collect::<BTreeMap<_, _>>();
    let estimate = |country, class, environment, vehicle| {
        medians
            .get(&(country, class, environment, vehicle))
            .or_else(|| medians.get(&(country, class, 0, vehicle)))
            .or_else(|| medians.get(&(0, class, environment, vehicle)))
            .or_else(|| medians.get(&(0, class, 0, vehicle)))
            .copied()
    };
    let mut priors = Vec::new();
    let mut shares = Vec::new();
    for country in countries {
        let mut prior = [[0.0; 3]; 2];
        let mut share = [[[0.0; 2]; 2]; 2];
        let mut supported = true;
        for (index, class) in [3, 4].into_iter().enumerate() {
            for environment in 0..3 {
                if let Some(value) = estimate(country, class, environment, 0) {
                    prior[index][usize::from(environment)] = value;
                } else {
                    supported = false;
                }
            }
            for environment in 1..=2 {
                for vehicle in 1..=2 {
                    if let Some(value) = estimate(country, class, environment, vehicle) {
                        share[index][usize::from(environment - 1)][usize::from(vehicle - 1)] =
                            value;
                    } else {
                        supported = false;
                    }
                }
            }
        }
        if supported {
            priors.push(country_row(country, json!(prior)));
            shares.push(country_row(country, json!(share)));
        }
    }
    ensure!(
        priors.iter().any(|r| r.get("countries").is_some()),
        "no observed global secondary/tertiary model"
    );
    let mut counted = [[[0.0; 2]; 2]; 5];
    for (class, environments) in counted.iter_mut().enumerate() {
        for (environment, vehicles) in environments.iter_mut().enumerate() {
            for (vehicle, value) in vehicles.iter_mut().enumerate() {
                *value = estimate(0,class as u8,environment as u8 + 1,vehicle as u8 + 1)
                    .ok_or_else(||anyhow::anyhow!("no measured class fraction for road {class}, environment {environment}, vehicle {vehicle}"))?;
            }
            ensure!(
                vehicles.iter().sum::<f64>() <= 100.0,
                "incompatible measured class fractions"
            );
        }
    }
    write(directory, "road-class-priors", &json!({"rows":priors}))?;
    write(directory, "road-class-shares", &json!({"rows":shares}))?;
    write(directory, "counted-class-shares", &json!({"rows":counted}))?;
    let section_totals = (0..6)
        .map(|class| estimate(0, class, 0, 0))
        .collect::<Vec<_>>();
    write(
        directory,
        "road-section-totals",
        &json!({"rows":section_totals}),
    )?;
    let mut section_classes = [[0.0; 3]; 6];
    for (class, values) in section_classes.iter_mut().enumerate() {
        for (vehicle, value) in values.iter_mut().enumerate() {
            *value = estimate(0, class as u8, 0, vehicle as u8).ok_or_else(|| {
                anyhow::anyhow!("no measured section prior for class {class}, quantity {vehicle}")
            })?;
        }
    }
    Ok(ClassModel {
        sample_counts: json!(counts),
        section_classes,
    })
}
