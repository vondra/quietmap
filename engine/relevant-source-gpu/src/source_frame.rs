//! Region-local source geometry for fixed 16-pixel relevance blocks.

use grid::{
    surface_corner::{SurfaceCorner, BLOCKS_PER_SQUARE_SIDE},
    Square,
};
use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};

/// `DeviceLineSource::flags`: the segment propagates over hard ground (a bridge).
pub const SOURCE_FLAG_BRIDGE: u32 = 1;
/// `DeviceLineSource::flags`: a point source (industrial, building): start == end,
/// spherical divergence, `extent_m` is its footprint exclusion radius.
pub const SOURCE_FLAG_POINT: u32 = 2;
/// `DeviceLineSource::flags`: an airport ground-ops microsegment's aircraft rows:
/// event energy over n_days, `theta / d_perp` divergence, band-mean ground, exact cadence.
pub const SOURCE_FLAG_GROUND_OPS_AIRCRAFT: u32 = 4;
/// `DeviceLineSource::flags`: the same microsegment's ground-support rows,
/// `GROUND_OPS_REF_OFFSET_M / d` divergence.
pub const SOURCE_FLAG_GROUND_OPS_GSE: u32 = 8;

pub use grid::surface_corner::{
    BLOCKS_PER_TILE_SIDE, BLOCK_PIXEL_SIDE, CORNERS_PER_TILE_SIDE, CORNER_COUNT, TILE_PIXEL_SIDE,
};
pub const BLOCK_COUNT: usize = BLOCKS_PER_TILE_SIDE * BLOCKS_PER_TILE_SIDE;
pub const PERIOD_COUNT: usize = 3;
pub const BAND_COUNT: usize = 8;

/// The 64-point CUDA cadence first needs a 65th sample at 11,872.35 m;
/// reject longer rays and check the device overflow flag after each launch.
pub const MAXIMUM_PROFILE_RAY_M: f32 = 11_872.0;

/// One source encoded once in the metric frame shared by a region's tiles and CUDA
/// scene: a line segment, or a point (`SOURCE_FLAG_POINT`) with start == end.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct DeviceLineSource {
    pub start_x_m: f32,
    pub start_y_m: f32,
    pub end_x_m: f32,
    pub end_y_m: f32,
    /// Segment length for a line; footprint exclusion radius for a point.
    pub extent_m: f32,
    pub max_distance_m: f32,
    pub source_height_m: f32,
    pub flags: u32,
    pub emission_linear: [f32; PERIOD_COUNT * BAND_COUNT],
}

impl DeviceLineSource {
    /// The longest ray any receiver can make the kernel profile for this source.
    ///
    /// The reach bounds the distance to the CLOSEST point of the segment; the fan
    /// and arc passes then profile points along the segment itself
    /// (`segment_point_at_azimuth` clamps to it), which the triangle inequality
    /// puts at most one segment further away. A point source has start == end and
    /// so adds nothing.
    pub fn longest_profile_ray_m(&self) -> f32 {
        self.max_distance_m + (self.end_x_m - self.start_x_m).hypot(self.end_y_m - self.start_y_m)
    }

    /// True when no receiver can make this source outrun the profile cadence.
    pub fn fits_the_profile_cadence(&self) -> bool {
        self.longest_profile_ray_m() <= MAXIMUM_PROFILE_RAY_M
    }
}

/// A stable local metric frame whose f32 coordinates retain centimetre-scale resolution.
#[derive(Clone, Copy, Debug)]
pub struct RegionMetricFrame {
    reference_latitude: f64,
    reference_longitude: f64,
    metres_per_longitude_degree: f64,
}

impl RegionMetricFrame {
    pub fn for_square(square: Square) -> Self {
        let corner = SurfaceCorner::new(
            u32::from(square.x) * BLOCKS_PER_SQUARE_SIDE + BLOCKS_PER_SQUARE_SIDE / 2,
            u32::from(square.y) * BLOCKS_PER_SQUARE_SIDE + BLOCKS_PER_SQUARE_SIDE / 2,
        )
        .expect("valid square");
        let [latitude, longitude] = corner.latitude_longitude();
        Self::for_latitude_longitude(latitude, longitude)
    }

    pub fn for_latitude_longitude(latitude: f64, longitude: f64) -> Self {
        Self {
            reference_latitude: latitude,
            reference_longitude: longitude,
            metres_per_longitude_degree: m_per_deg_lon(latitude.to_radians()),
        }
    }

    #[inline]
    pub fn encode(&self, latitude: f64, longitude: f64) -> [f32; 2] {
        [
            (grid::geo::wrapped_longitude_delta(self.reference_longitude, longitude)
                * self.metres_per_longitude_degree) as f32,
            ((latitude - self.reference_latitude) * M_PER_DEG_LAT) as f32,
        ]
    }

    #[inline]
    pub fn decode(&self, x_m: f32, y_m: f32) -> [f64; 2] {
        [
            self.reference_latitude + f64::from(y_m) / M_PER_DEG_LAT,
            grid::geo::normalize_longitude(
                self.reference_longitude + f64::from(x_m) / self.metres_per_longitude_degree,
            ),
        ]
    }

    pub fn reference_latitude(&self) -> f64 {
        self.reference_latitude
    }

    pub fn reference_longitude(&self) -> f64 {
        self.reference_longitude
    }

    pub fn metres_per_longitude_degree(&self) -> f64 {
        self.metres_per_longitude_degree
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_source_layout_is_the_expected_four_cache_lines() {
        assert_eq!(std::mem::size_of::<DeviceLineSource>(), 128);
    }

    /// The rule the profile cap rests on: a receiver at the reach of the segment's
    /// closest point can still be a whole segment away from the far end, and the
    /// fan profiles that ray too. The world's widest case — an 11 km rail reach on
    /// a 250 m microsegment — must fit, and one segment longer than the cadence
    /// can chart must not.
    #[test]
    fn a_source_outruns_the_profile_cadence_only_past_its_measured_reach() {
        let widest_in_the_world = DeviceLineSource {
            start_x_m: 0.0,
            start_y_m: 0.0,
            end_x_m: 250.0,
            end_y_m: 0.0,
            max_distance_m: 11_000.0,
            ..DeviceLineSource::default()
        };
        assert_eq!(widest_in_the_world.longest_profile_ray_m(), 11_250.0);
        assert!(widest_in_the_world.fits_the_profile_cadence());

        let point = DeviceLineSource {
            max_distance_m: MAXIMUM_PROFILE_RAY_M,
            flags: SOURCE_FLAG_POINT,
            ..DeviceLineSource::default()
        };
        assert!(point.fits_the_profile_cadence(), "a point adds no segment");

        let too_long = DeviceLineSource {
            end_x_m: 1_000.0,
            max_distance_m: 11_000.0,
            ..DeviceLineSource::default()
        };
        assert_eq!(too_long.longest_profile_ray_m(), 12_000.0);
        assert!(!too_long.fits_the_profile_cadence());
    }
}
