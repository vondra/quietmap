//! Reading `obstacles` tiles: every count, offset and index checked once, then plain accessors.

use super::{
    CELL_WORDS, CELLS, ENVELOPE_SHIFT, EnvelopeClass, HEADER_BYTES, MAGIC, MAXIMUM_SQUARES,
    OUTLINE_BYTES, OutlineKind, OutlineRecord, RUN_BYTES, Run, VERTEX_BYTES,
};
use crate::FormatError;

/// A parsed obstacles tile borrowing its bytes; every index in it was checked by [`Self::parse`].
#[derive(Clone, Copy)]
pub struct Obstacles<'a> {
    occupied_words: &'a [u8],
    occupied_before: &'a [u8],
    first_runs: &'a [u8],
    squares: &'a [u8],
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
        let count = |at: usize| u32_at(bytes, at) as usize;
        let (outlines, vertices, runs, occupied, squares) =
            (count(8), count(12), count(16), count(20), count(24));
        if count(28) != 0 || occupied > CELLS || squares > MAXIMUM_SQUARES {
            return Err(FormatError("obstacles: bad header"));
        }
        let lengths = [
            8 * CELL_WORDS,
            2 * CELL_WORDS,
            4 * (occupied + 1),
            4 * squares,
            RUN_BYTES.saturating_mul(runs),
            OUTLINE_BYTES.saturating_mul(outlines),
            VERTEX_BYTES.saturating_mul(vertices),
        ];
        let mut end = HEADER_BYTES;
        let sections = lengths.map(|length| {
            let start = end;
            end = end.saturating_add(length);
            start..end
        });
        if bytes.len() != end {
            return Err(FormatError("obstacles: length does not match the counts"));
        }
        let [
            words_at,
            before_at,
            first_runs_at,
            squares_at,
            runs_at,
            outlines_at,
            vertices_at,
        ] = sections;
        let parsed = Obstacles {
            occupied_words: &bytes[words_at],
            occupied_before: &bytes[before_at],
            first_runs: &bytes[first_runs_at],
            squares: &bytes[squares_at],
            runs: &bytes[runs_at],
            outlines: &bytes[outlines_at],
            vertices: &bytes[vertices_at],
        };
        parsed.check_indices(occupied)?;
        Ok(parsed)
    }

    fn check_indices(&self, occupied: usize) -> Result<(), FormatError> {
        let mut before = 0;
        for word in 0..CELL_WORDS {
            if usize::from(u16_at(self.occupied_before, 2 * word)) != before {
                return Err(FormatError("obstacles: occupied cells miscounted"));
            }
            before += self.word(word).count_ones() as usize;
        }
        if before != occupied
            || self.first_run(0) != 0
            || self.first_run(occupied) != self.run_count()
            || (0..occupied).any(|rank| self.first_run(rank + 1) <= self.first_run(rank))
        {
            return Err(FormatError("obstacles: cell runs out of order"));
        }
        let squares = self.squares.len() / 4;
        if (1..squares)
            .any(|row| u32_at(self.squares, 4 * row) <= u32_at(self.squares, 4 * row - 4))
        {
            return Err(FormatError("obstacles: footprint squares out of order"));
        }
        if self.outline_count() > 0 && self.first_vertex(0) != 0 {
            return Err(FormatError("obstacles: vertices before the first outline"));
        }
        let mut previous_id = 0;
        for index in 0..self.outline_count() {
            let record = self.outline_bytes(index);
            let kind = OutlineKind::ALL.get(usize::from(record[11] & 3)).copied();
            if kind.is_none() || EnvelopeClass::from_code(record[11] >> ENVELOPE_SHIFT).is_none() {
                return Err(FormatError("obstacles: unknown outline kind or envelope"));
            }
            if u16_at(record, 8) == 0 || usize::from(record[10]) >= squares {
                return Err(FormatError("obstacles: zero height or a missing square"));
            }
            let (first, end) = (self.first_vertex(index), self.end_vertex(index));
            let id = self.footprint_id(record);
            if end < first || end > self.vertex_count() || id < previous_id {
                return Err(FormatError("obstacles: outline out of range or order"));
            }
            let count = end - first;
            let well_formed = match kind {
                Some(OutlineKind::Wall) => count >= 2,
                _ => count >= 4 && self.vertex(first) == self.vertex(end - 1),
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

    fn word(&self, word: usize) -> u64 {
        u64::from_le_bytes(
            self.occupied_words[8 * word..8 * word + 8]
                .try_into()
                .unwrap(),
        )
    }

    fn first_run(&self, rank: usize) -> usize {
        u32_at(self.first_runs, 4 * rank) as usize
    }

    fn outline_bytes(&self, index: usize) -> &'a [u8] {
        &self.outlines[index * OUTLINE_BYTES..][..OUTLINE_BYTES]
    }

    fn first_vertex(&self, outline: usize) -> usize {
        u32_at(self.outline_bytes(outline), 0) as usize
    }

    /// Where an outline's vertices end: where the next one's begin, the last's at the end.
    fn end_vertex(&self, outline: usize) -> usize {
        if outline + 1 < self.outline_count() {
            self.first_vertex(outline + 1)
        } else {
            self.vertex_count()
        }
    }

    /// An outline record's footprint id: its square's high half and its ordinal.
    fn footprint_id(&self, record: &[u8]) -> u64 {
        u64::from(u32_at(self.squares, 4 * usize::from(record[10]))) << 32
            | u64::from(u32_at(record, 4))
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

    /// The cells with runs: the bitmap's set bits.
    pub fn occupied_count(&self) -> usize {
        self.first_runs.len() / 4 - 1
    }

    /// The footprint squares the tile names.
    pub fn square_count(&self) -> usize {
        self.squares.len() / 4
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / VERTEX_BYTES
    }

    pub fn run_count(&self) -> usize {
        self.runs.len() / RUN_BYTES
    }

    /// The indices of one cell's runs (cell `row * 128 + column`): none for an empty cell, else
    /// those its rank among the occupied cells names.
    pub fn cell_run_range(&self, cell: usize) -> std::ops::Range<usize> {
        let (word, bit) = (self.word(cell / 64), cell % 64);
        if word & 1 << bit == 0 {
            return 0..0;
        }
        let rank = usize::from(u16_at(self.occupied_before, 2 * (cell / 64)))
            + (word & ((1 << bit) - 1)).count_ones() as usize;
        self.first_run(rank)..self.first_run(rank + 1)
    }

    /// The runs of one cell.
    pub fn cell_runs(&self, cell: usize) -> impl ExactSizeIterator<Item = Run> + use<'a> {
        let parsed = *self;
        parsed
            .cell_run_range(cell)
            .map(move |index| parsed.run(index))
    }

    pub fn outline(&self, index: usize) -> OutlineRecord {
        let record = self.outline_bytes(index);
        let first_vertex = self.first_vertex(index);
        OutlineRecord {
            footprint_id: self.footprint_id(record),
            kind: OutlineKind::ALL[usize::from(record[11] & 3)],
            envelope: EnvelopeClass::ALL[usize::from(record[11] >> ENVELOPE_SHIFT)],
            height_m: f64::from(u16_at(record, 8)) / 10.0,
            first_vertex,
            vertex_count: self.end_vertex(index) - first_vertex,
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
