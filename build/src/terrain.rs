//! Builds `terrain` tiles from the dev4 raster tree: every node of a tile's window comes from the
//! z9 square that owns the node's position, so neighbouring tiles share identical seam nodes even
//! where two dev4 squares disagree (up to 0.2 m).

pub mod national;

use crate::dev4::{Dev4, Square, z9_raster_window};
use crate::output::write_tile;
use national::NationalHeights;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use tiles::Kind;
use tiles::geo::{Mercator, TileId};
use tiles::terrain::{HEIGHT_MISSING, NODES_PER_DEGREE, Node, Window, encode, height_m_of_code};

/// One dev4 square's four channels; an empty square is verified ocean.
struct SquareRaster {
    window: Window,
    dem: Vec<u8>,
    imd: Vec<u8>,
    forest: Vec<u8>,
    canopy: Vec<u8>,
}

const OCEAN_NODE: Node = Node {
    height_code: 2_500,
    impervious_percent: 100,
    forest_percent: 0,
    canopy_m: 0,
};

impl SquareRaster {
    fn load(dev4: &Dev4, square: Square) -> Result<Self, String> {
        let window = z9_raster_window(square);
        let read = |name: &str, bytes_per_node: usize| -> Result<Vec<u8>, String> {
            let path = dev4.raster_file(square, name);
            let bytes =
                std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            if !bytes.is_empty() && bytes.len() != window.node_count() * bytes_per_node {
                return Err(format!(
                    "{}: {} bytes for {:?}",
                    path.display(),
                    bytes.len(),
                    window
                ));
            }
            Ok(bytes)
        };
        Ok(SquareRaster {
            window,
            dem: read("dem.u16le", 2)?,
            imd: read("imd.u8", 1)?,
            forest: read("forest.u8", 1)?,
            canopy: read("canopy.u8", 1)?,
        })
    }

    /// The node at global indices, or `None` outside this square's window.
    fn node(&self, north_index: i32, east_index: i32) -> Option<Node> {
        let row = self.window.north_node - north_index;
        let column = east_index - self.window.west_node;
        if row < 0
            || column < 0
            || row as u32 >= self.window.rows
            || column as u32 >= self.window.columns
        {
            return None;
        }
        if self.dem.is_empty() {
            return Some(OCEAN_NODE);
        }
        let at = row as usize * self.window.columns as usize + column as usize;
        Some(Node {
            height_code: u16::from_le_bytes([self.dem[2 * at], self.dem[2 * at + 1]]),
            impervious_percent: self.imd[at],
            forest_percent: self.forest[at],
            // dev4's canopy channel is empty where GLAD has no data: no trees.
            canopy_m: self.canopy.get(at).copied().unwrap_or(0),
        })
    }
}

/// The z9 square owning a lattice node's position (XYZ floor rule).
fn owner_square(north_index: i32, east_index: i32) -> Square {
    let lat = f64::from(north_index) / f64::from(NODES_PER_DEGREE);
    let lon = f64::from(east_index) / f64::from(NODES_PER_DEGREE);
    let tile = TileId::containing(Mercator::from_degrees(lat, lon));
    Square {
        x: tile.x >> 3,
        y: tile.y >> 3,
    }
}

/// A node with the national models laid over its height (the first that has a say).
fn with_national(mut node: Node, north: i32, east: i32, national: &[NationalHeights]) -> Node {
    if node.height_code == HEIGHT_MISSING || node == OCEAN_NODE {
        return node;
    }
    if let Some((height_m, weight)) = national.iter().find_map(|model| model.at(north, east)) {
        let base_m = height_m_of_code(node.height_code);
        let blended_m = base_m + weight * (height_m - base_m);
        node.height_code = ((blended_m + 500.0) * 5.0)
            .round()
            .clamp(0.0, f64::from(HEIGHT_MISSING - 1)) as u16;
    }
    node
}

/// Writes the terrain tiles of `squares`, the `national` models laid over dev4's heights; returns
/// the number written (all-ocean tiles are absent).
pub fn build(
    dev4: &Dev4,
    squares: &[Square],
    national: &[NationalHeights],
    out: &Path,
) -> Result<usize, String> {
    let mut written = 0;
    for &square in squares {
        let mut rasters = HashMap::new();
        for neighbour in square.with_neighbours() {
            rasters.insert(neighbour, SquareRaster::load(dev4, neighbour)?);
        }
        let own = &rasters[&square];
        let children: Vec<TileId> = (0..64)
            .map(|i| TileId {
                x: square.x * 8 + i % 8,
                y: square.y * 8 + i / 8,
            })
            .collect();
        written += children
            .par_iter()
            .map(|&tile| {
                let window = Window::of_tile(tile);
                let mut nodes = Vec::with_capacity(window.node_count());
                for row in 0..window.rows as i32 {
                    for column in 0..window.columns as i32 {
                        let (north, east) = (window.north_node - row, window.west_node + column);
                        let node = rasters
                            .get(&owner_square(north, east))
                            .and_then(|raster| raster.node(north, east))
                            .or_else(|| own.node(north, east))
                            .ok_or_else(|| {
                                format!("{tile:?}: node {north}/{east} outside square {square:?}")
                            })?;
                        nodes.push(with_national(node, north, east, national));
                    }
                }
                if nodes.iter().all(|node| *node == OCEAN_NODE) {
                    return Ok(0);
                }
                write_tile(out, tile, Kind::Terrain, &encode(window, &nodes))?;
                Ok(1)
            })
            .collect::<Result<Vec<usize>, String>>()?
            .into_iter()
            .sum::<usize>();
    }
    Ok(written)
}
