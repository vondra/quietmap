//! Periodic airborne selection and conservative publication support for popup geometry.

use super::{meters_to_lat_deg, meters_to_lon_deg, AIRCRAFT_MAX_HORIZONTAL_REACH_M};
use grid::bounds::BoundedSquares;

#[derive(Clone, Copy, Debug)]
pub struct AirborneEnvelope {
    south: f32,
    north: f32,
    longitude_intervals: [[f32; 2]; 3],
}

impl AirborneEnvelope {
    pub fn new(lat: f64, lon: f64) -> Self {
        let lat_pad = meters_to_lat_deg(AIRCRAFT_MAX_HORIZONTAL_REACH_M);
        let lon_pad = meters_to_lon_deg(lat, AIRCRAFT_MAX_HORIZONTAL_REACH_M);
        let lon = grid::geo::normalize_longitude(lon);
        Self {
            south: (lat - lat_pad) as f32,
            north: (lat + lat_pad) as f32,
            longitude_intervals: [-360.0, 0.0, 360.0].map(|shift| {
                [
                    (lon + shift - lon_pad) as f32,
                    (lon + shift + lon_pad) as f32,
                ]
            }),
        }
    }

    /// Raw aggregate [south, west, north, east], not a single short arc.
    pub fn intersects_bbox(&self, bbox: [f64; 4]) -> bool {
        self.intersects_latitude(bbox[0], bbox[2])
            // A wide min/max can combine unrelated segments or cross the seam;
            // only the individual segment endpoints can disambiguate it.
            && (bbox[3] - bbox[1] >= 180.0 || self.intersects_longitude(bbox[1], bbox[3]))
    }

    pub fn intersects_segment(&self, start: [f32; 2], end: [f32; 2]) -> bool {
        let [west, east] = airborne_longitude_interval(start[1], end[1]);
        self.intersects_latitude(
            f64::from(start[0].min(end[0])),
            f64::from(start[0].max(end[0])),
        ) && self.intersects_longitude(west, east)
    }

    fn intersects_latitude(&self, south: f64, north: f64) -> bool {
        north >= f64::from(self.south) && south <= f64::from(self.north)
    }

    fn intersects_longitude(&self, west: f64, east: f64) -> bool {
        self.longitude_intervals
            .iter()
            .any(|[left, right]| east >= f64::from(*left) && west <= f64::from(*right))
    }
}

fn airborne_longitude_interval(start: f32, end: f32) -> [f64; 2] {
    let start = f64::from(start);
    let end = start + grid::geo::wrapped_longitude_delta(start, f64::from(end));
    [start.min(end), start.max(end)]
}

/// Inputs are the exact decoded f32 endpoints used by airborne::scatter.
pub fn airborne_support_cells(start: [f32; 2], end: [f32; 2]) -> Option<BoundedSquares> {
    if [start, end].into_iter().any(|[lat, lon]| {
        !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
    }) {
        return None;
    }
    let reach = AIRCRAFT_MAX_HORIZONTAL_REACH_M;
    let lat_pad = meters_to_lat_deg(reach);
    // The receiver envelope is cast to f32 before comparison. Adjacent f32
    // values conservatively enclose its rounding bin, without an epsilon.
    let south = (f64::from(start[0].min(end[0]).next_down()) - lat_pad)
        .next_down()
        .max(-90.0);
    let north = (f64::from(start[0].max(end[0]).next_up()) + lat_pad)
        .next_up()
        .min(90.0);
    let lon_pad = meters_to_lon_deg(south.abs().max(north.abs()), reach).next_up();
    let [west, east] = airborne_longitude_interval(start[1], end[1]);
    let west = (f64::from((west as f32).next_down()) - lon_pad).next_down();
    let east = (f64::from((east as f32).next_up()) + lon_pad).next_up();
    BoundedSquares::from_degrees(south, west, north, east)
}

/// Final stored bucket centroid/rep_len, not a raw flight or clipped z15 transit.
pub fn cruise_support_cells(lat: f64, lon: f64, rep_len_m: f32) -> Option<BoundedSquares> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !rep_len_m.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return None;
    }
    let radius = AIRCRAFT_MAX_HORIZONTAL_REACH_M
        + f64::from(rep_len_m).max(crate::compute::aircraft_v6::cruise::SLANT_FLOOR_M) * 0.5;
    let lat_pad = (radius / crate::constants::M_PER_DEG_LAT).next_up();
    let south = (lat - lat_pad).next_down().max(-90.0);
    let north = (lat + lat_pad).next_up().min(90.0);
    // The centroid gate scales longitude at the RECEIVER latitude. Use its
    // poleward extreme, including the same cosine floor, to enclose every disk.
    let lon_pad = (radius
        / crate::constants::m_per_deg_lon(south.abs().max(north.abs()).to_radians()))
    .next_up();
    BoundedSquares::from_degrees(
        south,
        (lon - lon_pad).next_down(),
        north,
        (lon + lon_pad).next_up(),
    )
}

#[cfg(test)]
mod tests;
