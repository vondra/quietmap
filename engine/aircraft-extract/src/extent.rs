//! Measured wrapped geometry bounds and a canonical z9 broadphase.
use crate::geo::{M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Extent {
    south: f64,
    west: f64,
    north: f64,
    east: f64,
    center_lon: f64,
}

impl Extent {
    pub(crate) fn empty(owner: u64) -> Self {
        let (_, west, _, east) = crate::spatial::square_bounds(owner);
        Self {
            south: f64::INFINITY,
            west: f64::INFINITY,
            north: f64::NEG_INFINITY,
            east: f64::NEG_INFINITY,
            center_lon: (west + east) * 0.5,
        }
    }

    pub(crate) fn include(&mut self, lat: f32, lon: f32) {
        let lon =
            self.center_lon + grid::geo::wrapped_longitude_delta(self.center_lon, f64::from(lon));
        self.south = self.south.min(f64::from(lat));
        self.north = self.north.max(f64::from(lat));
        self.west = self.west.min(lon);
        self.east = self.east.max(lon);
    }

    pub(crate) fn is_empty(self) -> bool {
        self.south > self.north
    }

    pub(crate) fn padded(mut self, meters: f32) -> Self {
        if self.is_empty() {
            return self;
        }
        let lon_scale = f64::from(M_PER_DEG_LON_EQUATOR)
            * self.south.abs().max(self.north.abs()).to_radians().cos();
        let lon_pad = (f64::from(meters) / lon_scale).min(180.0);
        let lat_pad = f64::from(meters / M_PER_DEG_LAT);
        self.south -= lat_pad;
        self.north += lat_pad;
        self.west -= lon_pad;
        self.east += lon_pad;
        self
    }

    pub(crate) fn intersects(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.south <= other.north
            && other.south <= self.north
            && [-360.0, 0.0, 360.0]
                .into_iter()
                .any(|shift| self.west <= other.east + shift && other.west + shift <= self.east)
    }

    pub(crate) fn squares(self) -> Vec<u64> {
        if self.is_empty() {
            return Vec::new();
        }
        let north = grid::square_of(self.north, 0.0).y;
        let south = grid::square_of(self.south, 0.0).y;
        let first_x = ((self.west + 180.0) / grid::Z9_SPAN_DEG).floor() as i32;
        let last_x = (((self.east + 180.0) / grid::Z9_SPAN_DEG).floor() as i32).min(first_x + 511);
        (first_x..=last_x)
            .flat_map(|x| {
                (north..=south).map(move |y| {
                    grid::square_id(grid::Square {
                        x: x.rem_euclid(512) as u16,
                        y,
                    }) as u64
                })
            })
            .collect()
    }
}
