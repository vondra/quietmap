//! Normalize Cerema CSV editions and their documented integer-percentage encoding before common matching.

use super::{Source, records};
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::ReferenceRule,
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::{collections::HashSet, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    integer_percent_divisor: f64,
    pub older_editions: Vec<Edition>,
    motorcycle_share: f64,
    medium_share_of_heavy: f64,
    search_radius_m: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edition {
    pub source: Source,
    integer_percent_divisor: f64,
}

struct Section {
    observation: Observation,
    ratio: Option<f64>,
    midpoint: [f64; 2],
    coverage: String,
}

fn localized(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn edition(
    recipe: &Recipe,
    source: &Source,
    divisor: f64,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Section>> {
    ensure!(
        divisor.is_finite() && divisor > 0.0,
        "invalid Cerema integer percentage divisor"
    );
    let mut csv = csv::ReaderBuilder::new().delimiter(b';').from_path(path)?;
    let headers = csv
        .headers()?
        .iter()
        .map(|s| s.trim().trim_start_matches('\u{feff}').to_owned())
        .collect::<Vec<_>>();
    let positions = ["route", "TMJA", "ratio_PL", "xD", "yD", "xF", "yF"]
        .map(|name| {
            headers
                .iter()
                .position(|h| h == name)
                .ok_or_else(|| anyhow!("missing Cerema column {name}"))
        })
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let transform = records::projection(2154)?;
    let strip = regex::Regex::new(r"^([A-Z])0*")?;
    let mut sections = Vec::new();
    for (index, record) in csv.records().enumerate() {
        let record = record?;
        let value = |column: usize| record.get(positions[column]).unwrap_or("").trim();
        let route = value(0);
        let Some(total) = localized(value(1)).map(f64::round).filter(|v| *v > 0.0) else {
            continue;
        };
        if route.is_empty() {
            continue;
        }
        let mut ratio = localized(value(2)).filter(|v| *v != 0.0);
        if !value(2).contains(['.', ',']) {
            ratio = ratio.map(|v| v / divisor);
        }
        if ratio.is_some_and(|v| !(0.0..=100.0).contains(&v)) {
            continue;
        }
        let (Some(x), Some(y)) = (
            localized(value(3)).filter(|v| *v >= 100_000.0),
            localized(value(4)),
        ) else {
            continue;
        };
        let mut line = vec![records::project(&transform, [x, y])?];
        let end = localized(value(5))
            .zip(localized(value(6)))
            .filter(|(x, y)| *x != 0.0 && *y != 0.0);
        if let Some(end) = end {
            line.push(records::project(&transform, [end.0, end.1])?);
        }
        let midpoint = records::project(
            &transform,
            end.map_or([x, y], |(ex, ey)| [(x + ex) / 2.0, (y + ey) / 2.0]),
        )?;
        let reference = strip.replace(route, "$1").to_string();
        let mut row = source.observation(
            inputs,
            format!("{}:{index}", recipe.source.input_id),
            CountBasis::BothDirections,
        )?;
        row.reference = reference.clone();
        row.reference_required = true;
        row.target.reference_rule = ReferenceRule::Whole;
        row.geometry = vec![line];
        row.road_class_mask = (1 << 10) - 1;
        row.search_radius_m = recipe.search_radius_m;
        row.total = Some(total);
        row.estimated_classes = 15;
        sections.push(Section {
            observation: row,
            ratio: ratio.map(|v| v / 100.0),
            midpoint,
            coverage: format!("{reference}:{:.2}:{:.2}", midpoint[1], midpoint[0]),
        });
    }
    let published = sections
        .iter()
        .filter_map(|s| {
            s.ratio
                .map(|ratio| (s.observation.reference.clone(), s.midpoint, ratio))
        })
        .collect::<Vec<_>>();
    let mut ratios = published.iter().map(|r| r.2).collect::<Vec<_>>();
    ratios.sort_by(f64::total_cmp);
    let median = (!ratios.is_empty()).then(|| ratios[(ratios.len() - 1) / 2]);
    for section in &mut sections {
        let ratio = section
            .ratio
            .or_else(|| {
                published
                    .iter()
                    .filter(|r| r.0 == section.observation.reference)
                    .min_by(|a, b| {
                        crate::roads::flat_distance(section.midpoint, a.1)
                            .total_cmp(&crate::roads::flat_distance(section.midpoint, b.1))
                    })
                    .map(|r| r.2)
            })
            .or(median);
        let total = section.observation.total.unwrap();
        let heavy = ratio.map(|v| (total * v).round());
        if heavy.is_some_and(|v| v + (total * recipe.motorcycle_share).round() > total) {
            continue;
        }
        section.observation.vehicle_classes = records::split_heavy(
            total,
            heavy,
            recipe.motorcycle_share,
            recipe.medium_share_of_heavy,
        );
    }
    sections.retain(|s| s.observation.vehicle_classes.iter().any(Option::is_some));
    Ok(sections)
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let mut rows = Vec::new();
    let mut covered = HashSet::new();
    let mut sources = vec![(
        &recipe.source,
        recipe.integer_percent_divisor,
        path.to_owned(),
    )];
    for older in &recipe.older_editions {
        sources.push((
            &older.source,
            older.integer_percent_divisor,
            inputs
                .by_id(&older.source.input_id)
                .map_err(anyhow::Error::msg)?,
        ));
    }
    sources.sort_by(|a, b| {
        b.0.input(inputs)
            .unwrap()
            .observed
            .bounds()
            .unwrap()
            .1
            .cmp(&a.0.input(inputs).unwrap().observed.bounds().unwrap().1)
    });
    for (source, divisor, path) in sources {
        let sections = edition(recipe, source, divisor, inputs, &path)?;
        let accepted = sections
            .into_iter()
            .filter(|s| !covered.contains(&s.coverage))
            .collect::<Vec<_>>();
        for section in accepted {
            covered.insert(section.coverage);
            rows.push(section.observation);
        }
    }
    Ok(rows)
}
