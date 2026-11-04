//! Convert pinned publisher JSON records to dated, nullable road observations selected by the manifest.

use super::records::{number, text};
pub(super) use super::{
    fields_recipe::Publisher,
    geometry::{GeometryFormat, geometry},
};
use crate::{roads::observation::Observation, year::YearInputs};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(super) fn observations(
    publisher: &Publisher,
    raw: &Value,
    inputs: &YearInputs,
) -> Result<Vec<Observation>> {
    let input = inputs
        .manifest
        .inputs
        .iter()
        .find(|r| r.id == publisher.input_id)
        .ok_or_else(|| anyhow!("unknown count input {}", publisher.input_id))?;
    ensure!(
        inputs
            .manifest
            .admitted(&input.id)
            .map_err(anyhow::Error::msg)?,
        "unadmitted count input"
    );
    let (start, end) = input.observed.bounds().map_err(anyhow::Error::msg)?;
    let country = publisher.country.as_bytes();
    ensure!(
        country.len() == 2
            && country.iter().all(u8::is_ascii_uppercase)
            && input.region.split(':').next() == Some(publisher.country.as_str()),
        "invalid publisher country"
    );
    let records = raw
        .pointer(&publisher.records)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("missing publisher records"))?;
    let mut rows = Vec::new();
    let coverage = publisher
        .coverage
        .as_ref()
        .map(|r| super::records::coverage(inputs, &r.input_id, &r.property, &r.name))
        .transpose()?;
    let empty_suffix = vec![String::new()];
    let suffixes = if publisher.reference_alias_suffixes.is_empty() {
        &empty_suffix
    } else {
        &publisher.reference_alias_suffixes
    };
    let field_filter = publisher
        .field_filter
        .as_ref()
        .map(|(field, pattern)| Ok::<_, anyhow::Error>((field, regex::Regex::new(pattern)?)))
        .transpose()?;
    for record in records {
        let values = record
            .pointer(&publisher.properties)
            .ok_or_else(|| anyhow!("missing count properties"))?;
        if let Some((field, pattern)) = &field_filter
            && !pattern.is_match(&text(&values[*field]))
        {
            continue;
        }
        let mut reference = publisher
            .reference
            .as_ref()
            .map(|key| text(&values[key]))
            .unwrap_or_default();
        if let Some((field, prefixes)) = &publisher.reference_prefix_by_field {
            let prefix = prefixes
                .get(&text(&values[field]))
                .map_or("", String::as_str);
            reference = format!(
                "{prefix}{}",
                reference
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect::<String>()
            );
        }
        if !publisher.reference_alias_prefixes.is_empty() {
            reference = publisher
                .reference_alias_prefixes
                .iter()
                .flat_map(|prefix| {
                    let reference = &reference;
                    suffixes
                        .iter()
                        .map(move |suffix| format!("{prefix}{reference}{suffix}"))
                })
                .collect::<Vec<_>>()
                .join(";");
        }
        let mask = if let Some((field, classes)) = &publisher.class_masks_by_field {
            let Some(mask) = classes.get(&text(&values[field])) else {
                continue;
            };
            *mask & publisher.road_class_mask
        } else {
            publisher.road_class_mask
        };
        if mask == 0 || publisher.reference_required && reference.is_empty() {
            continue;
        }
        let mut counts = [None; 4];
        for (i, fields) in publisher.classes.iter().enumerate() {
            if let Some(fields) = fields {
                let values = fields
                    .iter()
                    .map(|key| number(&values[key]))
                    .collect::<Result<Vec<_>>>()?;
                if values.iter().all(Option::is_some) {
                    counts[i] = Some(values.into_iter().flatten().sum());
                }
            }
        }
        let mut total = publisher
            .total
            .as_ref()
            .map(|field| number(&values[field]))
            .transpose()?
            .flatten();
        for field in &publisher.total_fallback_fields {
            if total.is_none() {
                total = number(&values[field])?;
            }
        }
        ensure!(
            publisher.total_scale.is_finite() && publisher.total_scale > 0.0,
            "invalid published count unit"
        );
        total = total.map(|n| n * publisher.total_scale);
        if total.is_some_and(|v| v < publisher.minimum_total) {
            continue;
        }
        if publisher.percentages {
            let Some(total) = total else {
                continue;
            };
            let scale = if let Some(target) = publisher.percentage_total {
                let sum = counts.iter().flatten().sum::<f64>();
                if counts.iter().filter(|v| v.is_some()).count()
                    != publisher.classes.iter().filter(|v| v.is_some()).count()
                    || sum == 0.0
                {
                    continue;
                }
                target / sum
            } else {
                1.0
            };
            counts = counts.map(|v| v.map(|n| (total * n * scale / 100.0).round()));
        }
        for (class, fraction) in publisher.fixed_fractions.iter().enumerate() {
            if let (Some(fraction), Some(total)) = (fraction, total) {
                ensure!(
                    fraction.is_finite() && (0.0..=1.0).contains(fraction),
                    "invalid publisher class fraction"
                );
                counts[class] = Some((total * fraction).round());
            }
        }
        if let Some(class) = publisher.residual_class {
            ensure!(class < 4, "invalid residual vehicle class");
            if let Some(total) = total.filter(|_| {
                counts
                    .iter()
                    .enumerate()
                    .all(|(i, n)| i == class || n.is_some())
            }) {
                let remaining = total
                    - counts
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != class)
                        .filter_map(|(_, n)| *n)
                        .sum::<f64>();
                ensure!(remaining >= 0.0, "published classes exceed their total");
                counts[class] = Some(remaining);
            }
        }
        if total.is_none() && counts.iter().all(Option::is_none) {
            continue;
        }
        if publisher.skip_zero_total
            && total.unwrap_or_else(|| counts.iter().flatten().sum()) == 0.0
        {
            continue;
        }
        let (observed_start, observed_end) = if let Some(field) = &publisher.year_field {
            let year: i32 = text(&values[field]).parse()?;
            let from = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
                .ok_or_else(|| anyhow!("invalid count year"))?;
            let to = chrono::NaiveDate::from_ymd_opt(year, 12, 31)
                .ok_or_else(|| anyhow!("invalid count year"))?;
            if from < start || to > end {
                continue;
            }
            (from, to)
        } else {
            (start, end)
        };
        let mut target = publisher.target.clone();
        if let Some(coverage) = &coverage {
            target.coverage = coverage.clone();
        }
        let mut groups = Vec::new();
        for (classes, fields) in &publisher.class_groups {
            let values = fields
                .iter()
                .map(|field| number(&values[field]))
                .collect::<Result<Vec<_>>>()?;
            if values.iter().all(Option::is_some) {
                let mut count: f64 = values.into_iter().flatten().sum();
                if publisher.percentages {
                    count = (total.unwrap() * count / 100.0).round();
                }
                groups.push(crate::roads::observation_groups::VehicleGroup {
                    classes: *classes,
                    count,
                });
            }
        }
        rows.push(Observation {
            identity: if values[&publisher.identity].is_null() {
                super::hpms::identity(record)
            } else {
                text(&values[&publisher.identity])
            },
            input_id: publisher.input_id.clone(),
            source_id: publisher.source_id,
            source_rank: publisher.source_rank,
            measured: publisher.measured,
            country: u16::from_le_bytes(country.try_into()?),
            observed_start: observed_start.to_string(),
            observed_end: observed_end.to_string(),
            count_basis: publisher.count_basis,
            time_basis: publisher.time_basis,
            direction: publisher.direction,
            reference,
            geometry: geometry(record, values, &publisher.geometry)?,
            road_class_mask: mask,
            search_radius_m: publisher.search_radius_m,
            reference_required: publisher.reference_required,
            vehicle_classes: counts,
            vehicle_groups: groups,
            total,
            estimated_classes: publisher.estimated_classes,
            daily_factor: publisher.daily_factor,
            target,
            sample: Default::default(),
        });
    }
    Ok(rows)
}

pub(super) fn convert(
    publisher: &Publisher,
    path: &std::path::Path,
    inputs: &YearInputs,
) -> Result<Vec<Observation>> {
    let mut rows = if publisher.files.is_empty() {
        observations(
            publisher,
            &serde_json::from_slice(&std::fs::read(path)?)?,
            inputs,
        )?
    } else {
        let mut rows = Vec::new();
        for file in &publisher.files {
            let raw =
                serde_json::from_slice(&std::fs::read(super::records::object_path(path, file)?)?)?;
            rows.extend(observations(publisher, &raw, inputs)?);
        }
        rows
    };
    if let Some(exclusion) = &publisher.exclusion {
        super::exclusions::attach(exclusion, inputs, &mut rows)?;
    }
    Ok(rows)
}
