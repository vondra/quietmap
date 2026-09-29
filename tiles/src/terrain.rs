//! `terrain` tiles: the one-arc-second lattice window of a z12 tile with height, imperviousness
//! and forest cover per node. The window brackets the tile with floor/ceil node edges, so every
//! point of the tile interpolates from its own file and neighbours share their seam nodes.

use crate::FormatError;
use crate::geo::{Mercator, TILES_PER_AXIS, TileId};

/// Lattice nodes per degree of latitude and of longitude.
pub const NODES_PER_DEGREE: i32 = 3600;
/// Height code of a node without data; a sample touching one is refused, never guessed.
pub const HEIGHT_MISSING: u16 = u16::MAX;
/// Imperviousness and forest cover above this percentage mark a node without data.
pub const PERCENT_MAX: u8 = 100;
const MAGIC: &[u8; 8] = b"qmterr1\n";
const HEADER_BYTES: usize = 24;
const NODE_BYTES: usize = 4;

/// Metres of a height code: -500 m + code / 5 (0.2 m steps up to 12,606.8 m), EGM2008.
pub fn height_m_of_code(code: u16) -> f64 {
    -500.0 + f64::from(code) / 5.0
}

/// A rectangle of lattice nodes: row 0 is the northernmost, column 0 the westernmost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Global node index of row 0: latitude = north_node / 3600.
    pub north_node: i32,
    /// Global node index of column 0: longitude = west_node / 3600.
    pub west_node: i32,
    pub rows: u32,
    pub columns: u32,
}

impl Window {
    /// The floor/ceil node window of a tile.
    pub fn of_tile(tile: TileId) -> Self {
        let axis = i64::from(TILES_PER_AXIS);
        let longitude_nodes = 360 * i64::from(NODES_PER_DEGREE);
        // Longitude edges have denominator 4096: exact integer floor and ceil.
        let west = i64::from(tile.x) * longitude_nodes / axis - longitude_nodes / 2;
        let east =
            ((i64::from(tile.x) + 1) * longitude_nodes + axis - 1) / axis - longitude_nodes / 2;
        let edge_latitude_nodes = |y: u32| {
            let mercator =
                std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / f64::from(TILES_PER_AXIS));
            mercator.sinh().atan().to_degrees() * f64::from(NODES_PER_DEGREE)
        };
        let north = edge_latitude_nodes(tile.y).ceil() as i32;
        let south = edge_latitude_nodes(tile.y + 1).floor() as i32;
        Window {
            north_node: north,
            west_node: west as i32,
            rows: (north - south + 1) as u32,
            columns: (east - west + 1) as u32,
        }
    }

    pub fn node_count(self) -> usize {
        self.rows as usize * self.columns as usize
    }
}

/// One node as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    /// See [`height_m_of_code`]; [`HEIGHT_MISSING`] marks a node without data.
    pub height_code: u16,
    /// Imperviousness, 0-100 % (above: no data): the ground factor is G = 1 - imperviousness.
    pub impervious_percent: u8,
    /// Canopy cover, 0-100 % (above: no data).
    pub forest_percent: u8,
}

/// Ground under one point: bilinear height and ground factor, nearest forest cover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    pub height_m: f64,
    /// CNOSSOS ground factor G in [0, 1]: 0 hard, 1 soft.
    pub ground_factor: f64,
    /// Canopy cover fraction in [0, 1].
    pub forest_cover: f64,
}

/// An absent terrain file in a complete release: sea level, hard water, no forest.
pub const OCEAN: GroundSample = GroundSample {
    height_m: 0.0,
    ground_factor: 0.0,
    forest_cover: 0.0,
};

