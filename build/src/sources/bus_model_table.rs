//! `qm-build bus-model-table`: the year's bus law fitted on its training places, each with the
//! timetables of one week, their shapes matched to the uncounted road rows the mapped lines serve
//! beside the trip ends the year's buildings make around them; the table and its support.

use super::bus::BusRoutes;
use super::bus_model_fit::{self as fit, CITY_TRIP_ENDS, FitRow, Law};
use super::bus_model_rows::{LEAST_TIMETABLED_BUSES, PlaceRows, place_rows};
use super::bus_shape_match::ShapeIndex;
use super::bus_timetables::{Place, Shape, Week, parse_places, read_week};
use super::road_traffic::bus_service_share;
use crate::dev4::Dev4;
use crate::source_tables::BusModel;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow, ensure};
use chrono::NaiveDate;
use rayon::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The training places and the path of each timetable they name: the model observations the
/// bus-models entry depends on, whose own dependencies are those timetables.
fn inventory(inputs: &YearInputs) -> Result<(Vec<Place>, BTreeMap<String, PathBuf>)> {
    let manifest = &inputs.manifest;
    let declared = |id: &str| manifest.inputs.iter().find(|input| input.id == id);
    let entries: Vec<_> = manifest
        .inputs
        .iter()
        .filter(|input| input.kind == Kind::BusModels)
        .collect();
    ensure!(entries.len() == 1, "bus-models needs one declaration");
    let dependencies: Vec<_> = entries[0]
        .depends_on
        .iter()
        .map(|id| declared(id).ok_or_else(|| anyhow!("bus-models depends on undeclared {id}")))
        .collect::<Result<_>>()?;
    for kind in [
        Kind::ModelObservations,
        Kind::Prepared,
        Kind::RoadTraffic,
        Kind::Bus,
    ] {
        let count = dependencies
            .iter()
            .filter(|input| input.kind == kind)
            .count();
        ensure!(count == 1, "bus-models must depend on one {kind} input");
    }
    let places_entry = dependencies
        .iter()
        .find(|input| input.kind == Kind::ModelObservations)
        .expect("checked above");
    let text =
        std::fs::read_to_string(inputs.by_id(&places_entry.id).map_err(anyhow::Error::msg)?)?;
    let places = parse_places(&text).with_context(|| places_entry.id.clone())?;
    let mut paths = BTreeMap::new();
    for id in places.iter().flat_map(|place| &place.timetables) {
        ensure!(
            places_entry.depends_on.contains(id),
            "{} names timetable {id} it does not depend on",
            places_entry.id
        );
        paths.insert(id.clone(), inputs.by_id(id).map_err(anyhow::Error::msg)?);
    }
    Ok((places, paths))
}

/// Each timetable's week, read once for each first day for every place that names it.
struct Readings {
    keys: Vec<((String, NaiveDate), Vec<usize>)>,
    weeks: Vec<Week>,
}

impl Readings {
    fn read(places: &[Place], paths: &BTreeMap<String, PathBuf>) -> Result<Self> {
        let mut keys = BTreeMap::<(String, NaiveDate), Vec<usize>>::new();
        for (index, place) in places.iter().enumerate() {
            for id in &place.timetables {
                let key = (id.clone(), place.first_day);
                keys.entry(key).or_default().push(index);
            }
        }
        let keys: Vec<_> = keys.into_iter().collect();
        let weeks = keys
            .par_iter()
            .map(|((id, first_day), named)| {
                let named: Vec<&Place> = named.iter().map(|&index| &places[index]).collect();
                read_week(&paths[id], *first_day, &named)
                    .with_context(|| format!("bus timetable {id}, the week from {first_day}"))
            })
            .collect::<Result<Vec<Week>>>()?;
        // A day without any bus lies outside the timetable's service (2026's Prague week ran
        // past its feed's last Saturday): the week would read the place's buses low.
        for (((id, first_day), _), week) in keys.iter().zip(&weeks) {
            if let Some(day) = week.bus_trips_by_day.iter().position(|&trips| trips == 0.0) {
                let date = *first_day + chrono::Days::new(day as u64);
                return Err(anyhow!(
                    "bus timetable {id} runs no bus on {date}, in the week from {first_day}"
                ));
            }
        }
        Ok(Readings { keys, weeks })
    }

