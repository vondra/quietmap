//! Collect popup sources through layer readers, spatial ownership and aircraft validation.

mod aircraft;
mod point_sources;
mod railways;
mod roads;
mod settlement;
mod spatial;
#[cfg(test)]
mod tests;
#[cfg(feature = "node")]
mod trace_limit;

pub use railways::{query_railways_from_batches, RailResult};
pub use roads::{query_roads_from_batches, RoadResult};
pub use settlement::{
    query_buildings_from_batches, query_leisure_from_batches, BuildingResult, LeisureResult,
};
pub use spatial::{
    square_dir, squares_within_radius, squares_within_reach, surface_squares_within_reach,
};
#[cfg(feature = "node")]
pub(crate) use trace_limit::apply_segment_top_k_with_cap;

use aircraft::{
    read_aircraft_batches_of_square_data, read_aircraft_stamps_of_square_data,
    AircraftPointQueryData,
};
use spatial::{BUILDING_QUERY_RADIUS_M, SHIP_QUERY_RADIUS_M};
use square_store::store::{load_square, SquareData};
use std::path::Path;

#[derive(Debug)]
pub struct PointQueryData {
    pub roads: Vec<noise_compute::types::RoadSegment>,
    pub railways: Vec<noise_compute::types::RailSegment>,
    pub buildings: Vec<noise_compute::types::PointSource>,
    pub industrial: Vec<noise_compute::types::PointSource>,
    /// Ship traffic cells within `SHIP_QUERY_RADIUS_M`.
    pub ships: Vec<noise_compute::types::PointSource>,
    /// v6 aircraft popup arrows. Rows are consumed via typed views in
    /// `compute_aircraft_v6` — no AircraftSegment synthesis happens here.
    pub aircraft_airborne_batches: Vec<arrow::record_batch::RecordBatch>,
    pub aircraft_cruise_batches: Vec<arrow::record_batch::RecordBatch>,
    /// `airport_traffic.arrow` per-microsegment sparse counters.
    pub aircraft_airport_traffic_batches: Vec<arrow::record_batch::RecordBatch>,
    pub airport_summary: crate::aircraft_v6::airport_summary_view::AirportSummaryAccum,
    /// `airport_lines.arrow` OSM ids and refs used to label runway and
    /// taxiway segment traces.
    pub airport_lines_batches: Vec<arrow::record_batch::RecordBatch>,
    pub aircraft_sampling_window: Option<noise_compute::emission::aircraft::SamplingWindow>,
    /// Emission layers dropped from the whole query because a file of theirs carried another
    /// contract; the answer lacks their noise and says so.
    pub unavailable_layers: Vec<&'static str>,
}

pub fn collect_sources_at_point(
    prepared_year_dir: &Path,
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let squares = squares_within_reach(lat, lng)?;
    let loaded: Vec<SquareData> = squares
        .iter()
        .map(|sq| load_square(&square_dir(prepared_year_dir, *sq)))
        .collect::<Result<_, _>>()?;
    let refs: Vec<_> = squares.into_iter().zip(&loaded).collect();

    collect_from_square_data(&refs, lat, lng)
}

pub fn collect_from_square_data(
    square_data: &[(grid::Square, &SquareData)],
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let mut all_roads = Vec::new();
    let mut all_railways = Vec::new();
    let mut all_buildings = Vec::new();
    let mut all_industrial = Vec::new();
    let mut all_ships = Vec::new();
    // A layer is one fact of the answer: a file of it dropped in any owner square, or any
    // aircraft stamp the reader does not know, drops it in every square, so a layer named in
    // `unavailable_layers` never also contributes sources.
    let mut unavailable_layers: Vec<&'static str> = square_data
        .iter()
        .flat_map(|(_, data)| data.unavailable_layers.iter().copied())
        .collect();
    let mut aircraft = AircraftPointQueryData::none();
    if !unavailable_layers.contains(&"aircraft") {
        aircraft = read_aircraft_batches_of_square_data(square_data, lat, lng)?;
        if let Err(fault) =
            read_aircraft_stamps_of_square_data(square_data, lat, lng, &mut aircraft)
        {
            square_store::warn_once::warn_once(
                &format!("{fault}; serving without the aircraft layer"),
                &format!("first seen at ({lat:.5}, {lng:.5})"),
            );
            unavailable_layers.push("aircraft");
            aircraft = AircraftPointQueryData::none();
        }
    }
    unavailable_layers.sort_unstable();
    unavailable_layers.dedup();
    let served = |layer: &str| !unavailable_layers.contains(&layer);

    for (_, data) in square_data {
        railways::collect_railways(data, lat, lng, &mut all_railways)?;
        roads::collect_roads(data, lat, lng, &mut all_roads)?;
        settlement::collect_buildings(data, lat, lng, &mut all_buildings)?;
        if served("leisure") {
            let batches = data
                .leisure
                .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M)?;
            settlement::collect_leisure(&batches, lat, lng, &mut all_buildings);
        }
        point_sources::collect_industrial(data, lat, lng, &mut all_industrial)?;
        if served("ships") {
            let batches = data.ships.batches_within(lat, lng, SHIP_QUERY_RADIUS_M)?;
            point_sources::collect_ships(&batches, lat, lng, &mut all_ships)?;
        }
    }

    Ok(PointQueryData {
        roads: all_roads,
        railways: all_railways,
        buildings: all_buildings,
        industrial: all_industrial,
        ships: all_ships,
        aircraft_airborne_batches: aircraft.airborne_batches,
        aircraft_cruise_batches: aircraft.cruise_batches,
        aircraft_airport_traffic_batches: aircraft.airport_traffic_batches,
        airport_summary: aircraft.airport_summary,
        airport_lines_batches: aircraft.airport_lines_batches,
        aircraft_sampling_window: aircraft.sampling_window,
        unavailable_layers,
    })
}
