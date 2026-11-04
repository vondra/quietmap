//! Normalize Baden-Württemberg complete counter-days and separate heavy traffic from its hourly totals.

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
    collections::BTreeMap,
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    pub archives: Vec<Members>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Members {
    pub nested_archive: Option<String>,
    pub members: Vec<String>,
}

#[derive(Default)]
struct Day {
    all: [f64; 24],
    heavy: [f64; 24],
    mask: u32,
    duplicate: bool,
}
struct Station {
    class: u8,
    days: BTreeMap<NaiveDate, Day>,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    let mut outer = zip::ZipArchive::new(File::open(path)?)?;
    let mut stations = BTreeMap::<String, Station>::new();
    let (first, last) = recipe
        .source
        .input(inputs)?
        .observed
        .bounds()
        .map_err(anyhow::Error::msg)?;
    for group in &recipe.archives {
        ensure!(!group.members.is_empty(), "no declared hourly members");
        let mut bytes = Vec::new();
        if let Some(nested) = &group.nested_archive {
            outer.by_name(nested)?.read_to_end(&mut bytes)?;
        }
        let mut nested = (!bytes.is_empty())
            .then(|| zip::ZipArchive::new(Cursor::new(bytes)))
            .transpose()?;
        for member in &group.members {
            let mut bytes = Vec::new();
            if let Some(archive) = &mut nested {
                archive.by_name(member)?.read_to_end(&mut bytes)?;
            } else {
                outer.by_name(member)?.read_to_end(&mut bytes)?;
            }
            parse(&bytes, first, last, &mut stations)?;
        }
    }
    let mut rows = Vec::new();
    for (identity, station) in stations {
        let complete = station
            .days
            .into_iter()
            .filter(|(_, d)| !d.duplicate && d.mask == (1 << 24) - 1)
            .collect::<Vec<_>>();
        if complete.is_empty() {
            continue;
        }
        let mut all = [0.0; 24];
        let mut heavy = [0.0; 24];
        for (_, day) in &complete {
            for h in 0..24 {
                all[h] += day.all[h];
                heavy[h] += day.heavy[h];
            }
        }
        if all.iter().sum::<f64>() == 0.0 {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, identity, CountBasis::BothDirections)?;
        row.observed_start = complete.first().unwrap().0.to_string();
        row.observed_end = complete.last().unwrap().0.to_string();
        row.time_basis = TimeBasis::Hourly;
        row.daily_factor = 24.0;
        row.total = Some(all.iter().sum::<f64>() / complete.len() as f64 / 24.0);
        row.road_class_mask = 1 << station.class;
        row.sample = Sample {
            fit_only: true,
            road_class: Some(station.class),
            environment: 0,
            length_m: None,
            hourly: Some(Hourly {
                complete_days: u32::try_from(complete.len())?,
                all_vehicles: all,
                heavy_vehicles: Some(heavy),
                weekday_days: 0,
                weekday_vehicles: 0.0,
            }),
        };
        rows.push(row);
    }
    Ok(rows)
}

fn parse(
    bytes: &[u8],
    first: NaiveDate,
    last: NaiveDate,
    stations: &mut BTreeMap<String, Station>,
) -> Result<()> {
    let text = encoding_rs::WINDOWS_1252.decode(bytes).0;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("missing BW station header"))?
        .split(';')
        .collect::<Vec<_>>();
    ensure!(header.len() >= 6, "short BW station header");
    let class = match header[3].trim() {
        "A" => 0,
        "B" => 2,
        "L" => 3,
        "K" => 4,
        _ => return Ok(()),
    };
    let identity = format!("{}:{}", header[0].trim(), header[1].trim());
    lines.next();
    lines.next();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("missing BW columns"))?
        .split(';')
        .collect::<Vec<_>>();
    let Some(indices) = ["KFZFS1", "KFZFS2", "SVFS1", "SVFS2"]
        .map(|key| header.iter().position(|v| *v == key))
        .into_iter()
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(());
    };
    let station = stations.entry(identity).or_insert(Station {
        class,
        days: BTreeMap::new(),
    });
    ensure!(station.class == class, "BW station road class changed");
    for line in lines {
        let fields = line.split(';').collect::<Vec<_>>();
        if fields.len() <= *indices.iter().max().unwrap() {
            continue;
        }
        let Some(short) = fields[0].parse::<u32>().ok() else {
            continue;
        };
        let year = first.year() / 100 * 100 + (short / 10000) as i32;
        let Some(date) = NaiveDate::from_ymd_opt(year, short / 100 % 100, short % 100) else {
            continue;
        };
        if date < first || date > last {
            continue;
        }
        let Some(hour) = fields[1]
            .split(':')
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|h| (1..=24).contains(h))
        else {
            continue;
        };
        let Some(counts) = indices
            .iter()
            .map(|&i| {
                fields[i]
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let all = counts[0] + counts[1];
        let heavy = counts[2] + counts[3];
        if heavy > all {
            continue;
        }
        let day = station.days.entry(date).or_default();
        day.duplicate |= day.mask & (1 << (hour - 1)) != 0;
        day.mask |= 1 << (hour - 1);
        day.all[hour - 1] = all;
        day.heavy[hour - 1] = heavy;
    }
    Ok(())
}
