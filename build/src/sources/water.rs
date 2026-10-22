//! ESA WorldCover's permanent water (`fetch/water.py`): each 3 arc-second cell's percent of water, a
//! 3 x 3 degree tile a file named by its south-west corner; a tile the grid lacks has no land: open
//! sea, all water.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A tile's side in cells (3 degrees of 1/1,200).
const CELLS: usize = 3_600;
const CELLS_PER_DEGREE: f64 = 1_200.0;
const TILE_DEGREES: i32 = 3;

/// The tiles of the water directory: WorldCover's grid names those it has.
pub struct Water {
    directory: PathBuf,
    tiles: HashSet<String>,
}

/// A square's tiles, read once each as its converters ask.
pub struct WaterTiles<'a> {
    water: &'a Water,
    read: HashMap<(i32, i32), Option<Vec<u8>>>,
}

fn tile_name((south, west): (i32, i32)) -> String {
    let (ns, ew) = (
        if south < 0 { 'S' } else { 'N' },
        if west < 0 { 'W' } else { 'E' },
    );
    format!("{ns}{:02}{ew}{:03}", south.abs(), west.abs())
}

fn corner_of(lat: f64, lon: f64) -> (i32, i32) {
    let corner = |degrees: f64| (degrees / f64::from(TILE_DEGREES)).floor() as i32 * TILE_DEGREES;
    (corner(lat), corner(lon))
}

impl Water {
    /// The directory `fetch/water.py` wrote, with WorldCover's grid beside the tiles.
    pub fn open(directory: &Path) -> Result<Self, String> {
        let grid = directory.join("esa_worldcover_grid.geojson");
        let text = std::fs::read_to_string(&grid)
            .map_err(|error| format!("{}: {error}", grid.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", grid.display()))?;
        let tiles: HashSet<String> = (json["features"].as_array().into_iter().flatten())
            .filter_map(|feature| feature["properties"]["ll_tile"].as_str().map(String::from))
            .collect();
        if tiles.is_empty() {
            return Err(format!("{}: no tiles", grid.display()));
        }
        Ok(Water {
            directory: directory.to_path_buf(),
            tiles,
        })
    }

    pub fn tiles(&self) -> WaterTiles<'_> {
        WaterTiles {
            water: self,
            read: HashMap::new(),
        }
    }
}

impl WaterTiles<'_> {
    /// The share of water (0-1) of the cell holding a point.
    pub fn share(&mut self, lat: f64, lon: f64) -> Result<f64, String> {
        let corner = corner_of(lat, lon);
        if !self.read.contains_key(&corner) {
            let name = tile_name(corner);
            let cells = if self.water.tiles.contains(&name) {
                let path = self.water.directory.join(format!("{name}.water"));
                let file = std::fs::File::open(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let cells = zstd::decode_all(file)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if cells.len() != CELLS * CELLS {
                    return Err(format!("{}: {} bytes", path.display(), cells.len()));
                }
                Some(cells)
            } else {
                None
            };
            self.read.insert(corner, cells);
        }
        let Some(cells) = &self.read[&corner] else {
            return Ok(1.0);
        };
        let index = |degrees: f64| {
            (degrees * CELLS_PER_DEGREE)
                .floor()
                .clamp(0.0, (CELLS - 1) as f64) as usize
        };
        let row = index(f64::from(corner.0 + TILE_DEGREES) - lat);
        let column = index(lon - f64::from(corner.1));
        Ok(f64::from(cells[row * CELLS + column]) / 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_named_by_their_south_west_corner_as_worldcover_names_them() {
        assert_eq!(tile_name(corner_of(39.5, -0.5)), "N39W003");
        assert_eq!(tile_name(corner_of(-53.2, 169.5)), "S54E168");
        assert_eq!(tile_name(corner_of(0.5, -180.0)), "N00W180");
    }

    /// A tile of the grid reads its cells, row 0 at its north edge; one the grid lacks is sea.
    #[test]
    fn a_point_reads_its_cell_and_the_open_sea_is_water() {
        let directory = std::env::temp_dir().join(format!("water-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("esa_worldcover_grid.geojson"),
            r#"{"features": [{"properties": {"ll_tile": "N39W003"}}]}"#,
        )
        .unwrap();
        let mut cells = vec![0u8; CELLS * CELLS];
        // The cell of 41.9996 N 2.9996 W (the tile's north-west corner) and of 39.0004 N 0.0004 W.
        cells[0] = 100;
        cells[CELLS * CELLS - 1] = 40;
        let file = std::fs::File::create(directory.join("N39W003.water")).unwrap();
        zstd::stream::copy_encode(&cells[..], file, 3).unwrap();
        let water = Water::open(&directory).unwrap();
        let mut tiles = water.tiles();
        assert_eq!(tiles.share(41.9996, -2.9996).unwrap(), 1.0);
        assert_eq!(tiles.share(39.0004, -0.0004).unwrap(), 0.4);
        assert_eq!(tiles.share(40.5, -1.5).unwrap(), 0.0);
        assert_eq!(tiles.share(10.0, -30.0).unwrap(), 1.0);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
