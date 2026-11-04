//! MLIT census sections retain road identities; unlocated route and class summaries remain explicit derived observations.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_target::{Identity, IdentityField},
    },
    year::YearInputs,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    fallback_source: Source,
    files: Vec<PathBuf>,
    weekday_to_model_daily: f64,
}

fn normalize(value: &str) -> String {
    crate::roads::canonical_road_name(value)
        .split_whitespace()
        .collect()
}
fn name_pattern(value: &str) -> String {
    value
        .chars()
        .map(|c| regex::escape(&c.to_string()))
        .collect::<Vec<_>>()
        .join(" ?")
}

fn digits(value: &str) -> String {
    normalize(value)
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect()
}
fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut values = values.collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        ((values[n / 2 - 1] + values[n / 2]) / 2.0).round()
    }
}

fn observation(
    recipe: &Recipe,
    inputs: &YearInputs,
    source: &Source,
    key: String,
    rows: &[[f64; 3]],
    mask: u16,
    half: bool,
) -> Result<Observation> {
    let has_observed_sections = rows.iter().any(|r| r[2] == 1.0);
    let observed_only = source.source_id == recipe.source.source_id && has_observed_sections;
    let selected = rows
        .iter()
        .filter(|r| !observed_only || r[2] == 1.0)
        .collect::<Vec<_>>();
    let mut small = median(selected.iter().map(|r| r[0]));
    let mut large = median(selected.iter().map(|r| r[1]));
    if half {
        small = (small / 2.0).round();
        large = (large / 2.0).round();
    }
    let mut result = source.observation(inputs, key, CountBasis::BothDirections)?;
    result.vehicle_classes = [Some(small), None, None, None];
    result.vehicle_groups = vec![crate::roads::observation_groups::VehicleGroup {
        classes: 6,
        count: large,
    }];
    result.total = None;
    result.time_basis = TimeBasis::WeekdayDaily;
    result.daily_factor = recipe.weekday_to_model_daily;
    result.measured = observed_only;
    result.estimated_classes = if result.measured { 14 } else { 15 };
    result.road_class_mask = mask;
    Ok(result)
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    root: &Path,
) -> Result<Vec<Observation>> {
    ensure!(
        !recipe.files.is_empty(),
        "MLIT has no declared census objects"
    );
    ensure!(
        recipe.source.input_id == recipe.fallback_source.input_id,
        "MLIT summaries must cite their census input"
    );
    let mut routes = BTreeMap::<String, Vec<[f64; 3]>>::new();
    let mut names = BTreeMap::<String, Vec<[f64; 3]>>::new();
    let mut classes = BTreeMap::<u8, Vec<[f64; 3]>>::new();
    for file in &recipe.files {
        ensure!(
            !file.is_absolute() && file.components().all(|c| matches!(c, Component::Normal(_))),
            "invalid MLIT object path"
        );
        let bytes = std::fs::read(root.join(file))?;
        let (decoded, _, errors) = encoding_rs::SHIFT_JIS.decode(&bytes);
        ensure!(!errors, "invalid Shift-JIS census input");
        let mut reader = csv::ReaderBuilder::new()
            .flexible(true)
            .from_reader(decoded.as_bytes());
        let mut admitted = 0;
        for record in reader.records() {
            let fields = record?;
            if fields.len() <= 60 {
                continue;
            }
            // These positions and type codes are the publisher's kasyo schema, independent of its edition.
            let target: &[u8] = match &fields[3] {
                "1" | "2" => &[0],
                "3" => &[1, 2],
                "4" => &[2, 3],
                "6" => &[4],
                _ => continue,
            };
            let count = |column: usize| -> Option<f64> {
                let v = fields[column].trim();
                if v.is_empty() {
                    return None;
                }
                v.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
            };
            let (Some(small), Some(large)) = (count(59), count(60)) else {
                continue;
            };
            if small + large == 0.0 {
                continue;
            }
            // MLIT kasyo explanation section 19: 1 observed, 2 publisher estimate.
            let flags = [fields[34].trim(), fields[46].trim()];
            let measured = flags.contains(&"1") && flags.iter().all(|f| matches!(*f, "1" | ""));
            let counts = [small, large, f64::from(measured)];
            admitted += 1;
            for class in target {
                classes.entry(*class).or_default().push(counts);
            }
            if fields[3] == *"3" {
                let reference = digits(&fields[4]);
                if !reference.is_empty() {
                    routes.entry(reference).or_default().push(counts);
                }
            } else if matches!(&fields[3], "1" | "2") {
                let name = normalize(&fields[5]);
                if !name.is_empty() {
                    names.entry(name).or_default().push(counts);
                }
            }
        }
        ensure!(
            admitted > 0,
            "MLIT object {} has no usable sections",
            file.display()
        );
    }
    let mut output = Vec::new();
    for (reference, rows) in routes {
        let mut row = observation(
            recipe,
            inputs,
            &recipe.source,
            format!("route:{reference}"),
            &rows,
            6,
            false,
        )?;
        row.target.identities = vec![
            Identity {
                field: IdentityField::Reference,
                pattern: format!(r"(?:^|[;,/])[^0-9]*{reference}(?:[^0-9]|$)"),
                road_class_mask: 2,
                priority: 1,
                prefer_first_match: true,
            },
            Identity {
                field: IdentityField::Name,
                pattern: format!(r"(?:一般)?国道{reference}号"),
                road_class_mask: 6,
                priority: 0,
                prefer_first_match: true,
            },
        ];
        output.push(row);
    }
    for (name, rows) in names {
        let mut row = observation(
            recipe,
            inputs,
            &recipe.source,
            format!("expressway:{name}"),
            &rows,
            1,
            false,
        )?;
        let chars = name.chars().collect::<Vec<_>>();
        if chars.len() < 4 {
            continue;
        }
        let substrings = (0..chars.len())
            .flat_map(|start| (start + 4..=chars.len()).map(move |end| (start, end)))
            .map(|(start, end)| name_pattern(&chars[start..end].iter().collect::<String>()))
            .collect::<Vec<_>>()
            .join("|");
        row.target.identities = vec![
            Identity {
                field: IdentityField::Name,
                pattern: format!("^{}$", name_pattern(&name)),
                road_class_mask: 1,
                priority: u16::MAX,
                prefer_first_match: false,
            },
            Identity {
                field: IdentityField::Name,
                pattern: format!("{}|^(?:{substrings})$", name_pattern(&name)),
                road_class_mask: 1,
                priority: u16::try_from(chars.len())?,
                prefer_first_match: false,
            },
        ];
        output.push(row);
    }
    for (class, rows) in classes {
        let row = observation(
            recipe,
            inputs,
            &recipe.fallback_source,
            format!("class:{class}"),
            &rows,
            1 << class,
            false,
        )?;
        output.push(row);
        if class <= 2 {
            let row = observation(
                recipe,
                inputs,
                &recipe.fallback_source,
                format!("class-link:{class}"),
                &rows,
                1 << (class + 10),
                true,
            )?;
            output.push(row);
        }
    }
    ensure!(!output.is_empty(), "MLIT census has no summaries");
    Ok(output)
}
