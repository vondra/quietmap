//! Flat-earth geometry for the popup read path, ported from
//! `noise-compute/src/propagation/geo.rs` (single source of truth there;
//! duplicated here so the store crate builds standalone — reunite on the
//! `noise-compute` transfer).

/// Metres per degree latitude (WGS-84 mean).
pub const M_PER_DEG_LAT: f64 = 110_540.0;
/// Metres per degree longitude at the equator.
pub const M_PER_DEG_LON_EQ: f64 = 111_320.0;

/// Metres per degree longitude at a latitude (radians).
pub fn m_per_deg_lon(lat_rad: f64) -> f64 {
    M_PER_DEG_LON_EQ * lat_rad.cos().max(0.01)
}

/// Planar distance in metres between two lon/lat points.
pub fn flat_dist(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let mid_lat = ((lat1 + lat2) / 2.0).to_radians();
    let dx = (lon2 - lon1) * m_per_deg_lon(mid_lat);
    let dy = (lat2 - lat1) * M_PER_DEG_LAT;
    (dx * dx + dy * dy).sqrt()
}

/// Closest point on segment AB to P (lon/lat degrees).
pub struct ClosestPoint {
    pub lat: f64,
    pub lon: f64,
    pub dist_m: f64,
    /// Unclamped fraction along A→B (may leave [0, 1]).
    pub fraction: f64,
}

pub fn closest_point_on_segment(
    p_lat: f64,
    p_lon: f64,
    a_lat: f64,
    a_lon: f64,
    b_lat: f64,
    b_lon: f64,
) -> ClosestPoint {
    let mid_lat = ((a_lat + b_lat) / 2.0).to_radians();
    let m_lon = m_per_deg_lon(mid_lat);
    let bx = (b_lon - a_lon) * m_lon;
    let by = (b_lat - a_lat) * M_PER_DEG_LAT;
    let px = (p_lon - a_lon) * m_lon;
    let py = (p_lat - a_lat) * M_PER_DEG_LAT;
    let ab_len_sq = bx * bx + by * by;
    let t_unclamped = if ab_len_sq < 1e-10 {
        0.0
    } else {
        (px * bx + py * by) / ab_len_sq
    };
    let t = t_unclamped.clamp(0.0, 1.0);
    let cp_x = t * bx;
    let cp_y = t * by;
    ClosestPoint {
        lat: a_lat + t * (b_lat - a_lat),
        lon: a_lon + t * (b_lon - a_lon),
        dist_m: ((px - cp_x).powi(2) + (py - cp_y).powi(2)).sqrt(),
        fraction: t_unclamped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_dist_prague_1km() {
        let d = flat_dist(50.08, 14.42, 50.08, 14.434);
        assert!((d - 1000.0).abs() < 50.0, "d={d}");
    }

    #[test]
    fn closest_point_mid_segment() {
        let cp = closest_point_on_segment(50.08, 14.42, 50.079, 14.41, 50.079, 14.43);
        assert!(cp.dist_m > 50.0 && cp.dist_m < 200.0, "dist={}", cp.dist_m);
        assert!((cp.fraction - 0.5).abs() < 0.1, "frac={}", cp.fraction);
    }
}
