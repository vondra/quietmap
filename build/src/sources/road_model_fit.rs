//! Fit the building-road models on the year's counted roads and the trip ends its buildings make.

use super::road_model_regression::{
    NormalEquations, country_weights, log_error_db, rounded, weighted_quantile,
};
use super::road_model_rows::{
    CountedRow, MAIN_CANDIDATE_RADII, MAIN_CLASSES, MAJOR_RADII, RADII_M,
};
use super::road_traffic::{MAJOR_TRIP_END_OFFSET, major_group};
use crate::roads::observation_sample::SamplingDesign;
use crate::source_tables::{MainModel, MajorModel, RegionalMainModels, RoadModels};
use crate::year::YearInputs;
use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A main-class model holds its trip ends at the counted rows' 90th percentile: above it the
/// law is extrapolated where few counted roads stand.
const MAIN_TRIP_END_QUANTILE: f64 = 0.9;

/// A whole network stands for its class where it covers most of the class's mapped length in its
/// country. On the 2026 counts no whole network covered between 25 % and 68 % of its class, so any
/// bound between gives the same fit: those above (Sweden's, Czechia's and Poland's secondary
/// roads, Sweden's tertiary roads, Thailand's rural network) read within 1.6 dB of the laws fitted
/// on them and Great Britain's random sample, those below from -4.6 to +6.8 dB (Germany's federal
/// and Poland's voivodeship roads the busy part of the tertiary class, Finland's matched state
/// roads a quiet part).
pub(crate) const REPRESENTATIVE_COVERAGE: f64 = 0.5;

/// Whether a counted row stands for the uncounted roads of its class: a random sample always, a
/// whole network that covers most of its class, never sites a publisher selected.
pub(crate) fn represents_its_class(r: &CountedRow) -> bool {
    match r.design {
        SamplingDesign::RandomSample => true,
        SamplingDesign::WholeNetwork => r.class_coverage > REPRESENTATIVE_COVERAGE,
        SamplingDesign::SelectedSites | SamplingDesign::Undeclared => false,
    }
}

/// The fit of one main class at the candidate radius that predicts best, on the rows of `rows`
/// whose counts represent their class ([`represents_its_class`]), with its support:
/// each candidate's weighted mean absolute error and, for every row of `rows`, the residuals of
/// each country and design at the chosen law.
fn main_model(class: u8, rows: &[&CountedRow]) -> Option<(MainModel, Value)> {
    let mut best: Option<(f64, MainModel, usize, f64, Vec<f64>)> = None;
    let mut errors = serde_json::Map::new();
    for radius in MAIN_CANDIDATE_RADII {
        let population: Vec<&CountedRow> = rows
            .iter()
            .copied()
            .filter(|r| r.around[radius] > 0.0 && represents_its_class(r))
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
        let mut equations = NormalEquations::new(4);
        for (r, &w) in population.iter().zip(&weights).filter(|(_, w)| **w > 0.0) {
            equations.add(&main_design(r, radius, cap), r.total.ln(), w);
        }
        if equations.support(0) == 0.0 || equations.support(1) == 0.0 {
            continue;
        }
        let Some(b) = equations.solve() else { continue };
        let (sum, weight) =
            population
                .iter()
                .zip(&weights)
                .fold((0.0, 0.0), |(sum, weight), (r, &w)| {
                    (
                        sum + w * log_error_db(main_residual(r, radius, cap, &b)),
                        weight + w,
                    )
                });
        let error = sum / weight;
        errors.insert(format!("{}", RADII_M[radius]), json!(error));
        let model = MainModel {
            class,
            radius_m: RADII_M[radius],
            most_around: cap,
            intercept: [rounded(b[0]), rounded(b[1])],
            slope: rounded(b[2]),
            oneway: rounded(b[3]),
        };
        if best.as_ref().is_none_or(|(e, ..)| error < *e) {
            best = Some((error, model, radius, cap, b));
        }
    }
    let (_, model, radius, cap, b) = best?;
    let residuals: Vec<(&CountedRow, f64)> = rows
        .iter()
        .filter(|r| r.around[radius] > 0.0)
        .map(|r| (*r, main_residual(r, radius, cap, &b)))
        .collect();
    Some((
        model,
        json!({"class": class, "countries": design_errors(&residuals), "mean_absolute_db": errors}),
    ))
}

