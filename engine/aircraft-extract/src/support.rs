//! Stored airborne geometry: the z30-quantized, Mercator-clamped endpoints the popup decodes.

use noise_compute::emission::aircraft::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M;

use crate::flight::FlightSegment;
use crate::geo::flat_dist;

/// The f32 endpoint the popup reads back for a source coordinate: encoded to
/// the z30 grid (latitude clamped to the Mercator limit) and decoded again.
pub fn airborne_decoded_endpoint(lat: f32, lon: f32) -> Option<[f32; 2]> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    let (gx, gy) = grid::lonlat_to_grid(f64::from(lon), f64::from(lat));
    let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
    Some([lat as f32, lon as f32])
}

/// Both stored endpoints of a segment.
pub fn airborne_stored_endpoints(segment: &FlightSegment) -> Option<([f32; 2], [f32; 2])> {
    Some((
        airborne_decoded_endpoint(segment.start_lat, segment.start_lon)?,
        airborne_decoded_endpoint(segment.end_lat, segment.end_lon)?,
    ))
}

/// Length of the stored geometry — what the reader pad and the length cap
/// bound. Near the poles the Mercator clamp can stretch a short source chord
/// to hundreds of kilometres, so the source `length_m` is not this number.
pub fn airborne_stored_length_m(segment: &FlightSegment) -> Option<f32> {
    let (start, end) = airborne_stored_endpoints(segment)?;
    Some(flat_dist(start[0], start[1], end[0], end[1]))
}

pub fn airborne_stored_length_within_cap(segment: &FlightSegment) -> bool {
    airborne_stored_length_m(segment)
        .is_some_and(|length| length <= AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M)
}

#[cfg(test)]
mod tests;
