//! Exact local distances and wrapped-longitude arithmetic for source topology.

/// Metres per degree latitude (WGS-84 mean).
pub const M_PER_DEG_LAT: f64 = 110_540.0;
/// Metres per degree longitude at the equator.
pub const M_PER_DEG_LON_EQ: f64 = 111_320.0;

/// Fold a longitude into the one canonical interval used by stored points.
#[inline]
pub fn normalize_longitude(lon_deg: f64) -> f64 {
    if (-180.0..180.0).contains(&lon_deg) {
        lon_deg
    } else {
        (lon_deg + 180.0).rem_euclid(360.0) - 180.0
    }
}

/// Signed shortest longitude delta from `from_deg` to `to_deg`.
#[inline]
pub fn wrapped_longitude_delta(from_deg: f64, to_deg: f64) -> f64 {
    normalize_longitude(to_deg - from_deg)
}

/// Midpoint longitude on the short arc, including across the antimeridian.
#[inline]
pub fn wrapped_longitude_midpoint(a_deg: f64, b_deg: f64) -> f64 {
    interpolate_longitude_short_arc(a_deg, b_deg, 0.5)
}

/// Canonical longitude at fraction `t` along the short source–receiver arc.
#[inline]
pub fn interpolate_longitude_short_arc(from_deg: f64, to_deg: f64, t: f64) -> f64 {
    normalize_longitude(from_deg + t * wrapped_longitude_delta(from_deg, to_deg))
}

/// Metres per degree longitude at `lat_rad` radians.
pub fn m_per_deg_lon(lat_rad: f64) -> f64 {
    M_PER_DEG_LON_EQ * lat_rad.cos().max(0.01)
}

/// Flat-earth distance in meters (accurate <0.3% at <50km).
pub fn flat_dist(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let mid_lat = ((lat1 + lat2) / 2.0).to_radians();
    let dx = wrapped_longitude_delta(lon1, lon2) * m_per_deg_lon(mid_lat);
    let dy = (lat2 - lat1) * M_PER_DEG_LAT;
    (dx * dx + dy * dy).sqrt()
}

/// Cumulative [`flat_dist`] metres along `points` (`[lat, lon]`), starting at `first_m`.
/// Left-to-right additions only, so a reader continuing from a stored interior value
/// reproduces the whole-way prefix sums bit for bit.
pub fn cumulative_flat_metres(first_m: f64, points: &[[f64; 2]]) -> Vec<f64> {
    let mut metres = Vec::with_capacity(points.len());
    if points.is_empty() {
        return metres;
    }
    metres.push(first_m);
    for hop in points.windows(2) {
        metres
            .push(metres[metres.len() - 1] + flat_dist(hop[0][0], hop[0][1], hop[1][0], hop[1][1]));
    }
    metres
}

/// A local-metre projection on the shortest longitude arc, including degenerate segments.
pub fn point_to_segment(
    point_lat: f64,
    point_lon: f64,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
) -> (f64, f64, f64, f64) {
    let longitude_metres = m_per_deg_lon(((start_lat + end_lat) / 2.0).to_radians());
    let bx = wrapped_longitude_delta(start_lon, end_lon) * longitude_metres;
    let by = (end_lat - start_lat) * M_PER_DEG_LAT;
    let px = wrapped_longitude_delta(start_lon, point_lon) * longitude_metres;
    let py = (point_lat - start_lat) * M_PER_DEG_LAT;
    let squared_length = bx * bx + by * by;
    let fraction = if squared_length < 1e-10 {
        0.0
    } else {
        ((px * bx + py * by) / squared_length).clamp(0.0, 1.0)
    };
    (
        ((px - fraction * bx).powi(2) + (py - fraction * by).powi(2)).sqrt(),
        start_lat + fraction * (end_lat - start_lat),
        interpolate_longitude_short_arc(start_lon, end_lon, fraction),
        fraction,
    )
}
