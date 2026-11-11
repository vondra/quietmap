//! Convert complete unrestricted FHWA lane-days into dated hourly samples, never whole-road AADT.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation, TimeBasis},
        observation_sample::{Hourly, Sample},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub members: Vec<String>,
}

struct Accumulator {
    hours: [f64; 24],
    days: BTreeSet<NaiveDate>,
    class: u8,
    environment: u8,
    weekday_days: u32,
    weekday_vehicles: f64,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut stations = BTreeMap::<String, Accumulator>::new();
    let (first, last) = recipe
        .source
        .input(inputs)?
        .observed
        .bounds()
        .map_err(anyhow::Error::msg)?;
    ensure!(!recipe.members.is_empty(), "no declared TMAS members");
    for member in &recipe.members {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(b'|')
            .from_reader(archive.by_name(member)?);
        let headers = reader
            .headers()?
            .iter()
            .map(|v| v.trim().to_lowercase())
            .collect::<Vec<_>>();
        let column = |name: &str| {
            headers
                .iter()
                .position(|v| v == name)
                .ok_or_else(|| anyhow!("TMAS column {name} missing"))
        };
        let indices = [
            "state_code",
            "station_id",
            "travel_dir",
            "travel_lane",
            "year_record",
            "month_record",
            "day_record",
            "f_system",
            "restrictions",
        ]
        .map(column)
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        let hours = (0..24)
            .map(|hour| column(&format!("hour_{hour:02}")))
            .collect::<Result<Vec<_>>>()?;
        for record in reader.records() {
            let record = record?;
            let value = |i| record.get(indices[i]).unwrap_or("").trim();
            if !matches!(value(8), "" | "0") {
                continue;
            }
            let date =
                NaiveDate::from_ymd_opt(value(4).parse()?, value(5).parse()?, value(6).parse()?)
                    .ok_or_else(|| anyhow!("invalid TMAS observation date"))?;
            if date < first || date > last {
                continue;
            }
            let counts = hours
                .iter()
                .map(|&i| {
                    record
                        .get(i)?
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                })
                .collect::<Option<Vec<_>>>();
            let Some(counts) = counts.filter(|v| v.iter().sum::<f64>() > 0.0) else {
                continue;
            };
            let function = value(7).as_bytes();
            let Some(&rank) = function.first().filter(|n| (b'1'..=b'7').contains(n)) else {
                continue;
            };
            let class = [0, 1, 2, 3, 4, 4, 5][usize::from(rank - b'1')];
            let environment = match function.get(1) {
                Some(b'R') => 1,
                Some(b'U') => 2,
                _ => 0,
            };
            // A station can be reclassified during the year; its two observed populations must not be pooled.
            let identity = format!(
                "{}/{}/{}/{}/{}",
                value(0).parse::<u16>()?,
                value(1),
                value(2),
                value(3),
                value(7)
            );
            let station = stations.entry(identity).or_insert(Accumulator {
                hours: [0.0; 24],
                days: BTreeSet::new(),
                class,
                environment,
                weekday_days: 0,
                weekday_vehicles: 0.0,
            });
            ensure!(
                station.class == class && station.environment == environment,
                "TMAS station classification changed inside an input interval"
            );
            ensure!(station.days.insert(date), "duplicate TMAS lane-day");
            if date.weekday().num_days_from_monday() < 5 {
                station.weekday_days += 1;
                station.weekday_vehicles += counts.iter().sum::<f64>();
            }
            for (sum, count) in station.hours.iter_mut().zip(counts) {
                *sum += count;
            }
        }
    }
    stations
        .into_iter()
        .map(|(identity, sample)| {
            let mut row = recipe
                .source
                .observation(inputs, identity, CountBasis::Directional)?;
            row.observed_start = sample.days.first().unwrap().to_string();
            row.observed_end = sample.days.last().unwrap().to_string();
            row.time_basis = TimeBasis::Hourly;
            row.daily_factor = 24.0;
            row.total = Some(sample.hours.iter().sum::<f64>() / sample.days.len() as f64 / 24.0);
            row.road_class_mask = 1 << sample.class;
            row.sample = Sample {
                fit_only: true,
                road_class: Some(sample.class),
                environment: sample.environment,
                length_m: None,
                hourly: Some(Hourly {
                    complete_days: u32::try_from(sample.days.len())?,
                    all_vehicles: sample.hours,
                    heavy_vehicles: None,
                    weekday_days: sample.weekday_days,
                    weekday_vehicles: sample.weekday_vehicles,
                }),
                ..Sample::default()
            };
            Ok(row)
        })
        .collect()
}