    /// A place's shapes and its stop calls by period over all its timetables.
    fn of_place(&self, place: usize) -> (Vec<&Shape>, [f64; 3]) {
        let (mut shapes, mut calls) = (Vec::new(), [0.0; 3]);
        for ((_, named), week) in self.keys.iter().zip(&self.weeks) {
            if let Some(at) = named.iter().position(|&other| other == place) {
                let found = &week.places[at];
                shapes.extend(found.shapes.iter().map(|&shape| &week.shapes[shape]));
                for (sum, period) in calls.iter_mut().zip(found.stop_calls) {
                    *sum += period;
                }
            }
        }
        (shapes, calls)
    }
}

/// `qm-build bus-model-table`: the year's `bus-models` table and its support beside it.
pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "bus model fit needs a new output path");
    let (places, paths) = inventory(inputs)?;
    let routes = BusRoutes::load(inputs.required(Kind::Bus).map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    let traffic = inputs
        .required(Kind::RoadTraffic)
        .map_err(anyhow::Error::msg)?;
    let dev4 = Dev4 {
        prepared: inputs
            .required(Kind::Prepared)
            .map_err(anyhow::Error::msg)?
            .into(),
        rasters: PathBuf::new(),
    };
    let readings = Readings::read(&places, &paths)?;
    // The timetables alone set the buses' period shares and a coach line's trips.
    let calls: Vec<[f64; 3]> = (0..places.len())
        .map(|place| readings.of_place(place).1)
        .collect();
    let coaches: Vec<f64> = readings
        .weeks
        .iter()
        .flat_map(|week| week.coach_departures.iter().copied())
        .collect();
    log_timetables(&places, &readings, &calls);
    let period_shares = fit::period_shares(&calls)?;
    let coach_departures = fit::coach_departures(&coaches)?;
    eprintln!(
        "bus-model-fit: period shares {period_shares:?} of {} places; a coach line direction {coach_departures:.3} a day, the median of {}",
        places.len(),
        coaches.len()
    );
    let measured = places
        .par_iter()
        .enumerate()
        .map(|(index, place)| {
            let shapes = ShapeIndex::new(place.bounds, readings.of_place(index).0);
            place_rows((&dev4, traffic, &routes), (index, place), &shapes)
                .with_context(|| format!("bus model fit: {}", place.name))
        })
        .collect::<Result<Vec<PlaceRows>>>()?;
    let fit_rows: Vec<FitRow> = measured
        .iter()
        .flat_map(|rows| rows.fit.iter().copied())
        .collect();
    let weights = fit::place_weights(&fit_rows);
    let (law, by_knee) = fit::fit_law(&fit_rows, &weights)?;
    let (least_service, below_floor) = fit::least_service(&fit_rows, &weights, &law);
    let mut model = BusModel {
        one_line_direction: law.one_line_direction,
        line_directions_exponent: law.line_directions_exponent,
        trolleybus_line_weight: law.trolleybus_line_weight,
        coach_departures,
        period_shares,
        city_trip_ends: CITY_TRIP_ENDS,
        service_exponent: law.service_exponent,
        most_trip_ends: law.most_trip_ends,
        least_service,
        service_by_built_up: [0.0; 3],
    };
    let shares: Vec<(u8, f64, f64)> = measured
        .iter()
        .flat_map(|rows| &rows.bus_rows)
        .map(|&(flag, trip_ends, length)| (flag, bus_service_share(&model, trip_ends), length))
        .collect();
    model.service_by_built_up = fit::service_by_built_up(&shares)?;
    let table = json!({"rows": [table_row(&model)]});
    std::fs::write(output, serde_json::to_vec_pretty(&table)?)?;
    let support = support(
        &places,
        (&readings, &calls, &measured),
        (&law, &by_knee, least_service, below_floor),
    );
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&support)?,
    )?;
    eprintln!("bus-model-fit: {} rows: {table}", fit_rows.len());
    Ok(())
}

