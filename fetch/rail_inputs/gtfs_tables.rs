//! Parse publisher CSV values and resolve original GTFS stop and shape identities.
use super::calendar::{field, read_rows};
use crate::railways::movements::Stop;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};
pub(super) fn parse_time(value: &str) -> Result<u32, String> {
    let parts: Vec<_> = value
        .split(':')
        .map(str::parse::<u32>)
        .collect::<Result<_, _>>()
        .map_err(|_| format!("invalid GTFS time {value}"))?;
    if parts.len() != 3 || parts[1] >= 60 || parts[2] >= 60 {
        return Err(format!("invalid GTFS time {value}"));
    }
    parts[0]
        .checked_mul(3600)
        .and_then(|seconds| seconds.checked_add(parts[1] * 60 + parts[2]))
        .ok_or_else(|| "GTFS time overflow".to_string())
}

pub(super) fn parse_sequence(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("invalid GTFS sequence {value}"))
}

pub(super) struct SourceStop {
    parent: String,
    coordinates: Option<[f64; 2]>,
}
pub(super) fn read_stops(directory: &Path) -> Result<BTreeMap<String, SourceStop>, String> {
    let mut stops = BTreeMap::new();
    read_rows(&directory.join("stops.txt"), |row| {
        let coordinates = field(row, "stop_lat")
            .parse::<f64>()
            .ok()
            .zip(field(row, "stop_lon").parse::<f64>().ok())
            .filter(|(lat, lon)| (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon))
            .map(|(lat, lon)| [lat, lon]);
        stops.insert(
            field(row, "stop_id").to_string(),
            SourceStop {
                parent: field(row, "parent_station").to_string(),
                coordinates,
            },
        );
        Ok(())
    })?;
    Ok(stops)
}
pub(super) fn resolve_stop(id: &str, stops: &BTreeMap<String, SourceStop>) -> Result<Stop, String> {
    let mut current = id;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let stop = stops
            .get(current)
            .ok_or_else(|| format!("unknown GTFS stop {current}"))?;
        if let Some([latitude, longitude]) = stop.coordinates {
            return Ok(Stop {
                source_id: id.to_string(),
                id: current.to_string(),
                latitude,
                longitude,
            });
        }
        current = &stop.parent;
    }
    Err(format!("cyclic GTFS parent stop {id}"))
}

pub(super) type Shapes = HashMap<String, Vec<(u32, [f64; 2])>>;
pub(super) fn read_shapes(
    directory: &Path,
    required_shapes: &HashSet<String>,
) -> Result<Shapes, String> {
    let mut shapes = HashMap::<String, Vec<(u32, [f64; 2])>>::new();
    if !required_shapes.is_empty() && directory.join("shapes.txt").exists() {
        read_rows(&directory.join("shapes.txt"), |row| {
            let id = field(row, "shape_id");
            if !required_shapes.contains(id) {
                return Ok(());
            }
            let point = [
                field(row, "shape_pt_lat")
                    .parse()
                    .map_err(|_| "invalid shape latitude")?,
                field(row, "shape_pt_lon")
                    .parse()
                    .map_err(|_| "invalid shape longitude")?,
            ];
            shapes
                .entry(id.to_string())
                .or_default()
                .push((parse_sequence(field(row, "shape_pt_sequence"))?, point));
            Ok(())
        })?;
    }
    for shape in shapes.values_mut() {
        shape.sort_by_key(|point| point.0);
    }
    for (id, vertices) in &shapes {
        // Some publisher exports repeat identical vertices; only conflicting order is ambiguous.
        if vertices
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1)
        {
            return Err(format!("conflicting repeated sequence in GTFS shape {id}"));
        }
    }
    Ok(shapes)
}

pub(super) fn read_agencies(directory: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut agencies = BTreeMap::new();
    let path = directory.join("agency.txt");
    if path.exists() {
        read_rows(&path, |row| {
            agencies.insert(
                field(row, "agency_id").to_owned(),
                field(row, "agency_name").to_owned(),
            );
            Ok(())
        })?;
    }
    Ok(agencies)
}