/// The bytes of a terrain file: header, then row-major nodes of 4 bytes (height code u16 LE,
/// imperviousness u8, forest cover u8).
pub fn encode(window: Window, nodes: &[Node]) -> Vec<u8> {
    assert_eq!(nodes.len(), window.node_count());
    let mut bytes = Vec::with_capacity(HEADER_BYTES + NODE_BYTES * nodes.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&window.north_node.to_le_bytes());
    bytes.extend_from_slice(&window.west_node.to_le_bytes());
    bytes.extend_from_slice(&window.rows.to_le_bytes());
    bytes.extend_from_slice(&window.columns.to_le_bytes());
    for node in nodes {
        bytes.extend_from_slice(&node.height_code.to_le_bytes());
        bytes.push(node.impervious_percent);
        bytes.push(node.forest_percent);
    }
    bytes
}

/// A parsed terrain file borrowing its bytes.
pub struct Terrain<'a> {
    window: Window,
    nodes: &'a [u8],
    /// Web Mercator y (z12 tile units) of every node row: a sample finds its rows without a
    /// transcendental function. Between two rows 31 m apart, latitude is linear in y to within
    /// 1e-6 of the row spacing (0.03 mm).
    row_mercator_y: Vec<f64>,
}

impl<'a> Terrain<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("terrain: bad magic"));
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let window = Window {
            north_node: word(8) as i32,
            west_node: word(12) as i32,
            rows: word(16),
            columns: word(20),
        };
        if bytes.len() != HEADER_BYTES + NODE_BYTES * window.node_count() {
            return Err(FormatError("terrain: length does not match the window"));
        }
        let row_mercator_y = (0..window.rows)
            .map(|row| {
                let lat = f64::from(window.north_node - row as i32) / f64::from(NODES_PER_DEGREE);
                Mercator::from_degrees(lat, 0.0).y
            })
            .collect();
        Ok(Terrain {
            window,
            nodes: &bytes[HEADER_BYTES..],
            row_mercator_y,
        })
    }

    pub fn window(&self) -> Window {
        self.window
    }

    pub fn node(&self, row: u32, column: u32) -> Node {
        let at = NODE_BYTES * (row as usize * self.window.columns as usize + column as usize);
        let record = &self.nodes[at..at + NODE_BYTES];
        Node {
            height_code: u16::from_le_bytes([record[0], record[1]]),
            impervious_percent: record[2],
            forest_percent: record[3],
        }
    }

    /// The ground at a point, or `None` outside the window or where a needed node has no data:
    /// height and ground factor bilinear, forest cover from the nearest node.
    pub fn sample(&self, position: Mercator) -> Option<GroundSample> {
        let columns_per_unit = f64::from(360 * NODES_PER_DEGREE) / f64::from(TILES_PER_AXIS);
        let column_position = position.x * columns_per_unit
            - f64::from(180 * NODES_PER_DEGREE)
            - f64::from(self.window.west_node);
        // A point on the window's west edge may land a rounding error outside it.
        let column_position = if column_position > -1e-6 {
            column_position.max(0.0)
        } else {
            column_position
        };
        let column = column_position.floor();
        let column_fraction = column_position - column;
        let rows = &self.row_mercator_y;
        let spacing = (rows[rows.len() - 1] - rows[0]) / (rows.len() - 1).max(1) as f64;
        let mut row = (((position.y - rows[0]) / spacing).floor() as isize)
            .clamp(0, rows.len() as isize - 1) as usize;
        while row > 0 && position.y < rows[row] {
            row -= 1;
        }
        while row + 1 < rows.len() && position.y >= rows[row + 1] {
            row += 1;
        }
        if position.y < rows[0] - 1e-9 || column < 0.0 {
            return None;
        }
        let row_fraction = if row + 1 < rows.len() {
            (position.y - rows[row]) / (rows[row + 1] - rows[row])
        } else {
            0.0
        };
        let (row, column) = (row as u32, column as u32);
        let next_row = row + u32::from(row_fraction > 0.0);
        let next_column = column + u32::from(column_fraction > 0.0);
        if next_row >= self.window.rows || next_column >= self.window.columns {
            return None;
        }
        let corners = [
            self.node(row, column),
            self.node(row, next_column),
            self.node(next_row, column),
            self.node(next_row, next_column),
        ];
        let nearest =
            corners[usize::from(row_fraction >= 0.5) * 2 + usize::from(column_fraction >= 0.5)];
        if nearest.forest_percent > PERCENT_MAX
            || corners.iter().any(|node| {
                node.height_code == HEIGHT_MISSING || node.impervious_percent > PERCENT_MAX
            })
        {
            return None;
        }
        let weights = [
            (1.0 - row_fraction) * (1.0 - column_fraction),
            (1.0 - row_fraction) * column_fraction,
            row_fraction * (1.0 - column_fraction),
            row_fraction * column_fraction,
        ];
        let (mut height, mut impervious) = (0.0, 0.0);
        for (node, weight) in corners.iter().zip(weights) {
            height += weight * height_m_of_code(node.height_code);
            impervious += weight * f64::from(node.impervious_percent);
        }
        Some(GroundSample {
            height_m: height,
            ground_factor: 1.0 - impervious / 100.0,
            forest_cover: f64::from(nearest.forest_percent) / 100.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prague_tile_window_brackets_the_tile() {
        let tile = TileId::containing(Mercator::from_degrees(50.07553, 14.43781));
        let window = Window::of_tile(tile);
        assert!(
            (204..=207).contains(&window.rows) && (318..=320).contains(&window.columns),
            "{window:?}"
        );
        let (north, west) = tile.to_mercator([-16_384, -16_384]).to_degrees();
        assert!(
            f64::from(window.north_node) >= north * 3600.0
                && f64::from(window.west_node) <= west * 3600.0
        );
    }

    #[test]
    fn neighbours_share_their_seam_nodes() {
        let tile = TileId { x: 2212, y: 1387 };
        let (here, east, south) = (
            Window::of_tile(tile),
            Window::of_tile(TileId { x: 2213, ..tile }),
            Window::of_tile(TileId { y: 1388, ..tile }),
        );
        assert!(here.west_node + here.columns as i32 > east.west_node);
        assert!(here.north_node - (here.rows as i32) < south.north_node);
    }

    #[test]
    fn samples_interpolate_heights_and_refuse_missing_nodes() {
        let window = Window {
            north_node: 180_001,
            west_node: 50_000,
            rows: 2,
            columns: 2,
        };
        let node = |metres: f64, impervious: u8, forest: u8| Node {
            height_code: ((metres + 500.0) * 5.0) as u16,
            impervious_percent: impervious,
            forest_percent: forest,
        };
        let nodes = [
            node(100.0, 0, 0),
            node(110.0, 100, 0),
            node(120.0, 0, 80),
            node(130.0, 100, 80),
        ];
        let bytes = encode(window, &nodes);
        let terrain = Terrain::parse(&bytes).unwrap();
        let at = |lat_nodes: f64, lon_nodes: f64| {
            Mercator::from_degrees(lat_nodes / 3600.0, lon_nodes / 3600.0)
        };
        let sample = terrain.sample(at(180_000.5, 50_000.5)).unwrap();
        assert!(
            (sample.height_m - 115.0).abs() < 1e-4 && (sample.ground_factor - 0.5).abs() < 1e-5
        );
        let north_west = terrain.sample(at(180_001.0, 50_000.0)).unwrap();
        assert!((north_west.height_m - 100.0).abs() < 1e-4 && north_west.forest_cover == 0.0);
        assert_eq!(terrain.sample(at(180_000.5, 50_001.5)), None);
        assert_eq!(terrain.sample(at(180_001.5, 50_000.5)), None);
        let mut missing = nodes;
        missing[3].height_code = HEIGHT_MISSING;
        assert_eq!(
            Terrain::parse(&encode(window, &missing))
                .unwrap()
                .sample(at(180_000.5, 50_000.5)),
            None
        );
        assert!(Terrain::parse(&bytes[..bytes.len() - 1]).is_err());
    }
}
