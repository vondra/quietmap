//! Fit the building-road models on the year's counted roads and the trip ends its buildings make.

use super::road_model_regression::{
    NormalEquations, country_weights, log_error_db, rounded, weighted_quantile,
};
use super::road_model_rows::{
    CountedRow, MAIN_CANDIDATE_RADII, MAIN_CLASSES, MAJOR_RADII, RADII_M,
};
use super::road_traffic::{MAJOR_TRIP_END_OFFSET, major_group};
use crate::source_tables::{MainModel, MajorModel, RegionalMainModels, RoadModels};
use crate::year::YearInputs;
use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A main-class model holds its trip ends at the counted rows' 90th percentile: above it the
/// law is extrapolated where few counted roads stand.
const MAIN_TRIP_END_QUANTILE: f64 = 0.9;

/// The fit of one main class on `rows` at the candidate radius that predicts them best, with its
/// support: each candidate's weighted mean absolute error and each country's rows, km, bias and
/// mean absolute error at the chosen one (dB).
fn main_model(class: u8, rows: &[&CountedRow]) -> Option<(MainModel, Value)> {
    let mut best: Option<(f64, MainModel, Vec<Residual>)> = None;
    let mut errors = serde_json::Map::new();
    for radius in MAIN_CANDIDATE_RADII {
        let population: Vec<&CountedRow> = rows
            .iter()
            .copied()
            .filter(|r| r.around[radius] > 0.0)
            .collect();
        let weights = country_weights(
            &population
                .iter()
                .map(|r| (r.country, r.length_m))
                .collect::<Vec<_>>(),
        );
        let trip_ends: Vec<(f64, f64)> = population
            .iter()
            .zip(&weights)
            .map(|(r, &w)| (r.around[radius], w))
            .collect();
        let Some(cap) = weighted_quantile(&trip_ends, MAIN_TRIP_END_QUANTILE) else {
            continue;
        };
        let cap = cap.round().max(1.0);
        let design = |r: &CountedRow| {
            [
                f64::from(r.built_up == 1),
                f64::from(r.built_up == 2),
                r.around[radius].min(cap).ln(),
                f64::from(r.oneway),
            ]
        };
        let mut equations = NormalEquations::new(4);
        for (r, &w) in population.iter().zip(&weights).filter(|(_, w)| **w > 0.0) {
            equations.add(&design(r), r.total.ln(), w);
        }
        if equations.support(0) == 0.0 || equations.support(1) == 0.0 {
            continue;
        }
        let Some(b) = equations.solve() else { continue };
        let residuals: Vec<Residual> = population
            .iter()
            .zip(&weights)
            .filter(|(_, w)| **w > 0.0)
            .map(|(r, &w)| {
                let predicted: f64 = design(r).iter().zip(&b).map(|(x, c)| x * c).sum();
                (*r, w, r.total.ln() - predicted)
            })
            .collect();
        let error = residuals
            .iter()
            .map(|(_, w, e)| w * log_error_db(*e))
            .sum::<f64>()
            / residuals.iter().map(|(_, w, _)| w).sum::<f64>();
        errors.insert(format!("{}", RADII_M[radius]), json!(error));
        let model = MainModel {
            class,
            radius_m: RADII_M[radius],
            most_around: cap,
            intercept: [rounded(b[0]), rounded(b[1])],
            slope: rounded(b[2]),
            oneway: rounded(b[3]),
        };
        if best.as_ref().is_none_or(|(e, _, _)| error < *e) {
            best = Some((error, model, residuals));
        }
    }
    let (_, model, residuals) = best?;
    Some((
        model,
        json!({"class": class, "countries": country_errors(&residuals), "mean_absolute_db": errors}),
    ))
}

/// A fitted row, its weight and its natural-log residual (count over prediction).
type Residual<'a> = (&'a CountedRow, f64, f64);

