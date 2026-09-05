//! Publication support uses the same z30-to-f32 coordinates as the actual writer and popup.

use noise_compute::emission::aircraft::airborne_support_cells;

pub fn airborne_decoded_endpoint(lat: f32, lon: f32) -> Option<[f32; 2]> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    let (gx, gy) = grid::lonlat_to_grid(f64::from(lon), f64::from(lat));
    let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
    Some([lat as f32, lon as f32])
}

pub fn airborne_segment_support(
    segment: &crate::flight::FlightSegment,
) -> Option<grid::bounds::BoundedSquares> {
    airborne_support_cells(
        airborne_decoded_endpoint(segment.start_lat, segment.start_lon)?,
        airborne_decoded_endpoint(segment.end_lat, segment.end_lon)?,
    )
}

#[cfg(test)]
mod tests;
