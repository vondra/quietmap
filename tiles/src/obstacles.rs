//! `obstacles` tiles: every building ring and wall crossing one of the tile's cells, stored whole
//! in the tile's int16 frame, and per cell the runs of consecutive edges passing through it.
//!
//! A cell is a z19 tile: 256 steps, 128 x 128 per tile (49 m in Prague, 76 m at the equator).
//! Tile-local coordinates are exact shifts of the global step lattice, so every tile storing an
//! outline stores bit-identical global vertices under the same global footprint id.
//!
//! Most cells hold nothing (96 % of the world's): a bitmap marks the occupied ones and counts them
//! before every 64, so a cell finds its runs in place. A footprint id is its z9 square (the high 32
//! bits) and its ordinal there: an outline keeps the ordinal and the row of its square in the
//! tile's table of them, and its vertices run to where the next outline's begin. Every section
//! starts 4-aligned, as the GPU reads it.
//!
//! ```text
//! magic "qmobs2\n\0", u32 outlines, vertices, runs, occupied cells, footprint squares, 0
//! 256 x u64: bit c % 64 of word c / 64 set for an occupied cell c (row * 128 + column)
//! 256 x u16: the occupied cells before each word
//! per occupied cell and one more, u32: its first run (the last: the runs)
//! per footprint square, u32: the high half of its footprint ids, ascending
//! per run, 8 B: outline u32, first edge u16, edge count u16
//! per outline, sorted by footprint id, 12 B: first vertex u32, ordinal u32, height u16 (dm),
//!   square row u8, kind | envelope << 2 u8
//! per vertex, 4 B: 2 x i16
//! ```

mod cells;
mod parse;

pub use cells::{for_each_edge_cell, tiles_crossed};
pub use parse::Obstacles;

const MAGIC: &[u8; 8] = b"qmobs2\n\0";
const HEADER_BYTES: usize = 32;
/// The occupied-cell bitmap's words, and the bytes of it and of the counts before each word.
const CELL_WORDS: usize = CELLS / 64;
const CELL_INDEX_BYTES: usize = CELL_WORDS * (8 + 2);
const RUN_BYTES: usize = 8;
const OUTLINE_BYTES: usize = 12;
const VERTEX_BYTES: usize = 4;
/// The most footprint squares a tile names: an outline keeps its square's row in a byte.
const MAXIMUM_SQUARES: usize = 256;
/// An outline's kind is its last byte's low two bits, its envelope the rest.
const ENVELOPE_SHIFT: u8 = 2;
/// Steps per cell side.
pub const CELL_STEPS: i64 = 256;
/// Cells per tile side; a tile's cells are row-major, row 0 the northernmost (y grows south).
pub const CELLS_PER_SIDE: i64 = 128;
/// Cells per tile.
pub const CELLS: usize = (CELLS_PER_SIDE * CELLS_PER_SIDE) as usize;
/// The tallest outline a tile holds: heights are u16 decimetres.
pub const MAXIMUM_HEIGHT_M: f64 = u16::MAX as f64 / 10.0;
/// The most vertices one outline holds (u16 counts; outlines are never split).
pub const MAXIMUM_VERTICES: usize = u16::MAX as usize;
/// Cells between a tile's centre (local 0) and its west or north edge.
const HALF_TILE_CELLS: i64 = CELLS_PER_SIDE / 2;

/// A building's envelope (dev4 `EnvelopeClass`): only `Outdoor` (carports, roof structures) is
/// open; a point under any other class is inside an enclosed building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnvelopeClass {
    Outdoor = 0,
    Residential = 1,
    Commercial = 2,
    Industrial = 3,
    Historic = 4,
    Default = 5,
}

impl EnvelopeClass {
    const ALL: [EnvelopeClass; 6] = [
        EnvelopeClass::Outdoor,
        EnvelopeClass::Residential,
        EnvelopeClass::Commercial,
        EnvelopeClass::Industrial,
        EnvelopeClass::Historic,
        EnvelopeClass::Default,
    ];

    /// The class of a stored code, `None` for an unknown one.
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    pub fn is_enclosed(self) -> bool {
        self != EnvelopeClass::Outdoor
    }
}

/// A building's exterior ring or hole (closed rings sharing the footprint id), or a wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    Exterior = 0,
    Hole = 1,
    Wall = 2,
}

impl OutlineKind {
    const ALL: [OutlineKind; 3] = [OutlineKind::Exterior, OutlineKind::Hole, OutlineKind::Wall];

    pub fn is_building(self) -> bool {
        self != OutlineKind::Wall
    }
}

/// One outline as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// Global and stable across tiles.
    pub footprint_id: u64,
    pub kind: OutlineKind,
    /// `Outdoor` for walls.
    pub envelope: EnvelopeClass,
    /// Above the local terrain, metres (0.1 m steps).
    pub height_m: f64,
    /// Tile-local vertices. A building ring is closed (its first vertex repeated last), so edge
    /// `i` runs from vertex `i` to vertex `i + 1` for rings and walls alike.
    pub vertices: Vec<[i16; 2]>,
}

/// Edges `first_edge .. first_edge + edge_count` of one outline, all passing through one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub outline: u32,
    pub first_edge: u16,
    pub edge_count: u16,
}