/// Each country's rows, km, weighted mean residual and mean absolute residual (dB).
fn country_errors(residuals: &[Residual]) -> Value {
    let mut by: BTreeMap<String, [f64; 5]> = BTreeMap::new();
    for (r, w, e) in residuals {
        let entry = by
            .entry(String::from_utf8_lossy(&r.country).into())
            .or_default();
        let signed = e * 10.0 / std::f64::consts::LN_10;
        *entry = [
            entry[0] + 1.0,
            entry[1] + r.length_m / 1_000.0,
            entry[2] + w * signed,
            entry[3] + w * signed.abs(),
            entry[4] + w,
        ];
    }
    let tenth = |v: f64| (v * 10.0).round() / 10.0;
    json!(
        by.into_iter()
            .map(|(c, [n, km, bias, absolute, w])| (
                c,
                json!({"rows": n, "km": tenth(km), "bias_db": tenth(bias / w), "mean_absolute_db": tenth(absolute / w)})
            ))
            .collect::<serde_json::Map<_, _>>()
    )
}

/// Columns of the major-road design: an intercept for each group and surroundings, a slope for
/// each group and radius, then one-way, ln lanes and unknown lanes.
const MAJOR_COLUMNS: usize = 8 + 12 + 3;

/// The intercept column of a major row: its group's rural or urban one.
fn major_cell(r: &CountedRow) -> usize {
    major_group(r.class).expect("a major row") * 2 + usize::from(r.built_up - 1)
}

fn major_design(r: &CountedRow) -> [f64; MAJOR_COLUMNS] {
    let group = major_group(r.class).expect("a major row");
    let mut x = [0.0; MAJOR_COLUMNS];
    x[major_cell(r)] = 1.0;
    for (k, &radius) in MAJOR_RADII.iter().enumerate() {
        x[8 + k * 4 + group] = (r.around[radius] + MAJOR_TRIP_END_OFFSET).ln();
    }
    x[20] = f64::from(r.oneway);
    x[21] = if r.lanes > 0 {
        f64::from(r.lanes).ln()
    } else {
        0.0
    };
    x[22] = f64::from(r.lanes == 0);
    x
}

/// The major-road model on all countries, and the countries where it, fitted without them,
/// predicts their counts better than the other countries' class constants do.
fn major_models(rows: &[&CountedRow]) -> Result<(RoadModels, Value)> {
    let weights = country_weights(
        &rows
            .iter()
            .map(|r| (r.country, r.length_m))
            .collect::<Vec<_>>(),
    );
    let mut all = NormalEquations::new(MAJOR_COLUMNS);
    let mut by_country: BTreeMap<[u8; 2], NormalEquations> = BTreeMap::new();
    for (r, &w) in rows.iter().zip(&weights).filter(|(_, w)| **w > 0.0) {
        let x = major_design(r);
        all.add(&x, r.total.ln(), w);
        by_country
            .entry(r.country)
            .or_insert_with(|| NormalEquations::new(MAJOR_COLUMNS))
            .add(&x, r.total.ln(), w);
    }
    ensure!(
        (0..8).all(|c| all.support(c) > 0.0),
        "major-road fit: a group lacks rural or urban counted rows"
    );
    let b = all
        .solve()
        .ok_or_else(|| anyhow!("major-road fit: singular design"))?;
    // Per country: the held-out model's and constants' weighted absolute errors, the weight,
    // the rows and km.
    let mut errors: BTreeMap<[u8; 2], [f64; 5]> = BTreeMap::new();
    let held_out: BTreeMap<[u8; 2], Option<(Vec<f64>, NormalEquations)>> = by_country
        .iter()
        .map(|(country, part)| {
            let rest = all.without(part);
            let identified = (0..8).all(|c| part.support(c) == 0.0 || rest.support(c) > 1e-9);
            (
                *country,
                identified
                    .then(|| rest.solve().map(|b| (b, rest)))
                    .flatten(),
            )
        })
        .collect();
    for (r, &w) in rows.iter().zip(&weights).filter(|(_, w)| **w > 0.0) {
        let Some(Some((b, rest))) = held_out.get(&r.country) else {
            continue;
        };
        let x = major_design(r);
        let cell = major_cell(r);
        let model: f64 = x.iter().zip(b).map(|(x, c)| x * c).sum();
        let constant = rest.indicator_mean(cell).expect("identified cell");
        let entry = errors.entry(r.country).or_default();
        entry[0] += w * log_error_db(r.total.ln() - model);
        entry[1] += w * log_error_db(r.total.ln() - constant);
        entry[2] += w;
        entry[3] += 1.0;
        entry[4] += r.length_m / 1_000.0;
    }
    let major_countries: Vec<[u8; 2]> = errors
        .iter()
        .filter(|(_, [m, c, ..])| m < c)
        .map(|(country, _)| *country)
        .collect();
    let support: serde_json::Map<String, Value> = errors
        .iter()
        .map(|(country, [m, c, w, n, km])| {
            (
                String::from_utf8_lossy(country).into(),
                json!({"rows": n, "km": (km * 10.0).round() / 10.0,
                    "held_out_model_db": m / w, "held_out_constants_db": c / w}),
            )
        })
        .collect();
    let models = RoadModels {
        main: Vec::new(),
        regional_main: Vec::new(),
        major: std::array::from_fn(|g| MajorModel {
            intercept: [rounded(b[g * 2]), rounded(b[g * 2 + 1])],
            slopes: std::array::from_fn(|k| rounded(b[8 + k * 4 + g])),
        }),
        major_radii_m: MAJOR_RADII.map(|radius| RADII_M[radius]),
        major_oneway: rounded(b[20]),
        major_ln_lanes: rounded(b[21]),
        major_lanes_unknown: rounded(b[22]),
        major_countries,
    };
    Ok((models, json!({"held_out": support})))
}

