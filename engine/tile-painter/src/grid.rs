//! Web Mercator (EPSG:3857) XYZ tile geometry.
//!
//! Compute and output share the same Mercator pixel lattice: a receiver
//! is the centre of one tile pixel at the build zoom ([`tile_store::PUBLISHED_BASE_ZOOM`]
//! with 512-px tiles — 6.1 m at Praha lat, half the 12.3 m of the retired z12 base),
//! the tile pixel is the receiver, and there is no projection or resampling step
//! between them.

use std::f64::consts::PI;

/// Tile pixel side at zoom z. 512 since the 2026-07 shift — must stay in
/// lockstep with raster-reader's copy and the CUDA kernels' TPX (see
/// noise-gpu/kernels/*.cu); the physical pixel size is TILE_PX-invariant
/// only because EQUATORIAL_M_PER_PX_Z0 scales inversely with it.
pub const TILE_PX: usize = 512;

/// Tile bbox in lat/lon (EPSG:4326) for a Mercator z/x/y tile.
#[derive(Debug, Clone, Copy)]
pub struct TileBbox {
    pub west_lon: f64,
    pub east_lon: f64,
    pub north_lat: f64,
    pub south_lat: f64,
}

#[inline]
pub fn tile_bbox(z: u8, x: u32, y: u32) -> TileBbox {
    let n = (1u64 << z) as f64;
    let xf = x as f64;
    let yf = y as f64;
    let west_lon = xf / n * 360.0 - 180.0;
    let east_lon = (xf + 1.0) / n * 360.0 - 180.0;
    let north_lat = (PI * (1.0 - 2.0 * yf / n)).sinh().atan().to_degrees();
    let south_lat = (PI * (1.0 - 2.0 * (yf + 1.0) / n))
        .sinh()
        .atan()
        .to_degrees();
    TileBbox {
        west_lon,
        east_lon,
        north_lat,
        south_lat,
    }
}

#[inline]
pub fn lat_lon_to_tile(z: u8, lat: f64, lon: f64) -> (u32, u32) {
    let n = (1u64 << z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
    let lat_rad = lat.to_radians();
    let merc = (lat_rad.tan() + 1.0 / lat_rad.cos()).ln();
    let y = ((1.0 - merc / PI) / 2.0 * n).floor() as u32;
    let cap = (n as u32).saturating_sub(1);
    (x.min(cap), y.min(cap))
}

/// Latitude of the centre of pixel row `py` (0..TILE_PX) inside a tile.
#[inline]
pub fn pixel_lat(bbox: &TileBbox, py: u32) -> f64 {
    let n = TILE_PX as f64;
    let north_merc = mercator_y_from_lat(bbox.north_lat);
    let south_merc = mercator_y_from_lat(bbox.south_lat);
    let frac = (py as f64 + 0.5) / n;
    let merc = north_merc + frac * (south_merc - north_merc);
    lat_from_mercator_y(merc)
}

/// Longitude of the centre of pixel column `px` (0..TILE_PX) inside a tile.
#[inline]
pub fn pixel_lon(bbox: &TileBbox, px: u32) -> f64 {
    let n = TILE_PX as f64;
    let frac = (px as f64 + 0.5) / n;
    bbox.west_lon + frac * (bbox.east_lon - bbox.west_lon)
}

#[inline]
fn mercator_y_from_lat(lat: f64) -> f64 {
    let lat_rad = lat.to_radians();
    (lat_rad.tan() + 1.0 / lat_rad.cos()).ln()
}

#[inline]
fn lat_from_mercator_y(merc: f64) -> f64 {
    (2.0 * merc.exp().atan() - PI / 2.0).to_degrees()
}

/// Range of tile indices at the given zoom that fully cover the bbox
/// specified in lat/lon. Returned ranges are inclusive on the lower
/// end and exclusive on the upper end, matching `std::ops::Range`.
pub fn tile_range(
    zoom: u8,
    south_lat: f64,
    west_lon: f64,
    north_lat: f64,
    east_lon: f64,
) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let (x_min, y_max) = lat_lon_to_tile(zoom, south_lat, west_lon);
    let (x_max, y_min) = lat_lon_to_tile(zoom, north_lat, east_lon);
    (x_min..x_max + 1, y_min..y_max + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_bbox_round_trips_to_pixel() {
        let bbox = tile_bbox(15, 17970, 11293);
        let (x, y) = lat_lon_to_tile(
            15,
            (bbox.north_lat + bbox.south_lat) * 0.5,
            (bbox.west_lon + bbox.east_lon) * 0.5,
        );
        assert_eq!((x, y), (17970, 11293));
    }

    #[test]
    fn pixel_lat_lon_are_inside_their_tile() {
        let bbox = tile_bbox(15, 17970, 11293);
        for py in 0..TILE_PX as u32 {
            let lat = pixel_lat(&bbox, py);
            assert!(lat <= bbox.north_lat && lat >= bbox.south_lat);
        }
        for px in 0..TILE_PX as u32 {
            let lon = pixel_lon(&bbox, px);
            assert!(lon >= bbox.west_lon && lon <= bbox.east_lon);
        }
    }
}
