//! `terrain-fine` tiles: the heights of a z12 tile on the 1/6" lattice, six intervals to each of the
//! one-arc-second terrain's, so every line of that lattice is one of these, where a national model
//! of 5 m or finer exists. A ray reads them within 100 m of its ends, where a berm, a cutting or a
//! road on a slope narrower than the 1" lattice screens or exposes it (the D4's berm at Mníšek 3.5
//! dB, a road on a valley shelf 15.5 dB; `evidence/2026-10-10/fine-terrain/` in the operations
//! repository). The window is the `terrain` tile's, six times as fine; the heights are its 0.2 m
//! codes. Stored in blocks of 128 x 128 cells, each block one zstd frame of its planar prediction's
//! residual (as `terrain`), behind a directory, so a click decodes only the blocks its rays' ends
//! touch (12-60 ms of CPU against 117-140 ms for whole tiles, measured on Czechia's clicks).
//! Ground factor and trees stay the one-arc-second tile's.

use crate::FormatError;
use crate::terrain::{HEIGHT_MISSING, Window, height_m_of_code, predicted};
use std::sync::OnceLock;

/// Fine intervals in one interval of the one-arc-second lattice.
pub const FINE_PER_NODE: i32 = 6;
/// Lattice nodes per degree of the fine lattice.
pub const FINE_NODES_PER_DEGREE: i32 = 3600 * FINE_PER_NODE;
/// Cells a block spans along each axis; a block also stores the node row and column closing its
/// last cells, so every cell's four corners lie in one block.
pub const BLOCK_CELLS: u32 = 128;
const MAGIC: &[u8; 8] = b"qmtfin1\n";
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

/// Blocks along an axis of `nodes` nodes.
fn blocks_along(nodes: u32) -> u32 {
    nodes.saturating_sub(1).div_ceil(BLOCK_CELLS).max(1)
}

/// The block of a node index along an axis, and the first and last node it stores.
fn block_span(block: u32, nodes: u32) -> (u32, u32) {
    let first = block * BLOCK_CELLS;
    (first, (first + BLOCK_CELLS).min(nodes - 1))
}

/// Encodes one block's heights, row-major: the planar residual zigzag-coded, low and high bytes.
fn encode_block(heights: &[u16], columns: usize) -> Vec<u8> {
    let n = heights.len();
    let mut planes = vec![0u8; 2 * n];
    for (at, &height) in heights.iter().enumerate() {
        let residual = height.wrapping_sub(predicted(heights, columns, at / columns, at % columns));
        let zigzag = (((residual as i16) << 1) ^ ((residual as i16) >> 15)) as u16;
        planes[at] = zigzag as u8;
        planes[n + at] = (zigzag >> 8) as u8;
    }
    zstd::bulk::compress(&planes, ZSTD_LEVEL).expect("zstd compresses a buffer")
}

