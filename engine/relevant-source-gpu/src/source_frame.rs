//! Region-local source geometry for fixed 16-pixel relevance blocks.

use h3o::{CellIndex, LatLng};
use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
use tile_painter::grid::TILE_PX;
use tile_painter::source_line::LineRow;
use tile_painter::source_point::PointRow;

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

pub const BLOCK_PIXEL_SIDE: usize = 16;
pub const TILE_PIXEL_SIDE: usize = TILE_PX;
pub const BLOCKS_PER_TILE_SIDE: usize = TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE;
pub const BLOCK_COUNT: usize = BLOCKS_PER_TILE_SIDE * BLOCKS_PER_TILE_SIDE;
pub const CORNERS_PER_TILE_SIDE: usize = BLOCKS_PER_TILE_SIDE + 1;
pub const CORNER_COUNT: usize = CORNERS_PER_TILE_SIDE * CORNERS_PER_TILE_SIDE;
pub const PERIOD_COUNT: usize = 3;
pub const BAND_COUNT: usize = 8;

/// The longest source→receiver ray the CUDA cadence still charts inside
/// `QUIETMAP_MAXIMUM_PROFILE_POINTS` chainages (`kernels/relevant_source_path.cuh`).
///
/// Running that cadence on the reference card over 20 million distances from
/// 0.01 m to 20 km in 1 mm steps, a ray first needs a 65th chainage at
/// 11,872.35 m. A dropped chainage moves painted bytes with no other sign, so the
/// cap is held here, at the exact distance it was measured for, rather than
/// through the reach and segment-length premises it was derived from: the world
/// sits far inside it — the widest reach any source carries is rail's 11 km
/// ceiling and `osm-extract` splits every way at a hard 250 m (the longest
/// `length_m` in all 121,790 prepared cells of the 2026 world), an 11,250 m ray.
///
/// This refusal is the cheap one, before a bad source ever reaches the card and
/// with a message that names it. The guarantee is the card's own
/// `quietmap_profile_overflow`, which `RelevantSourceCuda::take_profile_overflow`
/// reads after every paint.
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
    pub fn for_cell(cell: CellIndex) -> Self {
        let centre = LatLng::from(cell);
        Self::for_latitude_longitude(centre.lat(), centre.lng())
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
            ((longitude - self.reference_longitude) * self.metres_per_longitude_degree) as f32,
            ((latitude - self.reference_latitude) * M_PER_DEG_LAT) as f32,
        ]
    }

    #[inline]
    pub fn decode(&self, x_m: f32, y_m: f32) -> [f64; 2] {
        [
            self.reference_latitude + f64::from(y_m) / M_PER_DEG_LAT,
            self.reference_longitude + f64::from(x_m) / self.metres_per_longitude_degree,
        ]
    }

    pub fn encode_line(&self, row: &LineRow) -> DeviceLineSource {
        let start = self.encode(row.start_lat, row.start_lon);
        let end = self.encode(row.end_lat, row.end_lon);
        DeviceLineSource {
            start_x_m: start[0],
            start_y_m: start[1],
            end_x_m: end[0],
            end_y_m: end[1],
            extent_m: row.length_m,
            max_distance_m: row.max_distance_m as f32,
            source_height_m: row.source_height_m as f32,
            flags: if row.bridge { SOURCE_FLAG_BRIDGE } else { 0 },
            emission_linear: flatten_emission(&row.emission_lin),
        }
    }

    /// One airport ground-ops microsegment for one vehicle kind, its rows' event
    /// energies already summed per period (class-weighted for aircraft).
    #[allow(clippy::too_many_arguments)]
    pub fn encode_ground_ops(
        &self,
        start_lat: f64,
        start_lon: f64,
        end_lat: f64,
        end_lon: f64,
        length_m: f32,
        max_distance_m: f64,
        source_height_m: f64,
        flag: u32,
        emission: &[[f32; BAND_COUNT]; PERIOD_COUNT],
    ) -> DeviceLineSource {
        let start = self.encode(start_lat, start_lon);
        let end = self.encode(end_lat, end_lon);
        DeviceLineSource {
            start_x_m: start[0],
            start_y_m: start[1],
            end_x_m: end[0],
            end_y_m: end[1],
            extent_m: length_m,
            max_distance_m: max_distance_m as f32,
            source_height_m: source_height_m as f32,
            flags: flag,
            emission_linear: flatten_emission(emission),
        }
    }

    pub fn encode_point(&self, row: &PointRow) -> DeviceLineSource {
        let position = self.encode(row.lat, row.lon);
        DeviceLineSource {
            start_x_m: position[0],
            start_y_m: position[1],
            end_x_m: position[0],
            end_y_m: position[1],
            extent_m: row.exclusion_radius_m as f32,
            max_distance_m: row.max_distance_m as f32,
            source_height_m: row.source_height_m as f32,
            flags: SOURCE_FLAG_POINT,
            emission_linear: flatten_emission(&row.emission_lin),
        }
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

fn flatten_emission(
    emission: &[[f32; BAND_COUNT]; PERIOD_COUNT],
) -> [f32; PERIOD_COUNT * BAND_COUNT] {
    let mut flat = [0.0; PERIOD_COUNT * BAND_COUNT];
    for period in 0..PERIOD_COUNT {
        flat[period * BAND_COUNT..(period + 1) * BAND_COUNT].copy_from_slice(&emission[period]);
    }
    flat
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
