//! Normalize each operator's GTFS patterns and stops without mixing observation calendars.
use super::{
    gtfs::{OperatorCoverage, Options, Trip, operator_key},
    gtfs_tables::{SourceStop, resolve_stop},
};
use crate::railways::{
    coverage::Coverage,
    movements::{Location, Movement},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

fn coverage_for_operator(
    options: &Options,
    coverages: &OperatorCoverage,
    operator: &str,
) -> Coverage {
    #[cfg(test)]
    if options.frozen_comparison {
        return Coverage::default();
    }
    let _ = options;
    coverages[operator]
}

pub(super) fn normalize(
    directory: &Path,
    options: &Options,
    trips: HashMap<String, Trip>,
    tram_counts: Vec<(String, String, f64)>,
    stops: &BTreeMap<String, SourceStop>,
    coverages: &OperatorCoverage,
) -> Result<Vec<Movement>, String> {
    let required_shapes: HashSet<_> = trips
        .values()
        .filter(|trip| !trip.tram && !trip.shape.is_empty())
        .map(|trip| trip.shape.clone())
        .collect();
    let shapes = super::gtfs_tables::read_shapes(directory, &required_shapes)?;
    let missing_shapes = required_shapes
        .iter()
        .filter(|id| !shapes.contains_key(*id))
        .count();
    if missing_shapes > 0 {
        eprintln!(
            "railways: {}: {missing_shapes} referenced shapes are absent; ordered stations retain unshaped, ambiguity-checked matching",
            options.identity
        );
    }
    let mut ordered = trips
        .into_values()
        .filter(|trip| !trip.tram)
        .collect::<Vec<_>>();
    ordered.sort_by(|a, b| (&a.shape, &a.id).cmp(&(&b.shape, &b.id)));
    let mut movements = Vec::<Movement>::new();
    let mut patterns = HashMap::<String, usize>::new();
    for mut trip in ordered {
        trip.stops.sort_by_key(|stop| stop.0);
        if trip.stops.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(format!("{}: repeated stop_sequence", trip.id));
        }
        let resolved = trip
            .stops
            .iter()
            .map(|(_, id)| resolve_stop(id, stops))
            .collect::<Result<Vec<_>, _>>();
        let mut observation = Movement {
            missing_calendar_estimate: [0.0; 2],
            attributes: Default::default(),
            id: format!("{}/{}", options.identity, trip.id),
            source_id: options.source_id,
            country: options.country.to_string(),
            passenger: Some(trip.departures),
            freight: None,
            passenger_status: 2,
            freight_status: 0,
            matching: 0,
            rail_types: if trip.tram { vec![1, 2] } else { vec![0, 3] },
            coverage: coverage_for_operator(options, coverages, &trip.operator),
            location: Location::Unlocated {
                reason: "fewer than two resolved stations".into(),
            },
        };
        let resolved = match resolved {
            Ok(stops) => stops,
            Err(reason) => {
                observation.location = Location::Unlocated { reason };
                movements.push(observation);
                continue;
            }
        };
        if resolved.len() >= 2 {
            let shape: Vec<_> = if trip.shape.is_empty() {
                Vec::new()
            } else {
                shapes
                    .get(&trip.shape)
                    .map(|shape| shape.iter().map(|point| point.1).collect())
                    .unwrap_or_default()
            };
            let mut key = operator_key(options, &trip.operator).to_owned();
            if !key.is_empty() {
                key.push('\n');
            }
            key.push_str(&trip.direction);
            for stop in &resolved {
                key.push_str(&format!(
                    "\n{}@{:.5},{:.5}",
                    stop.source_id, stop.latitude, stop.longitude
                ));
            }
            key.push('\n');
            for point in &shape {
                key.push_str(&format!("{:.5},{:.5}\n", point[0], point[1]));
            }
            observation.location = Location::Service {
                stops: resolved,
                shape,
            };
            add_pattern(&mut movements, &mut patterns, key, observation);
        } else {
            movements.push(observation);
        }
    }
    for (operator, id, departures) in tram_counts {
        let location = match resolve_stop(&id, stops) {
            Ok(stop) => Location::Station { stop },
            Err(reason) => Location::Unlocated { reason },
        };
        let key = match &location {
            Location::Station { stop } => {
                format!("{operator}tram/{:.4}/{:.4}", stop.latitude, stop.longitude)
            }
            _ => format!("{operator}unlocated-tram/{id}"),
        };
        let movement = Movement {
            missing_calendar_estimate: [0.0; 2],
            attributes: Default::default(),
            id: if operator.is_empty() {
                format!("{}/{}", options.identity, id)
            } else {
                format!("{}/{operator}/{id}", options.identity)
            },
            source_id: options.source_id,
            country: options.country.to_string(),
            passenger: Some(departures),
            freight: None,
            passenger_status: 2,
            freight_status: 0,
            matching: 0,
            rail_types: vec![1, 2],
            coverage: coverage_for_operator(options, coverages, &operator),
            location,
        };
        add_pattern(&mut movements, &mut patterns, key, movement);
    }
    for movement in &movements {
        movement.validate()?;
    }
    Ok(movements)
}

fn add_pattern(
    rows: &mut Vec<Movement>,
    indices: &mut HashMap<String, usize>,
    key: String,
    row: Movement,
) {
    if let Some(&index) = indices.get(&key) {
        *rows[index].passenger.as_mut().unwrap() += row.passenger.unwrap();
    } else {
        indices.insert(key, rows.len());
        rows.push(row);
    }
}