fn main_design(r: &CountedRow, radius: usize, cap: f64) -> [f64; 4] {
    [
        f64::from(r.built_up == 1),
        f64::from(r.built_up == 2),
        r.around[radius].min(cap).ln(),
        f64::from(r.oneway),
    ]
}

/// A row's natural-log residual (count over prediction) at a main-class law.
fn main_residual(r: &CountedRow, radius: usize, cap: f64, b: &[f64]) -> f64 {
    let predicted: f64 = main_design(r, radius, cap)
        .iter()
        .zip(b)
        .map(|(x, c)| x * c)
        .sum();
    r.total.ln() - predicted
}

/// For each country and sampling design: rows, km, the length-weighted mean residual and mean
/// absolute residual (dB).
fn design_errors(residuals: &[(&CountedRow, f64)]) -> Value {
    let mut by: BTreeMap<String, [f64; 4]> = BTreeMap::new();
    for (r, e) in residuals {
        let design = serde_json::to_value(r.design).expect("a design name");
        let key = format!(
            "{} {}",
            String::from_utf8_lossy(&r.country),
            design.as_str().unwrap_or_default()
        );
        let signed = e * 10.0 / std::f64::consts::LN_10;
        let entry = by.entry(key).or_default();
        *entry = [
            entry[0] + 1.0,
            entry[1] + r.length_m,
            entry[2] + r.length_m * signed,
            entry[3] + r.length_m * signed.abs(),
        ];
    }
    let tenth = |v: f64| (v * 10.0).round() / 10.0;
    json!(
        by.into_iter()
            .map(|(key, [n, metres, bias, absolute])| (
                key,
                json!({"rows": n, "km": tenth(metres / 1_000.0), "bias_db": tenth(bias / metres),
                    "mean_absolute_db": tenth(absolute / metres)})
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
    let residuals: Vec<(&CountedRow, f64)> = rows
        .iter()
        .map(|r| {
            let x = major_design(r);
            (
                *r,
                r.total.ln() - x.iter().zip(&b).map(|(x, c)| x * c).sum::<f64>(),
            )
        })
        .collect();
    Ok((
        models,
        json!({"held_out": support, "countries": design_errors(&residuals)}),
    ))
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

/// Each country's main classes, on and off a rural road network: mapped km, km under a whole
/// network's counts, their share and whether they stand for the class.
fn coverage_record(coverage: &super::road_model_rows::Coverage) -> Value {
    json!(
        coverage
            .iter()
            .filter(|(_, [_, counted])| *counted > 0.0)
            .map(|((country, class, network), [mapped, counted])| {
                let share = counted / mapped;
                json!({"country": String::from_utf8_lossy(country), "class": class,
                    "rural_network": network, "mapped_km": (mapped / 100.0).round() / 10.0,
                    "whole_network_km": (counted / 100.0).round() / 10.0, "share": share,
                    "represents_its_class": share > REPRESENTATIVE_COVERAGE})
            })
            .collect::<Vec<_>>()
    )
}

/// The year run's step: the counted rows of every prepared square with mapped buildings.
pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "building-road-models needs a new output path"
    );
    let (rows, coverage, squares) = super::road_model_rows::collect(inputs)?;
    eprintln!(
        "building-road-models: {} counted rows in {} squares",
        rows.len(),
        squares.len()
    );
    let (models, mut support) = fit_models(&rows).context("building-road-models")?;
    crate::source_tables::write_rows(output, &[models]).map_err(|e| anyhow!(e))?;
    support["squares"] = json!(squares);
    support["coverage"] = coverage_record(&coverage);
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