/// Each timetable week's trips a day and each place's stop calls, before anything is fitted.
fn log_timetables(places: &[Place], readings: &Readings, calls: &[[f64; 3]]) {
    for (((id, first_day), _), week) in readings.keys.iter().zip(&readings.weeks) {
        eprintln!(
            "bus-model-fit: {id} from {first_day}: bus trips a day {:?}, {} shapes near its places, {} coach route directions",
            week.bus_trips_by_day,
            week.shapes.len(),
            week.coach_departures.len()
        );
    }
    for (place, calls) in places.iter().zip(calls) {
        let total = calls.iter().sum::<f64>().max(1.0);
        eprintln!(
            "bus-model-fit: {}: {total:.0} bus stop calls a week, by day, evening and night {:.3?}",
            place.name,
            calls.map(|period| period / total)
        );
    }
}

fn table_row(model: &BusModel) -> serde_json::Value {
    json!({
        "one_line_direction": model.one_line_direction,
        "line_directions_exponent": model.line_directions_exponent,
        "trolleybus_line_weight": model.trolleybus_line_weight,
        "coach_departures": model.coach_departures,
        "period_shares": model.period_shares,
        "city_trip_ends": model.city_trip_ends,
        "service_exponent": model.service_exponent,
        "most_trip_ends": model.most_trip_ends,
        "least_service": model.least_service,
        "service_by_built_up": model.service_by_built_up,
    })
}

/// The fit's support: each place's rows, stop calls and mean level against its timetable, each
/// timetable week's trips, and the best law of every bound on the trip ends.
fn support(
    places: &[Place],
    (readings, calls, measured): (&Readings, &[[f64; 3]], &[PlaceRows]),
    (law, by_knee, least_service, below_floor): (&Law, &[Law], f64, usize),
) -> serde_json::Value {
    let modelled =
        |row: &FitRow| law.line_part(row.lines) * law.share(row.trip_ends).max(least_service);
    let places: Vec<_> = places
        .iter()
        .zip(measured)
        .zip(calls)
        .map(|((place, rows), calls)| {
            let (decibels, length) = rows.fit.iter().fold((0.0, 0.0), |(sum, total), row| {
                let db = 10.0 * (modelled(row) / row.buses).log10();
                (sum + row.length_m * db, total + row.length_m)
            });
            let [not_timetabled, timetabled] = rows.line_km;
            let trolleybus_km: f64 = rows
                .fit
                .iter()
                .filter(|row| row.lines[1] > 0.0)
                .map(|row| row.length_m / 1000.0)
                .sum();
            json!({
                "place": place.name, "first_day": place.first_day.to_string(),
                "timetables": place.timetables, "squares": rows.squares,
                "squares_without_mapped_buildings": rows.squares_without_mapped_buildings,
                "fitted_rows": rows.fit.len(), "fitted_km": length / 1000.0,
                "fitted_trolleybus_km": trolleybus_km,
                "bus_rows": rows.bus_rows.len(),
                "line_direction_km": {"timetabled": timetabled, "not_timetabled": not_timetabled},
                "mean_db_law_over_timetable": if length > 0.0 { decibels / length } else { 0.0 },
                "stop_calls": calls,
            })
        })
        .collect();
    let weeks: Vec<_> = readings
        .keys
        .iter()
        .zip(&readings.weeks)
        .map(|(((id, first_day), _), week)| {
            json!({
                "timetable": id, "first_day": first_day.to_string(),
                "bus_trips_by_day": week.bus_trips_by_day,
                "bus_trips_without_shape": week.bus_trips_without_shape,
                "shapes_near_places": week.shapes.len(),
                "coach_route_directions": week.coach_departures,
            })
        })
        .collect();
    let scan: Vec<_> = by_knee
        .iter()
        .map(|law| {
            json!({
                "most_trip_ends": law.most_trip_ends,
                "trolleybus_line_weight": law.trolleybus_line_weight,
                "squared_error": law.squared_error, "one_line_direction": law.one_line_direction,
                "line_directions_exponent": law.line_directions_exponent,
                "service_exponent": law.service_exponent,
            })
        })
        .collect();
    json!({
        "places": places, "timetable_weeks": weeks, "scan_by_most_trip_ends": scan,
        "rows_below_least_service": below_floor,
        "least_timetabled_buses": LEAST_TIMETABLED_BUSES,
    })
}
