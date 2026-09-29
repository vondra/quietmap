//! `obstacles` tiles: every building ring and wall crossing one of the tile's cells, stored whole
//! in the tile's int16 frame, and per cell the runs of consecutive edges passing through it.
//!
//! A cell is a z19 tile: 256 steps, 128 x 128 per tile (49 m in Prague, 76 m at the equator).
//! Tile-local coordinates are exact shifts of the global step lattice, so every tile storing an
//! outline stores bit-identical global vertices under the same global footprint id.

mod cells;
mod parse;

pub use cells::{for_each_edge_cell, tiles_crossed};
pub use parse::Obstacles;

const MAGIC: &[u8; 8] = b"qmobs1\n\0";
const HEADER_BYTES: usize = 32;
const RUN_BYTES: usize = 8;
const OUTLINE_BYTES: usize = 20;
const VERTEX_BYTES: usize = 4;
/// Steps per cell side.
pub const CELL_STEPS: i64 = 256;
/// Cells per tile side; a tile's cells are row-major, row 0 the northernmost (y grows south).
pub const CELLS_PER_SIDE: i64 = 128;
/// Cells per tile.
pub const CELLS: usize = (CELLS_PER_SIDE * CELLS_PER_SIDE) as usize;
/// The tallest outline a file holds: heights are u16 decimetres.
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

/// The bytes of an obstacles file: header, cell offsets, runs, outline records, vertices.
/// `outlines` are stored in the given order, which must be sorted by footprint id so the rings of
/// one footprint are consecutive; the cell table follows from their edges.
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
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + 4 * (CELLS + 1)
            + RUN_BYTES * run_count
            + OUTLINE_BYTES * outlines.len()
            + VERTEX_BYTES * vertex_count,
    );
    bytes.extend_from_slice(MAGIC);
    for count in [outlines.len(), vertex_count, run_count] {
        bytes.extend_from_slice(&u32::try_from(count).expect("tile too large").to_le_bytes());
    }
    bytes.extend_from_slice(&[0; 12]);
    let mut offset = 0u32;
    bytes.extend_from_slice(&offset.to_le_bytes());
    for runs in &cells {
        offset += runs.len() as u32;
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    for run in cells.iter().flatten() {
        bytes.extend_from_slice(&run.outline.to_le_bytes());
        bytes.extend_from_slice(&run.first_edge.to_le_bytes());
        bytes.extend_from_slice(&run.edge_count.to_le_bytes());
    }
    let mut first_vertex = 0u32;
    for outline in outlines {
        bytes.extend_from_slice(&outline.footprint_id.to_le_bytes());
        bytes.extend_from_slice(&first_vertex.to_le_bytes());
        bytes.extend_from_slice(&(outline.vertices.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&height_code(outline.height_m).to_le_bytes());
        bytes.extend_from_slice(&[outline.kind as u8, outline.envelope as u8, 0, 0]);
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
