//! Convert pinned publisher JSON records to dated, nullable road observations selected by the manifest.

use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_io,
    },
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    objects: Vec<Publisher>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publisher {
    input_id: String,
    records: String,
    properties: String,
    identity: String,
    reference: Option<String>,
    geometry: GeometryFormat,
    source_id: u16,
    source_rank: u32,
    country: String,
    measured: bool,
    count_basis: CountBasis,
    time_basis: TimeBasis,
    daily_factor: f64,
    direction: u8,
    road_class_mask: u16,
    search_radius_m: f64,
    reference_required: bool,
    classes: [Option<Vec<String>>; 4],
    total: Option<String>,
    year_field: Option<String>,
    estimated_classes: u8,
    #[serde(default)]
    reference_prefix_by_field: Option<(String, BTreeMap<String, String>)>,
    #[serde(default)]
    class_masks_by_field: Option<(String, BTreeMap<String, u16>)>,
    #[serde(default)]
    reference_alias_prefixes: Vec<String>,
    #[serde(default)]
    skip_zero_total: bool,
}

#[derive(Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case", deny_unknown_fields)]
enum GeometryFormat {
    Geojson,
    Arcgis,
    Point { longitude: String, latitude: String },
}

fn number(value: &Value) -> Result<Option<f64>> {
    if value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()) {
        return Ok(None);
    }
    let number = match value.as_f64() {
        Some(number) => number,
        None => value
            .as_str()
            .ok_or_else(|| anyhow!("invalid count value"))?
            .trim()
            .parse()?,
    };
    ensure!(number.is_finite() && number >= 0.0, "invalid count {value}");
    Ok(Some(number))
}

fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}

fn geometry(
    value: &Value,
    properties: &Value,
    format: &GeometryFormat,
) -> Result<Vec<Vec<[f64; 2]>>> {
    let lines: Vec<Value> = match format {
        GeometryFormat::Point {
            longitude,
            latitude,
        } => vec![serde_json::json!([[
            properties[longitude],
            properties[latitude]
        ]])],
        GeometryFormat::Arcgis => value["geometry"]["paths"]
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("missing ArcGIS paths"))?,
        GeometryFormat::Geojson => match value["geometry"]["type"].as_str() {
            Some("Point") => vec![serde_json::json!([value["geometry"]["coordinates"]])],
            Some("LineString") => vec![value["geometry"]["coordinates"].clone()],
            Some("MultiLineString") => value["geometry"]["coordinates"]
                .as_array()
                .cloned()
                .ok_or_else(|| anyhow!("missing line parts"))?,
            _ => return Err(anyhow!("road observation requires point or line geometry")),
        },
    };
    lines
        .into_iter()
        .map(|line| {
            line.as_array()
                .ok_or_else(|| anyhow!("invalid road line"))?
                .iter()
                .map(|point| {
                    let coordinates = point
                        .as_array()
                        .ok_or_else(|| anyhow!("invalid road coordinate"))?;
                    ensure!(coordinates.len() >= 2, "short road coordinate");
                    Ok([
                        coordinates[0]
                            .as_f64()
                            .ok_or_else(|| anyhow!("invalid longitude"))?,
                        coordinates[1]
                            .as_f64()
                            .ok_or_else(|| anyhow!("invalid latitude"))?,
                    ])
                })
                .collect()
        })
        .collect()
}

fn observations(
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
        country.len() == 2 && country.iter().all(u8::is_ascii_uppercase),
        "invalid publisher country"
    );
    let records = raw
        .pointer(&publisher.records)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("missing publisher records"))?;
    let mut rows = Vec::new();
    for record in records {
        let values = record
            .pointer(&publisher.properties)
            .ok_or_else(|| anyhow!("missing count properties"))?;
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
                .map(|prefix| format!("{prefix}{reference}"))
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
        let total = publisher
            .total
            .as_ref()
            .map(|field| number(&values[field]))
            .transpose()?
            .flatten();
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
        rows.push(Observation {
            identity: text(&values[&publisher.identity]),
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
            total,
            estimated_classes: publisher.estimated_classes,
            daily_factor: publisher.daily_factor,
        });
    }
    Ok(rows)
}

pub fn convert(inputs: &YearInputs, output: &Path) -> Result<()> {
    let path = inputs
        .required(Kind::RoadConversion)
        .map_err(anyhow::Error::msg)?;
    let recipe: Recipe = serde_json::from_slice(&std::fs::read(path)?)?;
    let conversion = inputs
        .manifest
        .inputs
        .iter()
        .find(|r| r.kind == Kind::RoadConversion)
        .ok_or_else(|| anyhow!("missing converter input"))?;
    ensure!(!recipe.objects.is_empty(), "empty road converter recipe");
    let mut rows = Vec::new();
    for publisher in recipe.objects {
        ensure!(
            conversion.depends_on.contains(&publisher.input_id),
            "converter omits raw dependency {}",
            publisher.input_id
        );
        let path = inputs
            .by_id(&publisher.input_id)
            .map_err(anyhow::Error::msg)?;
        let raw: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let converted = observations(&publisher, &raw, inputs)?;
        eprintln!(
            "road-counts: {}: {} observations",
            publisher.input_id,
            converted.len()
        );
        rows.extend(converted);
    }
    observation_io::write(output, &rows, inputs)
}
