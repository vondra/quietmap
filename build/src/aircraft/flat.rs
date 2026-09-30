//! dev4's flight geometry: equirectangular distance and interpolation, safe across the antimeridian.

pub const M_PER_DEG_LAT: f32 = 110_540.0;
pub const M_PER_DEG_LON_EQUATOR: f32 = 111_320.0;

/// `to - from` in degrees of longitude, wrapped into (-180, 180].
pub fn signed_longitude_delta(from: f32, to: f32) -> f32 {
    let delta = to - from;
    if delta > 180.0 {
        delta - 360.0
    } else if delta <= -180.0 {
        delta + 360.0
    } else {
        delta
    }
}

/// Distance in metres at the mean latitude.
pub fn flat_distance_m(lat1: f32, lon1: f32, lat2: f32, lon2: f32) -> f32 {
    let cos_lat = ((f64::from(lat1) + f64::from(lat2)) * 0.5)
        .to_radians()
        .cos() as f32;
    let dx = signed_longitude_delta(lon1, lon2) * M_PER_DEG_LON_EQUATOR * cos_lat;
    let dy = (lat2 - lat1) * M_PER_DEG_LAT;
    (dx * dx + dy * dy).sqrt()
}

/// The point at `fraction` of the way, longitude wrapped into (-180, 180].
pub fn interpolate(lat1: f32, lon1: f32, lat2: f32, lon2: f32, fraction: f32) -> (f32, f32) {
    let lat = lat1 + (lat2 - lat1) * fraction;
    let mut lon = lon1 + signed_longitude_delta(lon1, lon2) * fraction;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon <= -180.0 {
        lon += 360.0;
    }
    (lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_antimeridian_is_crossed_the_short_way() {
        assert!((signed_longitude_delta(179.9, -179.9) - 0.2).abs() < 1e-3);
        assert!((signed_longitude_delta(-179.9, 179.9) + 0.2).abs() < 1e-3);
        // f32 resolves 7.6e-6 degree (0.85 m) near 180 degrees.
        let across = flat_distance_m(0.0, 179.999, 0.0, -179.999);
        assert!((across - 222.6).abs() < 2.0, "{across}");
        let (lat, lon) = interpolate(0.0, 179.9, 0.0, -179.9, 0.75);
        assert_eq!(lat, 0.0);
        assert!((lon + 179.95).abs() < 1e-4, "{lon}");
    }
}
