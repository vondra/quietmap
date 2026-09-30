//! Terrain height under a point from the dev4 z9 raster tree (`dem.u16le`, one-arc-second nodes,
//! EGM2008 heights): the nearest node, with dev4 Stage 1's arithmetic. A node is read with one
//! positioned read, without read-ahead (a flight uses a few nodes per page): the page cache keeps
//! what days share and nothing terrain-sized is mapped into the process. An empty file is verified
//! ocean; a missing file or a node without data fails the day, never a guessed height.

use crate::dev4::{Dev4, Square, z9_raster_window};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, RwLock};
use tiles::geo::{Mercator, TileId};
use tiles::terrain::{HEIGHT_MISSING, NODES_PER_DEGREE, Window, height_m_of_code};

/// One square's heights: verified ocean (0 m) or its node file.
pub enum SquareHeights {
    Ocean,
    Nodes { window: Window, file: File },
}

pub struct TerrainHeights {
    rasters: Dev4,
    squares: RwLock<HashMap<Square, Arc<SquareHeights>>>,
}

/// The square a flight sampled last: consecutive samples mostly share it.
pub type LastSquare = Option<(Square, Arc<SquareHeights>)>;

impl TerrainHeights {
    pub fn new(rasters: Dev4) -> Self {
        TerrainHeights {
            rasters,
            squares: RwLock::new(HashMap::new()),
        }
    }

    /// Terrain height in metres at the node nearest to (lat, lon).
    pub fn height_m(&self, lat: f64, lon: f64, last: &mut LastSquare) -> Result<f32, String> {
        let tile = TileId::containing(Mercator::from_degrees(lat, lon));
        let square = Square {
            x: tile.x >> 3,
            y: tile.y >> 3,
        };
        let heights = match last {
            Some((cached, heights)) if *cached == square => heights.clone(),
            _ => {
                let heights = self.square(square)?;
                *last = Some((square, heights.clone()));
                heights
            }
        };
        let SquareHeights::Nodes { window, file } = heights.as_ref() else {
            return Ok(0.0);
        };
        let (row, column) = nearest_node(*window, lat, lon)
            .ok_or_else(|| format!("{lat} {lon} lies outside square {square:?}"))?;
        let mut code = [0u8; 2];
        file.read_exact_at(
            &mut code,
            2 * (row * window.columns as usize + column) as u64,
        )
        .map_err(|error| format!("terrain of square {square:?}: {error}"))?;
        match u16::from_le_bytes(code) {
            HEIGHT_MISSING => Err(format!("no terrain height at {lat} {lon}")),
            code => Ok(height_m_of_code(code) as f32),
        }
    }

    fn square(&self, square: Square) -> Result<Arc<SquareHeights>, String> {
        if let Some(heights) = self
            .squares
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&square)
        {
            return Ok(heights.clone());
        }
        let path = self.rasters.raster_file(square, "dem.u16le");
        let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let length = file
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?
            .len();
        let window = z9_raster_window(square);
        let heights = if length == 0 {
            SquareHeights::Ocean
        } else if length == 2 * window.node_count() as u64 {
            // SAFETY: plain advice on an open descriptor; no memory is touched.
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_RANDOM) };
            SquareHeights::Nodes { window, file }
        } else {
            return Err(format!("{}: {length} bytes for {window:?}", path.display()));
        };
        let heights = Arc::new(heights);
        self.squares
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(square, heights.clone());
        Ok(heights)
    }
}

/// dev4's nearest-node arithmetic (`grid::raster::sample_position`): fractions within the whole
/// degree first, so a large global node origin never eats the bits that decide half-node ties.
fn nearest_node(window: Window, lat: f64, lon: f64) -> Option<(usize, usize)> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    let lon = if (-180.0..180.0).contains(&lon) {
        lon
    } else {
        (lon + 180.0).rem_euclid(360.0) - 180.0
    };
    let degree_lat = (lat.floor() as i32).min(89);
    let degree_lon = lon.floor() as i32;
    let row_in_degree = (1.0 - (lat - f64::from(degree_lat))) * f64::from(NODES_PER_DEGREE);
    let column_in_degree = (lon - f64::from(degree_lon)) * f64::from(NODES_PER_DEGREE);
    let row =
        window.north_node - (degree_lat + 1) * NODES_PER_DEGREE + row_in_degree.round() as i32;
    let column = degree_lon * NODES_PER_DEGREE - window.west_node + column_in_degree.round() as i32;
    ((0..window.rows as i32).contains(&row) && (0..window.columns as i32).contains(&column))
        .then_some((row as usize, column as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_node_rounds_within_the_degree() {
        let window = z9_raster_window(Square { x: 276, y: 173 });
        // Prague Vinohrady, 50.075 N 14.44 E: nodes 180,270 N and 51,984 E.
        let (row, column) = nearest_node(window, 50.075, 14.44).unwrap();
        assert_eq!(window.north_node - row as i32, 180_270);
        assert_eq!(window.west_node + column as i32, 51_984);
        let (row, _) = nearest_node(window, 50.075 + 0.4 / 3600.0, 14.44).unwrap();
        assert_eq!(window.north_node - row as i32, 180_270);
        let (row, _) = nearest_node(window, 50.075 + 0.6 / 3600.0, 14.44).unwrap();
        assert_eq!(window.north_node - row as i32, 180_271);
        assert!(nearest_node(window, 48.0, 14.44).is_none());
    }

    /// A square's file: empty is verified ocean, a node reads its code, a node without data or
    /// a missing or truncated file fails, never a guessed height.
    #[test]
    fn terrain_is_read_or_refused() {
        let root = crate::aircraft::archive::tests::scratch_directory("dem");
        let prague = Square { x: 276, y: 173 };
        let dev4 = Dev4 {
            prepared: Default::default(),
            rasters: root.clone(),
        };
        let file = dev4.raster_file(prague, "dem.u16le");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let window = z9_raster_window(prague);
        let mut codes = vec![0u8; 2 * window.node_count()];
        let (row, column) = nearest_node(window, 50.075, 14.44).unwrap();
        let at = 2 * (row * window.columns as usize + column);
        codes[at..at + 2].copy_from_slice(&4_300u16.to_le_bytes());
        codes[at + 2..at + 4].copy_from_slice(&HEIGHT_MISSING.to_le_bytes());
        std::fs::write(&file, &codes).unwrap();
        let terrain = TerrainHeights::new(dev4);
        let mut last = None;
        assert_eq!(terrain.height_m(50.075, 14.44, &mut last), Ok(360.0));
        assert!(
            terrain
                .height_m(50.075, 14.44 + 1.0 / 3600.0, &mut last)
                .is_err()
        );
        let ocean = TerrainHeights::new(Dev4 {
            prepared: Default::default(),
            rasters: root.clone(),
        });
        std::fs::write(&file, b"").unwrap();
        assert_eq!(ocean.height_m(50.075, 14.44, &mut None), Ok(0.0));
        assert!(ocean.height_m(48.0, 16.0, &mut None).is_err(), "no file");
        std::fs::write(&file, &codes[..100]).unwrap();
        let truncated = TerrainHeights::new(Dev4 {
            prepared: Default::default(),
            rasters: root,
        });
        assert!(truncated.height_m(50.075, 14.44, &mut None).is_err());
    }
}
