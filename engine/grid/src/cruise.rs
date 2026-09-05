//! Shared z15 aircraft aggregation geometry for the producer and popup highlight.

use crate::{poly::meters_to_lonlat, Square, EARTH_CIRCUMFERENCE_M};

/// At the equator a z15 cell is 1,223 m wide; its 865 m corner radius
/// is smaller than the previous cruise aggregation's 1.22 km radius.
const CRUISE_ZOOM: u32 = 15;
pub const CRUISE_AXIS: u32 = 1 << CRUISE_ZOOM;

pub fn cell_axes(lat: f64, lon: f64) -> (u32, u32) {
    let lon = crate::geo::normalize_longitude(lon);
    let x = ((lon + 180.0) / 360.0 * f64::from(CRUISE_AXIS)).floor();
    let (_, northing) = crate::lonlat_to_meters(0.0, lat);
    let y = ((0.5 - northing / EARTH_CIRCUMFERENCE_M) * f64::from(CRUISE_AXIS)).floor();
    (
        x.clamp(0.0, f64::from(CRUISE_AXIS - 1)) as u32,
        y.clamp(0.0, f64::from(CRUISE_AXIS - 1)) as u32,
    )
}

pub fn cruise_cell_id(lat: f64, lon: f64) -> u64 {
    let (x, y) = cell_axes(lat, lon);
    (u64::from(x) << CRUISE_ZOOM) | u64::from(y)
}

fn cruise_axes(id: u64) -> (u32, u32) {
    (
        (id >> CRUISE_ZOOM) as u32,
        (id & u64::from(CRUISE_AXIS - 1)) as u32,
    )
}

pub fn cruise_parent(id: u64) -> u64 {
    let (x, y) = cruise_axes(id);
    crate::square_id(Square {
        x: (x >> (CRUISE_ZOOM - 9)) as u16,
        y: (y >> (CRUISE_ZOOM - 9)) as u16,
    }) as u64
}

pub fn cruise_centroid(id: u64) -> (f64, f64) {
    let (x, y) = cruise_axes(id);
    let scale = EARTH_CIRCUMFERENCE_M / f64::from(CRUISE_AXIS);
    meters_to_lonlat(
        (f64::from(x) + 0.5) * scale - EARTH_CIRCUMFERENCE_M / 2.0,
        EARTH_CIRCUMFERENCE_M / 2.0 - (f64::from(y) + 0.5) * scale,
    )
}

pub fn cruise_cell_name(id: u64) -> String {
    let (x, y) = cruise_axes(id);
    format!("z{CRUISE_ZOOM}/{x}/{y}")
}

/// Closed `(latitude, longitude)` ring; keep +180 on the last column's east edge.
pub fn cruise_cell_polygon(id: u64) -> Vec<(f64, f64)> {
    let (x, y) = cruise_axes(id);
    let longitude = |column| f64::from(column) * 360.0 / f64::from(CRUISE_AXIS) - 180.0;
    let latitude = |row| {
        let northing = (0.5 - f64::from(row) / f64::from(CRUISE_AXIS)) * EARTH_CIRCUMFERENCE_M;
        meters_to_lonlat(0.0, northing).1
    };
    let (west, east) = (longitude(x), longitude(x + 1));
    let (south, north) = (latitude(y + 1), latitude(y));
    vec![
        (south, west),
        (south, east),
        (north, east),
        (north, west),
        (south, west),
    ]
}
