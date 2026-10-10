//! `terrain` tiles: the one-arc-second lattice window of a z12 tile with height, imperviousness,
//! forest cover and canopy height per node. The window brackets the tile with floor/ceil node
//! edges, so every point of the tile interpolates from its own file and neighbours share their
//! seam nodes. Stored as planes in one zstd frame, the heights as the residual of a planar
//! prediction from the west, north and north-west nodes: 0.64 B a node of information in 4 stored
//! raw (`evidence/2026-10-10/storage-design/terrain.md`: the world 1,108.6 -> 177.1 GB, lossless).
//! A parsed tile holds its nodes decoded, 4 bytes each, as the samplers and the GPU index them.

use crate::FormatError;
use crate::geo::{Mercator, TILES_PER_AXIS, TileId};

/// Lattice nodes per degree of latitude and of longitude.
pub const NODES_PER_DEGREE: i32 = 3600;
/// Height code of a node without data; a sample touching one is refused, never guessed.
pub const HEIGHT_MISSING: u16 = u16::MAX;
/// Imperviousness and forest cover above this percentage mark a node without data.
pub const PERCENT_MAX: u8 = 100;
const MAGIC: &[u8; 8] = b"qmterr2\n";
const HEADER_BYTES: usize = 24;
/// A decoded node: height code u16 LE, imperviousness, forest cover.
pub const NODE_BYTES: usize = 4;
/// The frame's planes, n bytes each: the height residual's low and high bytes, imperviousness,
/// forest cover, canopy height.
const PLANES: usize = 5;
/// zstd level of a tile: 19 (zstd-9 is 4.6 % larger and builds 13x faster; terrain is rebuilt
/// rarely).
const ZSTD_LEVEL: i32 = 19;

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
    /// Height of the tree canopy above the ground, metres (GLAD 2020 through dev4's rasters).
    pub canopy_m: u8,
}

/// Ground under one point: bilinear height and ground factor, nearest forest cover and canopy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    pub height_m: f64,
    /// CNOSSOS ground factor G in [0, 1]: 0 hard, 1 soft.
    pub ground_factor: f64,
    /// Canopy cover fraction in [0, 1].
    pub forest_cover: f64,
    /// Canopy height above the ground (m).
    pub canopy_m: f64,
}

/// An absent terrain file in a complete release: sea level, hard water, no forest.
pub const OCEAN: GroundSample = GroundSample {
    height_m: 0.0,
    ground_factor: 0.0,
    forest_cover: 0.0,
    canopy_m: 0.0,
};

/// The planar prediction of node (r, c) from its west, north and north-west neighbours (0 off the
/// window), in the u16 ring: encoding stores the height minus it, decoding adds it back.
fn predicted(heights: &[u16], columns: usize, r: usize, c: usize) -> u16 {
    let at = |r: usize, c: usize| heights[r * columns + c];
    let west = if c > 0 { at(r, c - 1) } else { 0 };
    let north = if r > 0 { at(r - 1, c) } else { 0 };
    let north_west = if r > 0 && c > 0 { at(r - 1, c - 1) } else { 0 };
    west.wrapping_add(north).wrapping_sub(north_west)
}

/// The bytes of a terrain file: the header, then one zstd frame of the five planes, row-major,
/// the height residual zigzag-coded (small of either sign: small codes).
pub fn encode(window: Window, nodes: &[Node]) -> Vec<u8> {
    assert_eq!(nodes.len(), window.node_count());
    let (n, columns) = (nodes.len(), window.columns as usize);
    let heights: Vec<u16> = nodes.iter().map(|node| node.height_code).collect();
    let mut planes = vec![0u8; PLANES * n];
    for (at, node) in nodes.iter().enumerate() {
        let residual =
            node.height_code
                .wrapping_sub(predicted(&heights, columns, at / columns, at % columns))
                as i16;
        let zigzag = ((residual << 1) ^ (residual >> 15)) as u16;
        planes[at] = zigzag as u8;
        planes[n + at] = (zigzag >> 8) as u8;
        planes[2 * n + at] = node.impervious_percent;
        planes[3 * n + at] = node.forest_percent;
        planes[4 * n + at] = node.canopy_m;
    }
    let frame = zstd::bulk::compress(&planes, ZSTD_LEVEL).expect("zstd compresses a buffer");
    let mut bytes = Vec::with_capacity(HEADER_BYTES + frame.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&window.north_node.to_le_bytes());
    bytes.extend_from_slice(&window.west_node.to_le_bytes());
    bytes.extend_from_slice(&window.rows.to_le_bytes());
    bytes.extend_from_slice(&window.columns.to_le_bytes());
    bytes.extend_from_slice(&frame);
    bytes
}

