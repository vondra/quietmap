//! A training place's timetables over one week (GTFS): each bus trip's shape with its trips, the
//! bus stop calls inside the place by period, and the coach routes calling there.

use super::bus_gtfs::{Tables, field, number, read_shapes, read_trips};
use crate::rail_inputs::gtfs_tables::parse_time;
use anyhow::{Context, Result, anyhow, ensure};
use chrono::NaiveDate;
use csv::ByteRecord;
use std::collections::HashMap;
use std::path::Path;

/// A shape is kept for a place whose bounds it comes within this many degrees of (`measure.py`).
const NEAR_DEGREES: f64 = 0.01;

/// A training place of the inventory: its bounds, the first of its seven days and the manifest
/// ids of its timetables.
#[derive(Debug, PartialEq)]
pub(super) struct Place {
    pub name: String,
    /// South, west, north, east, in degrees.
    pub bounds: [f64; 4],
    pub first_day: NaiveDate,
    pub timetables: Vec<String>,
}

impl Place {
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        self.within(lat, lon, 0.0)
    }

    fn within(&self, lat: f64, lon: f64, margin: f64) -> bool {
        let [south, west, north, east] = self.bounds;
        (south - margin..=north + margin).contains(&lat)
            && (west - margin..=east + margin).contains(&lon)
    }
}

/// The inventory: a header line, then `place south west north east first_day timetables` per
/// place, tab-separated, the timetables comma-separated manifest ids.
pub(super) fn parse_places(text: &str) -> Result<Vec<Place>> {
    let mut lines = text.lines().enumerate();
    let header = lines.next().map(|(_, line)| line);
    ensure!(
        header == Some("place\tsouth\twest\tnorth\teast\tfirst_day\ttimetables"),
        "bus timetable places: unexpected header {header:?}"
    );
    let places = lines
        .map(|(number, line)| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [name, south, west, north, east, first_day, timetables] = fields[..] else {
                return Err(anyhow!("line {}: expected seven fields", number + 1));
            };
            let degrees = |text: &str| -> Result<f64> {
                text.parse()
                    .with_context(|| format!("line {}: {text}", number + 1))
            };
            let bounds = [
                degrees(south)?,
                degrees(west)?,
                degrees(north)?,
                degrees(east)?,
            ];
            ensure!(
                bounds[0] < bounds[2] && bounds[1] < bounds[3],
                "line {}: empty bounds",
                number + 1
            );
            Ok(Place {
                name: name.to_string(),
                bounds,
                first_day: NaiveDate::parse_from_str(first_day, "%Y-%m-%d")
                    .with_context(|| format!("line {}: {first_day}", number + 1))?,
                timetables: timetables.split(',').map(str::to_string).collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!places.is_empty(), "bus timetable places: none listed");
    Ok(places)
}

/// A bus shape that runs in the week near a place: its points (latitude, longitude) in order and
/// its trips over the seven days.
pub(super) struct Shape {
    pub points: Vec<[f64; 2]>,
    pub trips: f64,
}

/// What one timetable runs over one week for the places that name it.
pub(super) struct Week {
    /// The bus shapes near any of the places.
    pub shapes: Vec<Shape>,
    /// Per place, in the order given.
    pub places: Vec<PlaceWeek>,
    /// Per coach route direction calling at a stop inside any of the places: its trips a day.
    pub coach_departures: Vec<f64>,
    pub bus_trips_by_day: [f64; 7],
    pub bus_trips_without_shape: f64,
}

/// A place's bus stop calls by day, evening and night (each weighted by its trip's days in the
/// week) and the shapes near it.
pub(super) struct PlaceWeek {
    pub stop_calls: [f64; 3],
    pub shapes: Vec<usize>,
}

/// The week from `first_day` of the timetable at `path` for `places`.
pub(super) fn read_week(path: &Path, first_day: NaiveDate, places: &[&Place]) -> Result<Week> {
    let tables = Tables::open(path);
    let calendar = tables.calendar(first_day)?;
    let trips = read_trips(&tables, &calendar)?;
    let mut week = Week {
        shapes: Vec::new(),
        places: places
            .iter()
            .map(|_| PlaceWeek {
                stop_calls: [0.0; 3],
                shapes: Vec::new(),
            })
            .collect(),
        coach_departures: Vec::new(),
        bus_trips_by_day: trips.by_day,
        bus_trips_without_shape: trips.without_shape,
    };
    for (id, points) in read_shapes(&tables, &trips.by_shape)? {
        let near: Vec<usize> = (0..places.len())
            .filter(|&place| {
                points
                    .iter()
                    .any(|&[lat, lon]| places[place].within(lat, lon, NEAR_DEGREES))
            })
            .collect();
        for &place in &near {
            week.places[place].shapes.push(week.shapes.len());
        }
        if !near.is_empty() {
            week.shapes.push(Shape {
                points,
                trips: trips.by_shape[&id],
            });
        }
    }
    let mut stop_places = HashMap::<Vec<u8>, Vec<usize>>::new();
    let found = &mut stop_places;
    tables.rows("stops.txt", |columns| {
        let (stop, lat, lon) = (
            columns.index("stop_id")?,
            columns.index("stop_lat")?,
            columns.index("stop_lon")?,
        );
        Ok(move |row: &ByteRecord| {
            let (Some(lat), Some(lon)) = (number::<f64>(row, lat), number::<f64>(row, lon)) else {
                return Ok(());
            };
            let inside: Vec<usize> = (0..places.len())
                .filter(|&place| places[place].contains(lat, lon))
                .collect();
            if !inside.is_empty() {
                found.insert(field(row, Some(stop)).to_vec(), inside);
            }
            Ok(())
        })
    })?;
    // Days of the coach trips calling inside a place, by route direction; counted once a trip.
    let mut coach_days = vec![0u32; trips.coach_directions];
    let mut called = std::collections::HashSet::<Vec<u8>>::new();
    let (stops, running) = (&stop_places, &trips.trips);
    let (called_once, coach_trip_days, calls) = (&mut called, &mut coach_days, &mut week.places);
    tables.rows("stop_times.txt", |columns| {
        let (trip_column, stop) = (columns.index("trip_id")?, columns.index("stop_id")?);
        let (departure, arrival) = (
            columns.optional("departure_time"),
            columns.optional("arrival_time"),
        );
        Ok(move |row: &ByteRecord| {
            let Some(inside) = stops.get(field(row, Some(stop))) else {
                return Ok(());
            };
            let id = field(row, Some(trip_column));
            let Some(trip) = running.get(id) else {
                return Ok(());
            };
            if let Some(direction) = trip
                .coach_direction
                .filter(|_| called_once.insert(id.to_vec()))
            {
                coach_trip_days[direction] += u32::from(trip.days);
            }
            let time = match field(row, departure) {
                b"" => field(row, arrival),
                time => time,
            };
            let Some(seconds) = std::str::from_utf8(time)
                .ok()
                .and_then(|t| parse_time(t).ok())
            else {
                return Ok(());
            };
            let period = usize::from(crate::period::period_of_hour(seconds / 3600 % 24));
            for &place in inside {
                calls[place].stop_calls[period] += f64::from(trip.days);
            }
            Ok(())
        })
    })?;
    week.coach_departures = coach_days
        .into_iter()
        .filter(|&days| days > 0)
        .map(|days| f64::from(days) / 7.0)
        .collect();
    Ok(week)
}

#[cfg(test)]
#[path = "bus_timetables_tests.rs"]
mod tests;
