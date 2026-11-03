//! Exact OSM projection and square ownership on the prepared z30 grid.

mod geo;
pub mod poly;
pub use geo::*;
use std::f64::consts::PI;

/// EPSG:3857 sphere radius, metres.
pub const WEB_MERCATOR_RADIUS_M: f64 = 6_378_137.0;
/// Full equatorial circumference, metres: 2·π·R.
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_49;
/// Coordinate quantum, metres: circumference/2^30. Max global coordinate
/// 2^30 < i32::MAX, so z30 is the finest grid a global int32 holds.
pub const GRID_QUANTUM_M: f64 = 0.037_322_767_717_044_72;
/// Web-Mercator latitude limit, degrees.
pub const MAX_MERCATOR_LAT_DEG: f64 = 85.051_128_78;
/// A z9 unit: slippy `x`/`y` in `0..512`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Square {
    pub x: u16,
    pub y: u16,
}

/// Project lon/lat degrees to EPSG:3857 metres. Latitude clamps to the
/// Mercator limit so poles map to the edge row, never to infinity.
pub fn lonlat_to_meters(lon_deg: f64, lat_deg: f64) -> (f64, f64) {
    let lat = lat_deg.clamp(-MAX_MERCATOR_LAT_DEG, MAX_MERCATOR_LAT_DEG);
    let x = WEB_MERCATOR_RADIUS_M * lon_deg.to_radians();
    let y = WEB_MERCATOR_RADIUS_M * ((PI / 4.0 + lat.to_radians() / 2.0).tan().ln());
    (x, y)
}

/// Snap 3857 metres to the global int32 z30 grid. i64 holds the quotient
/// exactly (2^30 steps); the cast is safe because |coord| < 2^30 < i32::MAX.
pub fn meters_to_grid(x_m: f64, y_m: f64) -> (i32, i32) {
    let gx = (x_m / GRID_QUANTUM_M).floor() as i64 + (1 << 29);
    let gy = (y_m / GRID_QUANTUM_M).floor() as i64 + (1 << 29);
    (gx as i32, gy as i32)
}

/// Project lon/lat degrees straight to grid cells.
pub fn lonlat_to_grid(lon_deg: f64, lat_deg: f64) -> (i32, i32) {
    let (x, y) = lonlat_to_meters(lon_deg, lat_deg);
    meters_to_grid(x, y)
}

/// Grid cell corner back to 3857 metres.
pub fn grid_to_meters(gx: i32, gy: i32) -> (f64, f64) {
    (
        (gx as i64 - (1 << 29)) as f64 * GRID_QUANTUM_M,
        (gy as i64 - (1 << 29)) as f64 * GRID_QUANTUM_M,
    )
}

/// Slippy-tile cell `(x, y)` containing lon/lat at `zoom` (row-major from the
/// north-west corner). Longitude wraps at the antimeridian; latitude clamps
/// with the projection, so the poles land on the edge row.
pub fn web_mercator_cell_axes(lat_deg: f64, lon_deg: f64, zoom: u32) -> (u32, u32) {
    let axis = f64::from(1u32 << zoom);
    let lon = geo::normalize_longitude(lon_deg);
    let mut x = ((lon + 180.0) / 360.0 * axis)
        .floor()
        .clamp(0.0, axis - 1.0) as u32;
    // Adding 180 can erase a sub-ULP offset from an exact cell edge.
    // Compare against the dyadic geographic edges before assigning ownership.
    let span = 360.0 / axis;
    let west = f64::from(x) * span - 180.0;
    if lon < west {
        x = x.saturating_sub(1);
    } else if lon >= west + span && f64::from(x + 1) < axis {
        x += 1;
    }
    let (_, northing) = lonlat_to_meters(0.0, lat_deg);
    let y = ((0.5 - northing / EARTH_CIRCUMFERENCE_M) * axis).floor();
    (x, y.clamp(0.0, axis - 1.0) as u32)
}

/// z9 unit containing lon/lat. Longitude wraps at the antimeridian;
/// latitude clamps with the projection.
pub fn square_of(lat_deg: f64, lon_deg: f64) -> Square {
    let (x, y) = web_mercator_cell_axes(lat_deg, lon_deg, 9);
    Square {
        x: x as u16,
        y: y as u16,
    }
}

/// Canonical unit name, e.g. `z9/276/173`.
pub fn square_name(square: Square) -> String {
    format!("z9/{}/{}", square.x, square.y)
}

pub fn grid_cell_lonlat(gx: i32, gy: i32) -> (f64, f64) {
    let (x, y) = grid_to_meters(gx, gy);
    poly::meters_to_lonlat(x, y)
}
