//! Source ownership and shared line geometry gates.

use std::path::Path;

pub(super) const BUILDING_QUERY_RADIUS_M: f64 = 2_000.0;
pub(super) const INDUSTRIAL_QUERY_RADIUS_M: f64 = 5_000.0;
/// Leisure prefetch horizon: 4 km so the v4 formula classes (motorsport /
/// shooting, industrial reach) prefilter like industrial rows. The per-row
/// gate stays class-aware (2 km centroid for area classes).
pub(super) const LEISURE_QUERY_RADIUS_M: f64 = 4_000.0;
/// A ship row is read when its cell centre lies within the sub-cell reach plus the cell's
/// half diagonal (`batches_within` adds its own 2 % slack).
pub(super) const SHIP_QUERY_RADIUS_M: f64 = noise_compute::emission::ships::SHIP_MAX_RADIUS_M
    + noise_compute::emission::ships::SHIP_CELL_HALF_DIAGONAL_M;
// Existing row gates bound accepted line midpoints by 1.5 times their reach.
const LINE_MIDPOINT_REACH_FACTOR: f64 = 1.5;

/// Select every owner in the existing surface-source midpoint gates and every
/// cruise and airborne owner within reach. Airborne rows are owned by their
/// midpoint square and reach at most `AIRBORNE_QUERY_RADIUS_M` (16 km + half
/// the length cap); the radius box, the envelope and the cap share one
/// metre-per-degree metric, so an accepted arc's owner is always loaded.
pub fn squares_within_reach(lat: f64, lng: f64) -> Result<Vec<grid::Square>, String> {
    squares_within_radius(
        lat,
        lng,
        surface_reach_m()
            .max(noise_compute::emission::aircraft::CRUISE_QUERY_RADIUS_M)
            .max(noise_compute::emission::aircraft::AIRBORNE_QUERY_RADIUS_M),
    )
}

/// Owners that can hold a surface source or an obstacle screening the receiver.
pub fn surface_squares_within_reach(lat: f64, lng: f64) -> Result<Vec<grid::Square>, String> {
    squares_within_radius(lat, lng, surface_reach_m())
}

fn surface_reach_m() -> f64 {
    noise_compute::constants::RAILWAY_REACH_CEILING
        .max(noise_compute::constants::ROAD_MAX_RADIUS[0])
        .max(noise_compute::constants::GROUND_OPS_RUNWAY_MAX_RADIUS)
        .max(BUILDING_QUERY_RADIUS_M)
        .max(LEISURE_QUERY_RADIUS_M)
        .max(INDUSTRIAL_QUERY_RADIUS_M)
        .max(SHIP_QUERY_RADIUS_M)
        * LINE_MIDPOINT_REACH_FACTOR
}

pub fn squares_within_radius(
    lat: f64,
    lng: f64,
    radius_m: f64,
) -> Result<Vec<grid::Square>, String> {
    if !lat.is_finite()
        || lat.abs() > 90.0
        || !lng.is_finite()
        || !radius_m.is_finite()
        || radius_m < 0.0
    {
        return Err("invalid source query position or radius".to_string());
    }
    let lng = grid::geo::normalize_longitude(lng);
    let (latitude_radius, longitude_radius) = grid::geo::reach_box_half_extents_deg(lat, radius_m);
    let bounds = grid::bounds::BoundedSquares::from_degrees(
        (lat - latitude_radius).next_down(),
        (lng - longitude_radius).next_down(),
        (lat + latitude_radius).next_up(),
        (lng + longitude_radius).next_up(),
    )
    .ok_or_else(|| "invalid source query bounds".to_string())?;
    Ok(bounds.iter().collect())
}

/// Directory `<prepared_year>/z9/<x>/<y>` for one square.
pub fn square_dir(prepared_year_dir: &Path, square: grid::Square) -> std::path::PathBuf {
    prepared_year_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string())
}

pub(super) fn line_midpoint_exceeds_reach(
    lat: f64,
    lon: f64,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    radius_m: f64,
) -> bool {
    let mid_lat = (start_lat + end_lat) * 0.5;
    let latitude_distance = (lat - mid_lat).abs() * grid::geo::M_PER_DEG_LAT;
    if latitude_distance > radius_m * LINE_MIDPOINT_REACH_FACTOR {
        return true;
    }
    let mid_lon = grid::geo::wrapped_longitude_midpoint(start_lon, end_lon);
    let longitude_distance = grid::geo::wrapped_longitude_delta(mid_lon, lon).abs()
        * grid::geo::m_per_deg_lon(mid_lat.to_radians());
    longitude_distance > radius_m * LINE_MIDPOINT_REACH_FACTOR
}

// A zero length would bypass arc screening and disagree with the painter.
pub(super) fn segment_length_m(
    stored_length: Option<f32>,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
) -> f32 {
    stored_length
        .filter(|length| *length > 0.0)
        .unwrap_or_else(|| grid::geo::flat_dist(start_lat, start_lon, end_lat, end_lon) as f32)
}