/// A parsed terrain file: its nodes decoded.
pub struct Terrain {
    window: Window,
    /// `NODE_BYTES` a node, row-major.
    nodes: Vec<u8>,
    /// Canopy height (m) a node, row-major.
    canopy_m: Vec<u8>,
    /// Web Mercator y (z12 tile units) of every node row: a sample finds its rows without a
    /// transcendental function. Between two rows 31 m apart, latitude is linear in y to within
    /// 1e-6 of the row spacing (0.03 mm).
    row_mercator_y: Vec<f64>,
}

impl Terrain {
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
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
        let (n, columns) = (window.node_count(), window.columns as usize);
        let planes = zstd::bulk::decompress(&bytes[HEADER_BYTES..], PLANES * n)
            .map_err(|_| FormatError("terrain: the frame does not decode"))?;
        if planes.len() != PLANES * n {
            return Err(FormatError("terrain: the frame does not match the window"));
        }
        let mut heights = vec![0u16; n];
        let mut nodes = vec![0u8; NODE_BYTES * n];
        for at in 0..n {
            let zigzag = u16::from(planes[at]) | (u16::from(planes[n + at]) << 8);
            let residual = ((zigzag >> 1) as i16) ^ -((zigzag & 1) as i16);
            let height = (residual as u16).wrapping_add(predicted(
                &heights,
                columns,
                at / columns,
                at % columns,
            ));
            heights[at] = height;
            nodes[NODE_BYTES * at..NODE_BYTES * at + 2].copy_from_slice(&height.to_le_bytes());
            nodes[NODE_BYTES * at + 2] = planes[2 * n + at];
            nodes[NODE_BYTES * at + 3] = planes[3 * n + at];
        }
        let row_mercator_y = (0..window.rows)
            .map(|row| {
                let lat = f64::from(window.north_node - row as i32) / f64::from(NODES_PER_DEGREE);
                Mercator::from_degrees(lat, 0.0).y
            })
            .collect();
        Ok(Terrain {
            window,
            nodes,
            canopy_m: planes[4 * n..].to_vec(),
            row_mercator_y,
        })
    }

    /// The decoded nodes, `NODE_BYTES` each, row-major: what the GPU painter uploads.
    pub fn node_bytes(&self) -> &[u8] {
        &self.nodes
    }

    /// Web Mercator y of every node row (the GPU painter samples with these very values).
    pub fn row_mercator_y(&self) -> &[f64] {
        &self.row_mercator_y
    }

    pub fn window(&self) -> Window {
        self.window
    }

    pub fn node(&self, row: u32, column: u32) -> Node {
        let index = row as usize * self.window.columns as usize + column as usize;
        let record = &self.nodes[NODE_BYTES * index..NODE_BYTES * (index + 1)];
        Node {
            height_code: u16::from_le_bytes([record[0], record[1]]),
            impervious_percent: record[2],
            forest_percent: record[3],
            canopy_m: self.canopy_m[index],
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
            canopy_m: f64::from(nearest.canopy_m),
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

    /// Every node comes back as written: random heights (steep jumps, the no-data code and the
    /// u16 ends wrap the prediction), percents and canopies on an odd window.
    #[test]
    fn a_tile_decodes_to_the_nodes_it_was_encoded_from() {
        let window = Window {
            north_node: 181_042,
            west_node: 50_625,
            rows: 37,
            columns: 53,
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let nodes: Vec<Node> = (0..window.node_count())
            .map(|at| Node {
                height_code: match next() % 7 {
                    0 => HEIGHT_MISSING,
                    1 => 0,
                    2 => (next() % 65_536) as u16,
                    _ => 2_500 + (at as u16 % 53) * 3 + (next() % 5) as u16,
                },
                impervious_percent: (next() % 101) as u8,
                forest_percent: (next() % 102) as u8,
                canopy_m: (next() % 61) as u8,
            })
            .collect();
        let terrain = Terrain::parse(&encode(window, &nodes)).unwrap();
        assert_eq!(terrain.window(), window);
        for (at, node) in nodes.iter().enumerate() {
            let (row, column) = ((at as u32) / window.columns, (at as u32) % window.columns);
            assert_eq!(terrain.node(row, column), *node, "node {at}");
        }
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
            canopy_m: forest / 4,
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
