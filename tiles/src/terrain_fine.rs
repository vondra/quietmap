//! `terrain-fine` tiles: a national terrain model's detail over a z12 tile on the 1/6" lattice, six
//! intervals to each of the one-arc-second terrain's (every line of that lattice is one of these):
//! the model's height less its own 1" average, faded to nothing towards its edges and voids. A
//! ray's ground within 100 m of its ends is the `terrain` tile's bilinear height plus this detail,
//! where a berm, a cutting or a road on a slope narrower than the 1" lattice screens or exposes it
//! (the D4's berm at Mníšek 3.5 dB, a road on a valley shelf 15.5 dB; `evidence/2026-10-10/
//! fine-terrain/` in the operations repository); where a tile has no file the detail is nothing
//! and the 1" heights stand as they are. The window is the `terrain` tile's, six times as fine;
//! the detail is in the heights' 0.2 m steps, signed. Stored in blocks of 128 x 128 cells, each
//! block one zstd frame of its planar prediction's residual (as `terrain`), behind a directory, so a
//! click decodes only the blocks its rays' ends touch (12-60 ms of CPU against 117-140 ms for
//! whole tiles, measured on Czechia's clicks).

use crate::FormatError;
use crate::terrain::{Window, predicted};
use std::sync::OnceLock;

/// Fine intervals in one interval of the one-arc-second lattice.
pub const FINE_PER_NODE: i32 = 6;
/// Lattice nodes per degree of the fine lattice.
pub const FINE_NODES_PER_DEGREE: i32 = 3600 * FINE_PER_NODE;
/// Metres of a detail code: the 1" heights' step.
pub const DETAIL_STEP_M: f64 = 0.2;
/// Cells a block spans along each axis; a block also stores the node row and column closing its
/// last cells, so every cell's four corners lie in one block.
pub const BLOCK_CELLS: u32 = 128;
const MAGIC: &[u8; 8] = b"qmtfin2\n";
const HEADER_BYTES: usize = 24;
/// zstd level of a block, as `terrain`'s.
const ZSTD_LEVEL: i32 = 19;

/// The fine window of a `terrain` tile's window: the same corner nodes, six intervals to each.
pub fn fine_window(window: Window) -> Window {
    Window {
        north_node: window.north_node * FINE_PER_NODE,
        west_node: window.west_node * FINE_PER_NODE,
        rows: (window.rows - 1) * FINE_PER_NODE as u32 + 1,
        columns: (window.columns - 1) * FINE_PER_NODE as u32 + 1,
    }
}