/// The bytes of a fine terrain file: the header (magic, window), a directory of each block's
/// frame end (u32 LE, from the first frame's start, blocks row-major), then the frames.
pub fn encode(window: Window, heights: &[u16]) -> Vec<u8> {
    assert_eq!(heights.len(), window.node_count());
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
                    heights[start + column_first as usize..=start + column_last as usize]
                        .iter()
                        .copied()
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
    /// Each block's heights once decoded, row-major within the block; `None` where its frame does
    /// not decode.
    decoded: Vec<OnceLock<Option<Vec<u16>>>>,
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
    /// along an axis for its last node, decoded on first use; its heights, its first row and
    /// column, and its columns.
    fn block(&self, row: u32, column: u32) -> Result<(&[u16], u32, u32, u32), FormatError> {
        let block_row = (row / BLOCK_CELLS).min(self.blocks_down - 1);
        let block_column = (column / BLOCK_CELLS).min(self.blocks_across - 1);
        let (row_first, row_last) = block_span(block_row, self.window.rows);
        let (column_first, column_last) = block_span(block_column, self.window.columns);
        let index = (block_row * self.blocks_across + block_column) as usize;
        let columns = column_last - column_first + 1;
        let nodes = (row_last - row_first + 1) as usize * columns as usize;
        let heights = self.decoded[index]
            .get_or_init(|| {
                let (start, end) = self.frames[index];
                let planes = zstd::bulk::decompress(&self.bytes[start..end], 2 * nodes).ok()?;
                if planes.len() != 2 * nodes {
                    return None;
                }
                let mut heights = vec![0u16; nodes];
                for at in 0..nodes {
                    let zigzag = u16::from(planes[at]) | (u16::from(planes[nodes + at]) << 8);
                    let residual = ((zigzag >> 1) as i16) ^ -((zigzag & 1) as i16);
                    heights[at] = (residual as u16).wrapping_add(predicted(
                        &heights,
                        columns as usize,
                        at / columns as usize,
                        at % columns as usize,
                    ));
                }
                Some(heights)
            })
            .as_deref()
            .ok_or(FormatError("terrain-fine: a block does not decode"))?;
        Ok((heights, row_first, column_first, columns))
    }

    /// The height code of node (row, column); `HEIGHT_MISSING` passes through.
    pub fn node(&self, row: u32, column: u32) -> Result<u16, FormatError> {
        if row >= self.window.rows || column >= self.window.columns {
            return Err(FormatError("terrain-fine: a node outside the window"));
        }
        let (heights, row_first, column_first, columns) = self.block(row, column)?;
        Ok(heights
            [(row - row_first) as usize * columns as usize + (column - column_first) as usize])
    }

    /// The height codes of node (row, column) and the next along its row (`down` false) or its
    /// column (`down` true), from one block; `HEIGHT_MISSING` passes through.
    pub fn pair(&self, row: u32, column: u32, down: bool) -> Result<[u16; 2], FormatError> {
        let (next_row, next_column) = if down {
            (row + 1, column)
        } else {
            (row, column + 1)
        };
        if next_row >= self.window.rows || next_column >= self.window.columns {
            return Err(FormatError("terrain-fine: a node outside the window"));
        }
        let (heights, row_first, column_first, columns) = self.block(row, column)?;
        let at = (row - row_first) as usize * columns as usize + (column - column_first) as usize;
        let step = if down { columns as usize } else { 1 };
        Ok([heights[at], heights[at + step]])
    }

    /// The bilinear height (m) at fractional node coordinates, `None` where a corner it weighs has
    /// no data; an error where the point is outside the window or a block does not decode.
    pub fn height_at(&self, row: f64, column: f64) -> Result<Option<f64>, FormatError> {
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
        let (heights, row_first, column_first, columns) = self.block(r, c)?;
        let at = (r - row_first) as usize * columns as usize + (c - column_first) as usize;
        let corners = [
            (heights[at], (1.0 - fr) * (1.0 - fc)),
            (heights[at + 1], (1.0 - fr) * fc),
            (heights[at + columns as usize], fr * (1.0 - fc)),
            (heights[at + columns as usize + 1], fr * fc),
        ];
        let mut height = 0.0;
        for (code, weight) in corners {
            if weight > 0.0 {
                if code == HEIGHT_MISSING {
                    return Ok(None);
                }
                height += weight * height_m_of_code(code);
            }
        }
        Ok(Some(height))
    }

    /// Every block decoded, row-major over the window: what the GPU painter uploads.
    pub fn heights(&self) -> Result<Vec<u16>, FormatError> {
        let columns = self.window.columns as usize;
        let mut heights = vec![0u16; self.window.node_count()];
        for block_row in 0..self.blocks_down {
            for block_column in 0..self.blocks_across {
                let (block, row_first, column_first, block_columns) =
                    self.block(block_row * BLOCK_CELLS, block_column * BLOCK_CELLS)?;
                for (k, line) in block.chunks_exact(block_columns as usize).enumerate() {
                    let start = (row_first as usize + k) * columns + column_first as usize;
                    heights[start..start + line.len()].copy_from_slice(line);
                }
            }
        }
        Ok(heights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::TileId;

    /// Every node comes back as written through blocks of every shape (a window of 2.3 blocks by
    /// 1.5), the missing code included; a cut file and a bad frame are errors.
    #[test]
    fn a_fine_tile_decodes_to_the_heights_it_was_encoded_from() {
        let window = Window {
            north_node: 1_076_760,
            west_node: 307_800,
            rows: 2 * BLOCK_CELLS + 40,
            columns: BLOCK_CELLS + 65,
        };
        let heights: Vec<u16> = (0..window.node_count())
            .map(|at| match at % 97 {
                0 => HEIGHT_MISSING,
                k => 4_600 + (at / 300) as u16 + k as u16 * 3,
            })
            .collect();
        let bytes = encode(window, &heights);
        let fine = FineTerrain::parse(bytes.clone()).unwrap();
        assert_eq!(fine.window(), window);
        assert_eq!(fine.heights().unwrap(), heights);
        let node = |row: u32, column: u32| heights[(row * window.columns + column) as usize];
        for (row, column) in [
            (0, 0),
            (127, 127),
            (128, 128),
            (255, 191),
            (294, 0),
            (295, 64),
        ] {
            let east = fine.pair(row, column, false).unwrap();
            assert_eq!(east, [node(row, column), node(row, column + 1)]);
            if row + 1 < window.rows {
                let south = fine.pair(row, column, true).unwrap();
                assert_eq!(south, [node(row, column), node(row + 1, column)]);
            }
        }
        assert!(FineTerrain::parse(bytes[..HEADER_BYTES + 8].to_vec()).is_err());
        let mut broken = bytes;
        let last = broken.len() - 1;
        broken[last] ^= 0xff;
        let broken = FineTerrain::parse(broken).unwrap();
        assert!(broken.heights().is_err());
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
    fn heights_interpolate_and_refuse_missing_corners() {
        let window = Window {
            north_node: 6,
            west_node: 0,
            rows: 3,
            columns: 3,
        };
        let code = |m: f64| crate::terrain::code_of_height_m(m);
        let mut heights = vec![
            code(100.0),
            code(110.0),
            code(120.0),
            code(130.0),
            code(140.0),
            code(150.0),
            code(160.0),
            code(170.0),
            code(180.0),
        ];
        let fine = FineTerrain::parse(encode(window, &heights)).unwrap();
        let at = |row: f64, column: f64| fine.height_at(row, column).unwrap().unwrap();
        assert!((at(0.5, 0.5) - 120.0).abs() < 1e-9);
        assert!((at(2.0, 2.0) - 180.0).abs() < 1e-9);
        assert!((at(1.0, 0.25) - 132.5).abs() < 1e-9);
        assert!(fine.height_at(2.5, 0.0).is_err());
        heights[4] = HEIGHT_MISSING;
        let fine = FineTerrain::parse(encode(window, &heights)).unwrap();
        assert_eq!(fine.height_at(0.5, 0.5).unwrap(), None);
        assert!(fine.height_at(0.0, 0.0).unwrap().is_some());
    }
}