/// One stored outline's fields; its vertices are `first_vertex .. first_vertex + vertex_count`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutlineRecord {
    pub footprint_id: u64,
    pub kind: OutlineKind,
    pub envelope: EnvelopeClass,
    pub height_m: f64,
    pub first_vertex: usize,
    pub vertex_count: usize,
}

fn height_code(height_m: f64) -> u16 {
    let code = (height_m * 10.0).round();
    assert!(
        (1.0..=f64::from(u16::MAX)).contains(&code),
        "outline height {height_m} m outside the u16 decimetre range"
    );
    code as u16
}

/// The bytes of an obstacles tile: header, the occupied cells' index, the footprint squares, runs,
/// outline records, vertices. `outlines` are stored in the given order, which must be sorted by
/// footprint id so the rings of one footprint are consecutive; the cell index follows from their
/// edges.
pub fn encode(outlines: &[Outline]) -> Vec<u8> {
    let mut cells: Vec<Vec<Run>> = vec![Vec::new(); CELLS];
    for (index, outline) in outlines.iter().enumerate() {
        let count = outline.vertices.len();
        let closed = outline.vertices.first() == outline.vertices.last();
        match outline.kind {
            OutlineKind::Wall => assert!(count >= 2, "a wall needs two vertices"),
            _ => assert!(
                count >= 4 && closed,
                "a ring is closed with three vertices or more"
            ),
        }
        assert!(count <= MAXIMUM_VERTICES, "outline of {count} vertices");
        assert!(
            index == 0 || outlines[index - 1].footprint_id <= outline.footprint_id,
            "outlines sorted by footprint id"
        );
        let outline_index = u32::try_from(index).expect("tile too large");
        for (edge, pair) in outline.vertices.windows(2).enumerate() {
            let [start, end] = [pair[0], pair[1]].map(|v| [i64::from(v[0]), i64::from(v[1])]);
            for_each_edge_cell(start, end, |[column, row]| {
                let (column, row) = (column + HALF_TILE_CELLS, row + HALF_TILE_CELLS);
                if !(0..CELLS_PER_SIDE).contains(&column) || !(0..CELLS_PER_SIDE).contains(&row) {
                    return;
                }
                let runs = &mut cells[(row * CELLS_PER_SIDE + column) as usize];
                match runs.last_mut() {
                    Some(run)
                        if run.outline == outline_index
                            && usize::from(run.first_edge) + usize::from(run.edge_count)
                                == edge =>
                    {
                        run.edge_count += 1
                    }
                    _ => runs.push(Run {
                        outline: outline_index,
                        first_edge: edge as u16,
                        edge_count: 1,
                    }),
                }
            });
        }
    }
    let vertex_count: usize = outlines.iter().map(|outline| outline.vertices.len()).sum();
    let run_count: usize = cells.iter().map(Vec::len).sum();
    let occupied = cells.iter().filter(|runs| !runs.is_empty()).count();
    let mut squares: Vec<u32> = outlines
        .iter()
        .map(|outline| (outline.footprint_id >> 32) as u32)
        .collect();
    squares.dedup();
    assert!(
        squares.len() <= MAXIMUM_SQUARES,
        "{} footprint squares in a tile",
        squares.len()
    );
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + CELL_INDEX_BYTES
            + 4 * (occupied + 1)
            + 4 * squares.len()
            + RUN_BYTES * run_count
            + OUTLINE_BYTES * outlines.len()
            + VERTEX_BYTES * vertex_count,
    );
    bytes.extend_from_slice(MAGIC);
    for count in [
        outlines.len(),
        vertex_count,
        run_count,
        occupied,
        squares.len(),
        0,
    ] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    let words: Vec<u64> = cells
        .chunks_exact(64)
        .map(|word| {
            word.iter()
                .enumerate()
                .filter(|(_, runs)| !runs.is_empty())
                .fold(0, |bits, (bit, _)| bits | 1 << bit)
        })
        .collect();
    for word in &words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    let mut before = 0u16;
    for word in &words {
        bytes.extend_from_slice(&before.to_le_bytes());
        before += word.count_ones() as u16;
    }
    let mut first_run = 0u32;
    for runs in cells.iter().filter(|runs| !runs.is_empty()) {
        bytes.extend_from_slice(&first_run.to_le_bytes());
        first_run += runs.len() as u32;
    }
    bytes.extend_from_slice(&first_run.to_le_bytes());
    for square in &squares {
        bytes.extend_from_slice(&square.to_le_bytes());
    }
    for run in cells.iter().flatten() {
        bytes.extend_from_slice(&run.outline.to_le_bytes());
        bytes.extend_from_slice(&run.first_edge.to_le_bytes());
        bytes.extend_from_slice(&run.edge_count.to_le_bytes());
    }
    let mut first_vertex = 0u32;
    for outline in outlines {
        let square = squares
            .binary_search(&((outline.footprint_id >> 32) as u32))
            .expect("its square");
        bytes.extend_from_slice(&first_vertex.to_le_bytes());
        bytes.extend_from_slice(&(outline.footprint_id as u32).to_le_bytes());
        bytes.extend_from_slice(&height_code(outline.height_m).to_le_bytes());
        bytes.push(square as u8);
        bytes.push(outline.kind as u8 | (outline.envelope as u8) << ENVELOPE_SHIFT);
        first_vertex += outline.vertices.len() as u32;
    }
    for coordinate in outlines
        .iter()
        .flat_map(|outline| outline.vertices.iter().flatten())
    {
        bytes.extend_from_slice(&coordinate.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests;