/// The detail code of a detail in metres, within the codes' range.
pub fn detail_code(detail_m: f64) -> i16 {
    (detail_m / DETAIL_STEP_M)
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

/// Blocks along an axis of `nodes` nodes.
fn blocks_along(nodes: u32) -> u32 {
    nodes.saturating_sub(1).div_ceil(BLOCK_CELLS).max(1)
}

/// The block of a node index along an axis, and the first and last node it stores.
fn block_span(block: u32, nodes: u32) -> (u32, u32) {
    let first = block * BLOCK_CELLS;
    (first, (first + BLOCK_CELLS).min(nodes - 1))
}

/// Encodes one block's codes, row-major: the planar residual zigzag-coded, low and high bytes.
fn encode_block(codes: &[u16], columns: usize) -> Vec<u8> {
    let n = codes.len();
    let mut planes = vec![0u8; 2 * n];
    for (at, &code) in codes.iter().enumerate() {
        let residual = code.wrapping_sub(predicted(codes, columns, at / columns, at % columns));
        let zigzag = (((residual as i16) << 1) ^ ((residual as i16) >> 15)) as u16;
        planes[at] = zigzag as u8;
        planes[n + at] = (zigzag >> 8) as u8;
    }
    zstd::bulk::compress(&planes, ZSTD_LEVEL).expect("zstd compresses a buffer")
}

/// The bytes of a fine terrain file: the header (magic, window), a directory of each block's
/// frame end (u32 LE, from the first frame's start, blocks row-major), then the frames.
pub fn encode(window: Window, details: &[i16]) -> Vec<u8> {
    assert_eq!(details.len(), window.node_count());
    let (down, across) = (blocks_along(window.rows), blocks_along(window.columns));
    let mut frames = Vec::new();
    let mut ends = Vec::with_capacity((down * across) as usize);
    for block_row in 0..down {
        let (row_first, row_last) = block_span(block_row, window.rows);
        for block_column in 0..across {
            let (column_first, column_last) = block_span(block_column, window.columns);
            let block: Vec<u16> = (row_first..=row_last)
                .flat_map(|row| {
                    let start = row as usize * window.columns as usize;
                    details[start + column_first as usize..=start + column_last as usize]
                        .iter()
                        .map(|&detail| detail as u16)
                })
                .collect();
            frames.extend(encode_block(
                &block,
                (column_last - column_first + 1) as usize,
            ));
            ends.push(frames.len() as u32);
        }
    }
    let mut bytes = Vec::with_capacity(HEADER_BYTES + 4 * ends.len() + frames.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&window.north_node.to_le_bytes());
    bytes.extend_from_slice(&window.west_node.to_le_bytes());
    bytes.extend_from_slice(&window.rows.to_le_bytes());
    bytes.extend_from_slice(&window.columns.to_le_bytes());
    for end in ends {
        bytes.extend_from_slice(&end.to_le_bytes());
    }
    bytes.extend_from_slice(&frames);
    bytes
}

/// A parsed fine terrain file: its blocks decoded when first read.
pub struct FineTerrain {
    window: Window,
    bytes: Vec<u8>,
    /// Each block's frame within `bytes`, row-major.
    frames: Vec<(usize, usize)>,
    blocks_down: u32,
    blocks_across: u32,
    /// Each block's codes once decoded, row-major within the block; `None` where its frame does
    /// not decode.
    decoded: Vec<OnceLock<Option<Vec<i16>>>>,
}

impl FineTerrain {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("terrain-fine: bad magic"));
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let window = Window {
            north_node: word(8) as i32,
            west_node: word(12) as i32,
            rows: word(16),
            columns: word(20),
        };
        if window.rows < 2 || window.columns < 2 {
            return Err(FormatError("terrain-fine: a window without cells"));
        }
        let (down, across) = (blocks_along(window.rows), blocks_along(window.columns));
        let count = (down * across) as usize;
        let first = HEADER_BYTES + 4 * count;
        if bytes.len() < first {
            return Err(FormatError("terrain-fine: the directory is cut"));
        }
        let mut frames = Vec::with_capacity(count);
        let mut start = first;
        for block in 0..count {
            let end = first + word(HEADER_BYTES + 4 * block) as usize;
            if end < start || end > bytes.len() {
                return Err(FormatError("terrain-fine: a frame outside the file"));
            }
            frames.push((start, end));
            start = end;
        }
        Ok(FineTerrain {
            window,
            bytes,
            frames,
            blocks_down: down,
            blocks_across: across,
            decoded: (0..count).map(|_| OnceLock::new()).collect(),
        })
    }

    pub fn window(&self) -> Window {
        self.window
    }

    /// The block holding the cell whose north-west corner is node (row, column), the last block
    /// along an axis for its last node (so it holds that node too), decoded on first use; its
    /// codes, its first row and column, and its columns.
    fn block(&self, row: u32, column: u32) -> Result<(&[i16], u32, u32, u32), FormatError> {
        let block_row = (row / BLOCK_CELLS).min(self.blocks_down - 1);
        let block_column = (column / BLOCK_CELLS).min(self.blocks_across - 1);
        let (row_first, row_last) = block_span(block_row, self.window.rows);
        let (column_first, column_last) = block_span(block_column, self.window.columns);
        let index = (block_row * self.blocks_across + block_column) as usize;
        let columns = column_last - column_first + 1;
        let nodes = (row_last - row_first + 1) as usize * columns as usize;
        let codes = self.decoded[index]
            .get_or_init(|| {
                let (start, end) = self.frames[index];
                let planes = zstd::bulk::decompress(&self.bytes[start..end], 2 * nodes).ok()?;
                if planes.len() != 2 * nodes {
                    return None;
                }
                let mut codes = vec![0u16; nodes];
                for at in 0..nodes {
                    let zigzag = u16::from(planes[at]) | (u16::from(planes[nodes + at]) << 8);
                    let residual = ((zigzag >> 1) as i16) ^ -((zigzag & 1) as i16);
                    codes[at] = (residual as u16).wrapping_add(predicted(
                        &codes,
                        columns as usize,
                        at / columns as usize,
                        at % columns as usize,
                    ));
                }
                Some(codes.into_iter().map(|code| code as i16).collect())
            })
            .as_deref()
            .ok_or(FormatError("terrain-fine: a block does not decode"))?;
        Ok((codes, row_first, column_first, columns))
    }

    /// The detail (m) of node (row, column).
    pub fn node(&self, row: u32, column: u32) -> Result<f64, FormatError> {
        if row >= self.window.rows || column >= self.window.columns {
            return Err(FormatError("terrain-fine: a node outside the window"));
        }
        let (codes, row_first, column_first, columns) = self.block(row, column)?;
        let at = (row - row_first) as usize * columns as usize + (column - column_first) as usize;
        Ok(f64::from(codes[at]) * DETAIL_STEP_M)
    }

    /// The detail (m) a `fraction` of the way from node (row, column) to the next along its row
    /// (`down` false) or its column (`down` true), from one block; a node of no weight not read.
    pub fn on_line(
        &self,
        row: u32,
        column: u32,
        down: bool,
        fraction: f64,
    ) -> Result<f64, FormatError> {
        let (next_row, next_column) = if down {
            (row + 1, column)
        } else {
            (row, column + 1)
        };
        if fraction <= 0.0 {
            return self.node(row, column);
        }
        if fraction >= 1.0 {
            return self.node(next_row, next_column);
        }
        if next_row >= self.window.rows || next_column >= self.window.columns {
            return Err(FormatError("terrain-fine: a node outside the window"));
        }
        let (codes, row_first, column_first, columns) = self.block(row, column)?;
        let at = (row - row_first) as usize * columns as usize + (column - column_first) as usize;
        let next = at + if down { columns as usize } else { 1 };
        let (first, second) = (f64::from(codes[at]), f64::from(codes[next]));
        Ok((first + fraction * (second - first)) * DETAIL_STEP_M)
    }

    /// The bilinear detail (m) at fractional node coordinates; an error where the point is outside
    /// the window or a block does not decode.
    pub fn at(&self, row: f64, column: f64) -> Result<f64, FormatError> {
        let last = |nodes: u32| f64::from(nodes - 1);
        if !(0.0..=last(self.window.rows)).contains(&row)
            || !(0.0..=last(self.window.columns)).contains(&column)
        {
            return Err(FormatError("terrain-fine: a point outside the window"));
        }
        let (r, c) = (
            (row.floor() as u32).min(self.window.rows - 2),
            (column.floor() as u32).min(self.window.columns - 2),
        );
        let (fr, fc) = (row - f64::from(r), column - f64::from(c));
        let (codes, row_first, column_first, columns) = self.block(r, c)?;
        let (at, columns) = (
            (r - row_first) as usize * columns as usize + (c - column_first) as usize,
            columns as usize,
        );
        let code = |k: usize| f64::from(codes[k]);
        let detail = (1.0 - fr) * ((1.0 - fc) * code(at) + fc * code(at + 1))
            + fr * ((1.0 - fc) * code(at + columns) + fc * code(at + columns + 1));
        Ok(detail * DETAIL_STEP_M)
    }

    /// Every block decoded, row-major over the window: what the GPU painter uploads.
    pub fn codes(&self) -> Result<Vec<i16>, FormatError> {
        let columns = self.window.columns as usize;
        let mut codes = vec![0i16; self.window.node_count()];
        for block_row in 0..self.blocks_down {
            for block_column in 0..self.blocks_across {
                let (block, row_first, column_first, block_columns) =
                    self.block(block_row * BLOCK_CELLS, block_column * BLOCK_CELLS)?;
                for (k, line) in block.chunks_exact(block_columns as usize).enumerate() {
                    let start = (row_first as usize + k) * columns + column_first as usize;
                    codes[start..start + line.len()].copy_from_slice(line);
                }
            }
        }
        Ok(codes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::TileId;

    /// Every node comes back as written through blocks of every shape (a window of 2.3 blocks by
    /// 1.5), negative and positive alike; a cut file and a bad frame are errors.
    #[test]
    fn a_fine_tile_decodes_to_the_details_it_was_encoded_from() {
        let window = Window {
            north_node: 1_076_760,
            west_node: 307_800,
            rows: 2 * BLOCK_CELLS + 40,
            columns: BLOCK_CELLS + 65,
        };
        let details: Vec<i16> = (0..window.node_count())
            .map(|at| match at % 97 {
                0 => i16::MIN,
                1 => i16::MAX,
                k => (k as i16 - 48) * 3 + (at / 300) as i16 % 7,
            })
            .collect();
        let bytes = encode(window, &details);
        let fine = FineTerrain::parse(bytes.clone()).unwrap();
        assert_eq!(fine.window(), window);
        assert_eq!(fine.codes().unwrap(), details);
        let metres = |row: u32, column: u32| {
            f64::from(details[(row * window.columns + column) as usize]) * DETAIL_STEP_M
        };
        for (row, column) in [
            (0, 0),
            (127, 127),
            (128, 128),
            (255, 191),
            (294, 0),
            (295, 64),
        ] {
            assert_eq!(fine.node(row, column).unwrap(), metres(row, column));
            let east = fine.on_line(row, column, false, 0.5).unwrap();
            assert!((east - (metres(row, column) + metres(row, column + 1)) / 2.0).abs() < 1e-9);
            if row + 1 < window.rows {
                let south = fine.on_line(row, column, true, 0.25).unwrap();
                let expected = 0.75 * metres(row, column) + 0.25 * metres(row + 1, column);
                assert!((south - expected).abs() < 1e-9);
            }
        }
        assert!(FineTerrain::parse(bytes[..HEADER_BYTES + 8].to_vec()).is_err());
        let mut broken = bytes;
        let last = broken.len() - 1;
        broken[last] ^= 0xff;
        assert!(FineTerrain::parse(broken).unwrap().codes().is_err());
    }

    /// A tile's fine window has the one-arc-second window's corner nodes, six intervals to each.
    #[test]
    fn the_fine_window_nests_in_the_tile_window() {
        let coarse = Window::of_tile(TileId { x: 2210, y: 1391 });
        let fine = fine_window(coarse);
        assert_eq!(fine.north_node, 6 * coarse.north_node);
        assert_eq!(
            fine.west_node + fine.columns as i32 - 1,
            6 * (coarse.west_node + coarse.columns as i32 - 1)
        );
        assert_eq!(
            fine.north_node - fine.rows as i32 + 1,
            6 * (coarse.north_node - coarse.rows as i32 + 1)
        );
    }

    #[test]
    fn details_interpolate_along_lines_and_over_cells() {
        let window = Window {
            north_node: 6,
            west_node: 0,
            rows: 3,
            columns: 3,
        };
        let details: Vec<i16> = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, -8.0]
            .iter()
            .map(|&m| detail_code(m))
            .collect();
        let fine = FineTerrain::parse(encode(window, &details)).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(fine.at(0.5, 0.5).unwrap(), 2.0));
        assert!(close(fine.at(2.0, 2.0).unwrap(), -8.0));
        assert!(close(fine.on_line(1, 0, false, 0.25).unwrap(), 3.25));
        assert!(close(fine.on_line(2, 2, false, 0.0).unwrap(), -8.0));
        assert!(close(fine.on_line(1, 2, true, 1.0).unwrap(), -8.0));
        assert!(fine.at(2.5, 0.0).is_err());
    }
}
