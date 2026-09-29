//! Reading `obstacles` files: every count, offset and index checked once, then plain accessors.

use super::{
    CELLS, EnvelopeClass, HEADER_BYTES, MAGIC, OUTLINE_BYTES, OutlineKind, OutlineRecord,
    RUN_BYTES, Run, VERTEX_BYTES,
};
use crate::FormatError;

/// A parsed obstacles file borrowing its bytes; every index in it was checked by [`Self::parse`].
#[derive(Clone, Copy)]
pub struct Obstacles<'a> {
    cell_offsets: &'a [u8],
    runs: &'a [u8],
    outlines: &'a [u8],
    vertices: &'a [u8],
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

impl<'a> Obstacles<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(FormatError("obstacles: bad magic"));
        }
        if bytes[20..HEADER_BYTES].iter().any(|&byte| byte != 0) {
            return Err(FormatError("obstacles: reserved header bytes set"));
        }
        let count = |at: usize| u32_at(bytes, at) as usize;
        let (outlines, vertices, runs) = (count(8), count(12), count(16));
        let offsets_end = HEADER_BYTES + 4 * (CELLS + 1);
        let runs_end = offsets_end + RUN_BYTES * runs;
        let outlines_end = runs_end + OUTLINE_BYTES * outlines;
        if bytes.len() != outlines_end + VERTEX_BYTES * vertices {
            return Err(FormatError("obstacles: length does not match the counts"));
        }
        let parsed = Obstacles {
            cell_offsets: &bytes[HEADER_BYTES..offsets_end],
            runs: &bytes[offsets_end..runs_end],
            outlines: &bytes[runs_end..outlines_end],
            vertices: &bytes[outlines_end..],
        };
        parsed.check_indices()?;
        Ok(parsed)
    }

    fn check_indices(&self) -> Result<(), FormatError> {
        if self.offset(0) != 0
            || self.offset(CELLS) != self.run_count()
            || (0..CELLS).any(|cell| self.offset(cell + 1) < self.offset(cell))
        {
            return Err(FormatError("obstacles: cell offsets out of order"));
        }
        let mut previous_id = 0;
        for index in 0..self.outline_count() {
            let record = &self.outlines[index * OUTLINE_BYTES..][..OUTLINE_BYTES];
            let (first, count) = (u32_at(record, 8) as usize, usize::from(u16_at(record, 12)));
            let id = u64::from_le_bytes(record[..8].try_into().unwrap());
            let kind = OutlineKind::ALL.get(usize::from(record[16])).copied();
            if kind.is_none() || EnvelopeClass::from_code(record[17]).is_none() {
                return Err(FormatError("obstacles: unknown outline kind or envelope"));
            }
            if u16_at(record, 14) == 0 || record[18..].iter().any(|&byte| byte != 0) {
                return Err(FormatError("obstacles: zero height or reserved bytes set"));
            }
            if first + count > self.vertex_count() || id < previous_id {
                return Err(FormatError("obstacles: outline out of range or order"));
            }
            let well_formed = match kind {
                Some(OutlineKind::Wall) => count >= 2,
                _ => count >= 4 && self.vertex(first) == self.vertex(first + count - 1),
            };
            if !well_formed {
                return Err(FormatError("obstacles: open ring or short outline"));
            }
            previous_id = id;
        }
        for index in 0..self.run_count() {
            let run = self.run(index);
            let outline = run.outline as usize;
            if outline >= self.outline_count()
                || run.edge_count == 0
                || usize::from(run.first_edge) + usize::from(run.edge_count)
                    >= self.outline(outline).vertex_count
            {
                return Err(FormatError("obstacles: run out of range"));
            }
        }
        Ok(())
    }

    fn offset(&self, cell: usize) -> usize {
        u32_at(self.cell_offsets, 4 * cell) as usize
    }

    pub fn run(&self, index: usize) -> Run {
        let record = &self.runs[index * RUN_BYTES..][..RUN_BYTES];
        Run {
            outline: u32_at(record, 0),
            first_edge: u16_at(record, 4),
            edge_count: u16_at(record, 6),
        }
    }

    pub fn outline_count(&self) -> usize {
        self.outlines.len() / OUTLINE_BYTES
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / VERTEX_BYTES
    }

    pub fn run_count(&self) -> usize {
        self.runs.len() / RUN_BYTES
    }

    /// The indices of one cell's runs (cell `row * 128 + column`).
    pub fn cell_run_range(&self, cell: usize) -> std::ops::Range<usize> {
        self.offset(cell)..self.offset(cell + 1)
    }

    /// The runs of one cell.
    pub fn cell_runs(&self, cell: usize) -> impl ExactSizeIterator<Item = Run> + use<'a> {
        let parsed = *self;
        parsed
            .cell_run_range(cell)
            .map(move |index| parsed.run(index))
    }

    pub fn outline(&self, index: usize) -> OutlineRecord {
        let record = &self.outlines[index * OUTLINE_BYTES..][..OUTLINE_BYTES];
        OutlineRecord {
            footprint_id: u64::from_le_bytes(record[..8].try_into().unwrap()),
            kind: OutlineKind::ALL[usize::from(record[16])],
            envelope: EnvelopeClass::ALL[usize::from(record[17])],
            height_m: f64::from(u16_at(record, 14)) / 10.0,
            first_vertex: u32_at(record, 8) as usize,
            vertex_count: usize::from(u16_at(record, 12)),
        }
    }

    pub fn vertex(&self, index: usize) -> [i16; 2] {
        let at = index * VERTEX_BYTES;
        let v = &self.vertices[at..at + VERTEX_BYTES];
        [
            i16::from_le_bytes([v[0], v[1]]),
            i16::from_le_bytes([v[2], v[3]]),
        ]
    }
}
