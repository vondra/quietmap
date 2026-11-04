//! Normalize CIS railway bitmap calendars and resolved ordered station pairs into annual movements.
use super::station_names::Stations;
use crate::railways::{
    coverage::Coverage,
    movements::{Location, Movement, Stop},
};
use chrono::NaiveDate;
use serde::Deserialize;
use std::{collections::HashMap, fs::File, io::Read, path::Path};

#[derive(Deserialize)]
pub struct NamedStop {
    code: String,
    name: String,
}
pub struct Sequence {
    pub id: String,
    pub stops: Vec<NamedStop>,
    pub passenger: Option<f64>,
    pub freight: Option<f64>,
}

fn descendant<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Result<&'a str, String> {
    node.descendants()
        .find(|node| node.has_tag_name(name))
        .and_then(|node| node.text())
        .ok_or_else(|| format!("CIS timetable lacks {name}"))
}
fn date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value.split('T').next().unwrap_or(value), "%Y-%m-%d")
        .map_err(|error| error.to_string())
}

fn validate_passenger_path(root: roxmltree::Node<'_, '_>) -> Result<(), String> {
    let mut classified = false;
    // A terminal CZPTTLocation may omit the classification carried by preceding locations.
    for traffic in root
        .descendants()
        .filter(|node| node.has_tag_name("TrafficType"))
    {
        let value = traffic.text().unwrap_or("");
        if !["11", "C1", "C2", "C3", "C4"].contains(&value) {
            return Err(format!("unsupported CIS passenger traffic type {value}"));
        }
        classified = true;
    }
    if !classified {
        return Err("CIS path has no traffic classification".into());
    }
    Ok(())
}

pub fn convert(
    archive: &Path,
    stations: &Path,
    start: NaiveDate,
    end: NaiveDate,
    source: u16,
    country: &str,
) -> Result<Vec<Movement>, String> {
    let days = start
        .iter_days()
        .take_while(|day| *day < end)
        .collect::<Vec<_>>();
    let mut covered = vec![false; days.len()];
    let mut sequences = Vec::new();
    let mut archive = zip::ZipArchive::new(File::open(archive).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        if file.is_dir() || !file.name().ends_with(".xml") {
            continue;
        }
        let identity = file.name().to_string();
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        let document = roxmltree::Document::parse(&text).map_err(|error| error.to_string())?;
        let root = document.root_element();
        if !root.has_tag_name("CZPTTCISMessage") {
            return Err(format!(
                "unsupported CIS message {}",
                root.tag_name().name()
            ));
        }
        let calendar = root
            .descendants()
            .find(|node| node.has_tag_name("PlannedCalendar"))
            .ok_or("CIS path has no service calendar")?;
        let first = date(descendant(calendar, "StartDateTime")?)?;
        let last = date(descendant(calendar, "EndDateTime")?)?;
        let bitmap = descendant(calendar, "BitmapDays")?.trim().as_bytes();
        if last < first
            || bitmap.len() != (last - first).num_days() as usize + 1
            || bitmap.iter().any(|byte| !b"01".contains(byte))
        {
            return Err(format!("{identity}: bitmap and validity interval disagree"));
        }
        if first >= end || last < start {
            continue;
        }
        let mut active = 0;
        for (index, day) in days.iter().enumerate() {
            if (first..=last).contains(day) {
                covered[index] = true;
                active += usize::from(bitmap[(*day - first).num_days() as usize] == b'1');
            }
        }
        let mut stops = Vec::new();
        validate_passenger_path(root)?;
        for location in root
            .descendants()
            .filter(|node| node.has_tag_name("CZPTTLocation"))
        {
            stops.push(NamedStop {
                code: descendant(location, "LocationPrimaryCode")?.to_string(),
                name: descendant(location, "PrimaryLocationName")?.to_string(),
            });
        }
        sequences.push(Sequence {
            id: identity,
            stops,
            passenger: Some(active as f64 / days.len() as f64),
            freight: None,
        });
    }
    if !covered.iter().any(|day| *day) {
        return Ok(Vec::new());
    }
    let coverage = Coverage::from_days(&covered)?;
    eprintln!(
        "railways: CIS: {} defined paths, {}/{} covered days",
        sequences.len(),
        covered.iter().filter(|day| **day).count(),
        days.len()
    );
    pairs(
        sequences,
        &Stations::read(stations)?,
        coverage,
        source,
        country,
    )
}

pub fn pairs(
    sequences: Vec<Sequence>,
    stations: &Stations,
    coverage: Coverage,
    source: u16,
    country: &str,
) -> Result<Vec<Movement>, String> {
    let mut resolved = HashMap::<String, Stop>::new();
    for sequence in &sequences {
        for stop in &sequence.stops {
            if !resolved.contains_key(&stop.code)
                && let Some(point) = stations.resolve(&stop.code, &stop.name)
            {
                resolved.insert(stop.code.clone(), point);
            }
        }
    }
    let mut movements = Vec::new();
    for sequence in sequences {
        let mut stops: Vec<_> = sequence
            .stops
            .iter()
            .filter_map(|stop| resolved.get(&stop.code).cloned())
            .collect();
        stops.dedup_by(|a, b| a.id == b.id);
        let locations = if stops.len() < 2 {
            vec![Location::Unlocated {
                reason: "CIS path has fewer than two resolved stations".into(),
            }]
        } else {
            stops
                .windows(2)
                .map(|pair| Location::StationPair {
                    from: pair[0].clone(),
                    to: pair[1].clone(),
                })
                .collect()
        };
        for (index, location) in locations.into_iter().enumerate() {
            let movement = Movement {
                missing_calendar_estimate: [0.0; 2],
                attributes: Default::default(),
                id: format!("{}/{index}", sequence.id),
                source_id: source,
                country: country.to_string(),
                passenger: sequence.passenger,
                freight: sequence.freight,
                passenger_status: if sequence.passenger.is_some() { 2 } else { 0 },
                freight_status: if sequence.freight.is_some() { 2 } else { 0 },
                matching: 2,
                rail_types: vec![0, 3],
                coverage,
                location,
            };
            movement.validate()?;
            movements.push(movement);
        }
    }
    Ok(movements)
}

#[cfg(test)]
pub fn frozen_sequences(
    path: &Path,
    stations: &Path,
    source: u16,
    country: &str,
) -> Result<Vec<Movement>, String> {
    #[derive(Deserialize)]
    struct Raw {
        sequences: Vec<LegacySequence>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacySequence {
        stops: Vec<NamedStop>,
        is_freight: bool,
    }
    let rows: Raw =
        serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let sequences = rows
        .sequences
        .into_iter()
        .enumerate()
        .map(|(index, row)| Sequence {
            id: index.to_string(),
            stops: row.stops,
            passenger: (!row.is_freight).then_some(1.0),
            freight: row.is_freight.then_some(1.0),
        })
        .collect();
    pairs(
        sequences,
        &Stations::read(stations)?,
        Coverage::default(),
        source,
        country,
    )
}

#[cfg(test)]
#[path = "czptt_tests.rs"]
mod tests;
