//! Antimeridian-safe flight geometry in the original trajectory model.

pub use crate::spatial::square_path;

pub(crate) const M_PER_DEG_LAT: f32 = 110_540.0;
pub(crate) const M_PER_DEG_LON_EQUATOR: f32 = 111_320.0;

/// Smallest signed longitude difference (`lon2 - lon1`) wrapped into
/// the range `(-180, 180]`. Critical for transpacific cruise where the
/// raw subtraction would yield ±300° instead of ±60°.
pub fn signed_lon_diff(lon1: f32, lon2: f32) -> f32 {
    let mut d = lon2 - lon1;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Equirectangular distance in metres between two `(lat, lon)` points.
/// Antimeridian-safe via [`signed_lon_diff`].
pub fn flat_dist(lat1: f32, lon1: f32, lat2: f32, lon2: f32) -> f32 {
    let mid_lat_rad = ((lat1 + lat2) as f64 * 0.5).to_radians();
    let cos_lat = mid_lat_rad.cos() as f32;
    let dx = signed_lon_diff(lon1, lon2) * M_PER_DEG_LON_EQUATOR * cos_lat;
    let dy = (lat2 - lat1) * M_PER_DEG_LAT;
    (dx * dx + dy * dy).sqrt()
}

/// Antimeridian-safe midpoint. Output longitude wrapped to `(-180, 180]`.
pub fn midpoint(lat1: f32, lon1: f32, lat2: f32, lon2: f32) -> (f32, f32) {
    interp_along_path(lat1, lon1, lat2, lon2, 0.5)
}

/// Antimeridian-safe linear interpolation along the path
/// `(lat1, lon1) → (lat2, lon2)` at fraction `frac` ∈ [0, 1].
/// Output longitude wrapped to `(-180, 180]`. Latitude is plain
/// linear (latitude cannot wrap).
pub fn interp_along_path(lat1: f32, lon1: f32, lat2: f32, lon2: f32, frac: f32) -> (f32, f32) {
    let lat = lat1 + (lat2 - lat1) * frac;
    let mut lon = lon1 + signed_lon_diff(lon1, lon2) * frac;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon <= -180.0 {
        lon += 360.0;
    }
    (lat, lon)
}
