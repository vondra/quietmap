//! Web Mercator tile bbox in WGS84 degrees and the side of one painted tile.

/// Side length of one output tile in receiver pixels. 512 since the 2026-07
/// shift — lockstep with tile-painter grid.rs and the CUDA
/// `QUIETMAP_TILE_PIXEL_SIDE` (`relevant-source-gpu/build.rs` reads it from
/// this file).
pub const TILE_PX: usize = 512;

/// Mercator tile bbox in lat/lon (EPSG:4326).
#[derive(Debug, Clone, Copy)]
pub struct TileBbox {
    pub west_lon: f64,
    pub east_lon: f64,
    pub north_lat: f64,
    pub south_lat: f64,
}

impl TileBbox {
    pub fn from_xyz(zoom: u8, x: u32, y: u32) -> Self {
        use std::f64::consts::PI;
        let n = (1u64 << zoom) as f64;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn praha_tile_bbox_round_trips() {
        // Praha LKPR (50.10°N, 14.26°E) → z=12 tile (2210, 1386).
        let bbox = TileBbox::from_xyz(12, 2210, 1386);
        assert!(
            bbox.south_lat > 49.9 && bbox.north_lat < 50.3,
            "lat range {:.3}..{:.3}",
            bbox.south_lat,
            bbox.north_lat
        );
        assert!(
            bbox.west_lon > 14.0 && bbox.east_lon < 14.5,
            "lon range {:.3}..{:.3}",
            bbox.west_lon,
            bbox.east_lon
        );
    }
}
