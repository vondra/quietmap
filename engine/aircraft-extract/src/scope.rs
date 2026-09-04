//! Limit writes from a regional ADS-B cache to intersecting z9 partitions.

/// Preserve the previous regional extract's 50 km border coverage.
pub const SCOPE_BUFFER_M: f64 = 50_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeBbox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl ScopeBbox {
    pub fn parse(value: &str) -> Result<Self, String> {
        let coordinates = value
            .split(',')
            .map(|part| part.trim().parse::<f64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("invalid bbox: {error}"))?;
        let [min_lat, min_lon, max_lat, max_lon] = coordinates.as_slice() else {
            return Err("expected min_lat,min_lon,max_lat,max_lon".into());
        };
        if !coordinates.iter().all(|coordinate| coordinate.is_finite())
            || !(-90.0..=90.0).contains(min_lat)
            || !(-90.0..=90.0).contains(max_lat)
            || !(-180.0..=180.0).contains(min_lon)
            || !(-180.0..=180.0).contains(max_lon)
            || min_lat > max_lat
            || min_lon > max_lon
        {
            return Err("bbox must be finite, in range, and ordered south,west,north,east".into());
        }
        Ok(Self {
            min_lat: *min_lat,
            min_lon: *min_lon,
            max_lat: *max_lat,
            max_lon: *max_lon,
        })
    }

    pub fn contains_square(&self, id: u64) -> bool {
        if i64::try_from(id)
            .ok()
            .and_then(grid::square_from_id)
            .is_none()
        {
            return false;
        }
        let (south, west, north, east) = crate::spatial::square_bounds(id);
        let latitude_buffer = SCOPE_BUFFER_M / grid::geo::M_PER_DEG_LAT;
        if north < self.min_lat - latitude_buffer || south > self.max_lat + latitude_buffer {
            return false;
        }
        let extreme_latitude = north.abs().max(south.abs());
        let (_, longitude_buffer) =
            grid::geo::reach_box_half_extents_deg(extreme_latitude, SCOPE_BUFFER_M);
        [-360.0, 0.0, 360.0].iter().any(|shift| {
            east + shift >= self.min_lon - longitude_buffer
                && west + shift <= self.max_lon + longitude_buffer
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::square_id;

    #[test]
    fn regional_scope_keeps_borders_and_wraps_the_dateline() {
        let canary = ScopeBbox::parse("27,-18.5,29.5,-13").unwrap();
        assert!(canary.contains_square(square_id(27.93, -15.39).unwrap()));
        assert!(canary.contains_square(square_id(29.77, -15.4).unwrap()));
        assert!(!canary.contains_square(square_id(50.1, 14.26).unwrap()));
        let dateline = ScopeBbox::parse("-1,179.9,1,180").unwrap();
        assert!(dateline.contains_square(square_id(0.0, -179.99).unwrap()));
        assert!(!dateline.contains_square(u64::MAX));
    }

    #[test]
    fn invalid_scope_cannot_expand_destructive_work() {
        for value in [
            "27,18,29",
            "30,18,27,20",
            "NaN,0,1,1",
            "-91,0,1,1",
            "0,0,1,181",
        ] {
            assert!(ScopeBbox::parse(value).is_err(), "{value}");
        }
    }
}
