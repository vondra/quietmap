//! Convert GTFS service calendars, frequencies, parent stops and ordered shapes into rail movements.
use super::calendar::{ServiceCalendar, field, read_rows};
use super::gtfs_tables::{parse_sequence, parse_time, read_stops};
use crate::railways::{coverage::Coverage, movements::Movement};
use chrono::NaiveDate;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

pub struct Options<'a> {
    pub identity: &'a str,
    pub source_id: u16,
    pub country: &'a str,
    pub rail_types: &'a [u16],
    pub tram_types: &'a [u16],
    pub excluded_names: &'a [String],
    pub excluded_agency_names: &'a [String],
    pub ignore_feed_window: bool,
    #[cfg(test)]
    pub frozen_comparison: bool,
}

pub(super) struct Trip {
    pub id: String,
    pub service: String,
    pub operator: String,
    pub direction: String,
    pub shape: String,
    pub tram: bool,
    pub departures: f64,
    pub stops: Vec<(u32, String)>,
}

pub(super) type OperatorCoverage = BTreeMap<String, Coverage>;

pub fn convert(
    directory: &Path,
    options: &Options,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<Movement>, String> {
    let agencies = super::gtfs_tables::read_agencies(directory)?;
    if agencies.is_empty() && !options.excluded_agency_names.is_empty() {
        return Err("agency exclusions require publisher agency identities".into());
    }
    let mut routes = HashMap::new();
    read_rows(&directory.join("routes.txt"), |row| {
        let kind: u16 = field(row, "route_type")
            .parse()
            .map_err(|_| "invalid route_type")?;
        let operator = field(row, "agency_id").to_owned();
        if agencies
            .get(&operator)
            .is_some_and(|name| options.excluded_agency_names.contains(name))
        {
            return Ok(());
        }
        if options
            .excluded_names
            .iter()
            .any(|name| name == field(row, "route_short_name"))
        {
            return Ok(());
        }
        if options.rail_types.contains(&kind) {
            routes.insert(field(row, "route_id").to_string(), (false, operator));
        } else if options.tram_types.contains(&kind) {
            routes.insert(field(row, "route_id").to_string(), (true, operator));
        }
        Ok(())
    })?;
    if routes.is_empty() {
        return Ok(Vec::new());
    }
    let mut trips = HashMap::new();
    let mut services = HashSet::new();
    read_rows(&directory.join("trips.txt"), |row| {
        let Some((tram, operator)) = routes.get(field(row, "route_id")) else {
            return Ok(());
        };
        let service = field(row, "service_id").to_string();
        services.insert(service.clone());
        let id = field(row, "trip_id").to_string();
        if id.is_empty()
            || trips
                .insert(
                    id.clone(),
                    Trip {
                        id,
                        service,
                        operator: operator.clone(),
                        tram: *tram,
                        direction: field(row, "direction_id").to_string(),
                        shape: field(row, "shape_id").to_string(),
                        departures: 1.0,
                        stops: Vec::new(),
                    },
                )
                .is_some()
        {
            return Err("empty or repeated GTFS trip".into());
        }
        Ok(())
    })?;
    let calendar = ServiceCalendar::read_selected(
        directory,
        start,
        end,
        Some(&services),
        options.ignore_feed_window,
    )?;
    let mut operator_services = BTreeMap::<String, HashSet<String>>::new();
    for trip in trips.values() {
        operator_services
            .entry(trip.operator.clone())
            .or_default()
            .insert(trip.service.clone());
    }
    let coverages = operator_services
        .iter()
        .map(|(operator, services)| {
            Ok((
                operator.clone(),
                calendar.coverage_for(services.iter().map(String::as_str))?,
            ))
        })
        .collect::<Result<OperatorCoverage, String>>()?;
    let before = trips.len();
    trips.retain(|_, trip| calendar.active.contains_key(&trip.service));
    if trips.len() != before {
        eprintln!(
            "railways: {}: {} trips have no applicable service calendar in the flow window",
            options.identity,
            before - trips.len()
        );
    }
    for trip in trips.values_mut() {
        trip.departures = calendar.annual_departures(&trip.service, 1.0)?;
    }
    trips.retain(|_, trip| {
        let include_inactive = coverages[&trip.operator].covered_days() > 0;
        #[cfg(test)]
        let include_inactive = include_inactive && !options.frozen_comparison;
        trip.departures > 0.0 || include_inactive
    });
    report_calendars(options, &calendar, &agencies, &coverages, &trips);
    if trips.is_empty() {
        return Ok(Vec::new());
    }
    let frequency_file = directory.join("frequencies.txt");
    if frequency_file.exists() {
        let mut frequencies = HashMap::<String, f64>::new();
        let mut changed_templates = 0;
        let mut added_departures = 0.0;
        read_rows(&frequency_file, |row| {
            let id = field(row, "trip_id");
            if !trips.contains_key(id) {
                return Ok(());
            }
            let first = parse_time(field(row, "start_time"))?;
            let last = parse_time(field(row, "end_time"))?;
            let headway: u32 = field(row, "headway_secs")
                .parse()
                .map_err(|_| "invalid frequency headway")?;
            if last <= first || headway == 0 {
                return Err("nonpositive GTFS frequency interval".into());
            }
            let departures = frequency_departures(first, last, headway);
            if departures != ((last - first) / headway).max(1) {
                changed_templates += 1;
                added_departures += f64::from(departures - ((last - first) / headway).max(1))
                    * trips[id].departures;
            }
            #[cfg(test)]
            let departures = if options.frozen_comparison {
                ((last - first) / headway).max(1)
            } else {
                departures
            };
            *frequencies.entry(id.to_string()).or_default() += f64::from(departures);
            Ok(())
        })?;
        for (id, departures) in frequencies {
            trips.get_mut(&id).unwrap().departures *= departures;
        }
        if changed_templates > 0 {
            eprintln!(
                "railways: {}: {changed_templates} frequency intervals include a final departure; +{added_departures} daily departures over this window",
                options.identity
            );
        }
    }
    let stops = read_stops(directory)?;
    // The large stop_times file is streamed; unselected trips never allocate a row map.
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(directory.join("stop_times.txt"))
        .map_err(|error| error.to_string())?;
    let headers = reader.headers().map_err(|error| error.to_string())?.clone();
    let column = |name| {
        headers
            .iter()
            .position(|value| value.trim_start_matches('\u{feff}').trim() == name)
            .ok_or_else(|| format!("stop_times lacks {name}"))
    };
    let trip_column = column("trip_id")?;
    let stop_column = column("stop_id")?;
    let sequence_column = column("stop_sequence").ok();
    let mut tram_counts = Vec::<(String, String, f64)>::new();
    let mut tram_indices = HashMap::new();
    for row in reader.records() {
        let row = row.map_err(|error| error.to_string())?;
        let Some(trip) = trips.get_mut(row.get(trip_column).unwrap_or("").trim()) else {
            continue;
        };
        let stop = row.get(stop_column).unwrap_or("").trim();
        if trip.tram {
            let operator = operator_key(options, &trip.operator).to_owned();
            let index = *tram_indices
                .entry((operator.clone(), stop.to_string()))
                .or_insert_with(|| {
                    tram_counts.push((operator, stop.to_string(), 0.0));
                    tram_counts.len() - 1
                });
            tram_counts[index].2 += trip.departures;
        } else {
            trip.stops.push((
                parse_sequence(
                    row.get(sequence_column.ok_or("rail stop_times lacks stop_sequence")?)
                        .unwrap_or(""),
                )?,
                stop.to_string(),
            ));
        }
    }
    super::gtfs_movements::normalize(directory, options, trips, tram_counts, &stops, &coverages)
}

fn frequency_departures(first: u32, last: u32, headway: u32) -> u32 {
    (last - first).div_ceil(headway)
}

pub(super) fn operator_key<'a>(options: &Options, operator: &'a str) -> &'a str {
    #[cfg(test)]
    if options.frozen_comparison {
        return "";
    }
    let _ = options;
    operator
}

fn report_calendars(
    options: &Options,
    calendar: &ServiceCalendar,
    agencies: &BTreeMap<String, String>,
    coverages: &OperatorCoverage,
    trips: &HashMap<String, Trip>,
) {
    let mut totals = BTreeMap::<&str, (usize, usize)>::new();
    for trip in trips.values() {
        let count = totals.entry(&trip.operator).or_default();
        count.0 += usize::from(trip.departures > 0.0);
        count.1 += 1;
    }
    for (operator, coverage) in coverages {
        let dates: Vec<_> = calendar
            .days
            .iter()
            .enumerate()
            .filter_map(|(index, day)| coverage.contains(index).then_some(day.to_string()))
            .collect();
        let (active, defined) = totals.get(operator.as_str()).copied().unwrap_or_default();
        eprintln!(
            "railways: operator-calendar {}",
            serde_json::json!({
                "input":options.identity, "operator":operator, "name":agencies.get(operator),
                "days": calendar.days.len(), "covered_days":coverage.covered_days(),
                "first":dates.first(), "last":dates.last(), "active_trips":active, "defined_trips":defined,
            })
        );
    }
}

#[cfg(test)]
#[path = "gtfs_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gtfs_operator_tests.rs"]
mod operator_tests;
