//! A training place's uncounted road rows served by mapped bus lines, where its buildings are
//! mapped: their trip ends within 2 km, and the buses its timetables run along them.

use super::bus::BusRoutes;
use super::bus_model_fit::FitRow;
use super::bus_shape_match::ShapeIndex;
use super::bus_timetables::Place;
use super::road_traffic::{BuildingTraffic, local_km};
use crate::dev4::{Dev4, Square, column, degrees_to_z30, require_stamp, z30_corner_degrees};
use anyhow::Result;
use arrow_array::{BooleanArray, Float32Array, Int32Array, Int64Array, UInt8Array, UInt16Array};
use std::path::Path;

/// A row runs timetabled buses above this many a day (one trip in a fortnight is noise).
pub(super) const LEAST_TIMETABLED_BUSES: f64 = 0.5;

/// The z9 squares a place's bounds reach into.
fn squares([south, west, north, east]: [f64; 4]) -> Vec<Square> {
    let corner = |lat, lon| {
        let (x, y) = degrees_to_z30(lat, lon);
        (x.rem_euclid(1 << 30) >> 21, (511 - (y >> 21)).clamp(0, 511))
    };
    let ((west_x, south_y), (east_x, north_y)) = (corner(south, west), corner(north, east));
    (west_x..=east_x)
        .flat_map(|x| {
            (north_y..=south_y).map(move |y| Square {
                x: x as u32,
                y: y as u32,
            })
        })
        .collect()
}

/// A place's rows: those the law is fitted on, every bus row's built-up flag, trip ends and length
/// where the buildings are mapped, and the line-direction-km the timetable did and did not match.
#[derive(Default)]
pub(super) struct PlaceRows {
    pub fit: Vec<FitRow>,
    pub bus_rows: Vec<(u8, f64, f64)>,
    pub squares: usize,
    pub squares_without_mapped_buildings: usize,
    /// Not timetabled, timetabled.
    pub line_km: [f64; 2],
}

/// The rows of `place` (the `index`-th) in the prepared squares its bounds reach into, its
/// shapes matched to them.
pub(super) fn place_rows(
    (dev4, traffic, routes): (&Dev4, &Path, &BusRoutes),
    (index, place): (usize, &Place),
    shapes: &ShapeIndex,
) -> Result<PlaceRows> {
    let mut rows = PlaceRows::default();
    for square in squares(place.bounds) {
        let Some(table) = dev4
            .table(square, "roads.arrow")
            .map_err(anyhow::Error::msg)?
        else {
            continue;
        };
        rows.squares += 1;
        require_stamp(&table, "grid", "z30").map_err(anyhow::Error::msg)?;
        let mut lengths = Vec::new();
        for batch in &table.batches {
            let class = column::<UInt8Array>(batch, "road_class").map_err(anyhow::Error::msg)?;
            let length = column::<Float32Array>(batch, "length_m").map_err(anyhow::Error::msg)?;
            lengths.extend(
                (0..batch.num_rows()).map(|row| (class.value(row), f64::from(length.value(row)))),
            );
        }
        let Some(buildings) = BuildingTraffic::load(traffic, square, local_km(lengths.into_iter()))
            .map_err(anyhow::Error::msg)?
        else {
            rows.squares_without_mapped_buildings += 1;
            continue;
        };
        for batch in &table.batches {
            let read = |name| column::<Int32Array>(batch, name).map_err(anyhow::Error::msg);
            let (start_x, start_y, end_x, end_y) = (
                read("start_gx")?,
                read("start_gy")?,
                read("end_gx")?,
                read("end_gy")?,
            );
            let bytes = |name| column::<UInt8Array>(batch, name).map_err(anyhow::Error::msg);
            let (oneway, built_up, estimated) = (
                bytes("oneway")?,
                bytes("built_up")?,
                bytes("traffic_estimated")?,
            );
            let source = column::<UInt16Array>(batch, "source_id").map_err(anyhow::Error::msg)?;
            let tunnel = column::<BooleanArray>(batch, "tunnel").map_err(anyhow::Error::msg)?;
            let way = column::<Int64Array>(batch, "osm_id").map_err(anyhow::Error::msg)?;
            let length = column::<Float32Array>(batch, "length_m").map_err(anyhow::Error::msg)?;
            for row in 0..batch.num_rows() {
                if !super::road::guessed(source.value(row), estimated.value(row))
                    || tunnel.value(row)
                {
                    continue;
                }
                let Some(([bus, trolleybus, _], departures)) = routes.lines(way.value(row)) else {
                    continue;
                };
                let (start, end) = (
                    z30_corner_degrees(start_x.value(row), start_y.value(row)),
                    z30_corner_degrees(end_x.value(row), end_y.value(row)),
                );
                if !place.contains(0.5 * (start.0 + end.0), 0.5 * (start.1 + end.1)) {
                    continue;
                }
                let middle = (
                    ((i64::from(start_x.value(row)) + i64::from(end_x.value(row))) / 2) as i32,
                    ((i64::from(start_y.value(row)) + i64::from(end_y.value(row))) / 2) as i32,
                );
                let (trip_ends, length_m) = (
                    buildings.bus_trip_ends(middle).max(1.0),
                    f64::from(length.value(row)),
                );
                rows.bus_rows
                    .push((built_up.value(row), trip_ends, length_m));
                let lines = [f64::from(bus), f64::from(trolleybus)];
                // A tagged interval runs its own departures, which the law does not predict.
                if departures != [0.0; 2] || lines[0] + lines[1] < 1.0 {
                    continue;
                }
                let Some(buses) =
                    shapes.buses_along([start.0, start.1], [end.0, end.1], oneway.value(row))
                else {
                    continue;
                };
                let timetabled = buses > LEAST_TIMETABLED_BUSES;
                rows.line_km[usize::from(timetabled)] += length_m * (lines[0] + lines[1]) / 1000.0;
                if timetabled {
                    rows.fit.push(FitRow {
                        place: index,
                        buses,
                        lines,
                        trip_ends,
                        length_m,
                    });
                }
            }
        }
    }
    Ok(rows)
}
