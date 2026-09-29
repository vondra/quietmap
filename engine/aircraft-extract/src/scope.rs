//! Limit writes from a regional ADS-B cache to intersecting z9 partitions.

/// Preserve the previous regional extract's 50 km border coverage.
pub const SCOPE_BUFFER_M: f64 = 50_000.0;

/// One or more `south,west,north,east` boxes (`;`-separated); a square is in
/// scope when any box reaches it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeBbox {
    boxes: Vec<Bbox>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bbox {
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
}

impl ScopeBbox {
    pub fn parse(value: &str) -> Result<Self, String> {
        let boxes = value
            .split(';')
            .map(Bbox::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { boxes })
    }

    /// Canonical text of the scope, recorded by stage receipts.
    pub fn key(&self) -> String {
        self.boxes
            .iter()
            .map(|b| format!("{},{},{},{}", b.min_lat, b.min_lon, b.max_lat, b.max_lon))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Whether a trace whose sane samples span `[south, north] × [west, east]`
    /// can reach a written square: the box meets a scope box grown by the
    /// square buffer and one z9 square, so every segment, transit or ground
    /// run a scoped stage keeps lies inside a kept trace's extent.
    pub fn may_touch_extent(&self, south: f64, west: f64, north: f64, east: f64) -> bool {
        self.boxes
            .iter()
            .any(|b| b.may_touch_extent(south, west, north, east))
    }

    pub fn contains_square(&self, id: u64) -> bool {
        if i64::try_from(id)
            .ok()
            .and_then(grid::square_from_id)
            .is_none()
        {
            return false;
        }
        let bounds = crate::spatial::square_bounds(id);
        self.boxes.iter().any(|b| b.reaches_square(bounds))
    }
}

impl Bbox {
    fn parse(value: &str) -> Result<Self, String> {
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

    fn may_touch_extent(&self, south: f64, west: f64, north: f64, east: f64) -> bool {
        let square_deg = 360.0 / f64::from(1u32 << 9);
        let latitude_buffer = SCOPE_BUFFER_M / grid::geo::M_PER_DEG_LAT + square_deg;
        if north < self.min_lat - latitude_buffer || south > self.max_lat + latitude_buffer {
            return false;
        }
        if east - west >= 180.0 {
            return true;
        }
        let extreme_latitude =
            (self.max_lat.abs().max(self.min_lat.abs()) + latitude_buffer).min(89.0);
        let (_, longitude_buffer) =
            grid::geo::reach_box_half_extents_deg(extreme_latitude, SCOPE_BUFFER_M);
        self.overlaps_longitudes(west, east, longitude_buffer + square_deg)
    }

    fn reaches_square(&self, (south, west, north, east): (f64, f64, f64, f64)) -> bool {
        let latitude_buffer = SCOPE_BUFFER_M / grid::geo::M_PER_DEG_LAT;
        if north < self.min_lat - latitude_buffer || south > self.max_lat + latitude_buffer {
            return false;
        }
        let extreme_latitude = north.abs().max(south.abs());
        let (_, longitude_buffer) =
            grid::geo::reach_box_half_extents_deg(extreme_latitude, SCOPE_BUFFER_M);
        self.overlaps_longitudes(west, east, longitude_buffer)
    }

    fn overlaps_longitudes(&self, west: f64, east: f64, buffer: f64) -> bool {
        [-360.0, 0.0, 360.0].iter().any(|shift| {
            east + shift >= self.min_lon - buffer && west + shift <= self.max_lon + buffer
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
        let two = ScopeBbox::parse("27,-18.5,29.5,-13;49.75,13.8,50.45,14.8").unwrap();
        assert!(two.contains_square(square_id(27.93, -15.39).unwrap()));
        assert!(two.contains_square(square_id(50.1, 14.26).unwrap()));
        assert!(!two.contains_square(square_id(47.45, 8.56).unwrap()));
        assert!(two.may_touch_extent(50.0, 14.0, 50.1, 14.1));
        assert!(!two.may_touch_extent(47.4, 8.5, 47.5, 8.6));
        assert_eq!(two.key(), "27,-18.5,29.5,-13;49.75,13.8,50.45,14.8");
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