/// Every model of the table from the counted rows: the major roads', each main class's on rows
/// off any rural road network, and a country's network's own where it has the support.
pub(crate) fn fit_models(rows: &[CountedRow]) -> Result<(RoadModels, Value)> {
    let major: Vec<&CountedRow> = rows
        .iter()
        .filter(|r| major_group(r.class).is_some())
        .collect();
    let (mut models, major_support) = major_models(&major)?;
    let mut main_support = Vec::new();
    for class in MAIN_CLASSES {
        let pooled: Vec<&CountedRow> = rows
            .iter()
            .filter(|r| r.class == class && !r.rural_network)
            .collect();
        if let Some((model, support)) = main_model(class, &pooled) {
            models.main.push(model);
            main_support.push(support);
        }
    }
    let mut networks: BTreeMap<[u8; 2], Vec<&CountedRow>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.rural_network) {
        networks.entry(r.country).or_default().push(r);
    }
    let mut regional_support = Vec::new();
    for (country, network) in networks {
        let mut fitted = Vec::new();
        for class in MAIN_CLASSES {
            let rows: Vec<&CountedRow> = network
                .iter()
                .copied()
                .filter(|r| r.class == class)
                .collect();
            if let Some((model, support)) = main_model(class, &rows) {
                fitted.push(model);
                regional_support.push(support);
            }
        }
        if !fitted.is_empty() {
            models.regional_main.push(RegionalMainModels {
                country,
                models: fitted,
            });
        }
    }
    ensure!(
        !models.main.is_empty(),
        "no main-class fit has a country of enough counted rows"
    );
    Ok((
        models,
        json!({"major": major_support, "main": main_support, "regional_main": regional_support}),
    ))
}

/// The year run's step: the counted rows of every prepared square with mapped buildings.
pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "building-road-models needs a new output path"
    );
    let (rows, squares) = super::road_model_rows::collect(inputs)?;
    eprintln!(
        "building-road-models: {} counted rows in {} squares",
        rows.len(),
        squares.len()
    );
    let (models, mut support) = fit_models(&rows).context("building-road-models")?;
    crate::source_tables::write_rows(output, &[models]).map_err(|e| anyhow!(e))?;
    support["squares"] = json!(squares);
    support["minimum_country_rows"] = json!(super::road_model_regression::MINIMUM_COUNTRY_ROWS);
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&support)?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "road_model_fit_tests.rs"]
mod tests;
