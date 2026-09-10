//! Count owned rows per square during scatter to size and reserve gather allocations.

use super::{shuffle_bucket, PASS_A_SPILL_BYTES, SHUFFLE_HASH_BUCKETS};
use crate::arrow_io::SEGMENT_WRITE_CHUNK_ROWS;
use crate::flight::{FlightSegment, Phase};

#[derive(Clone, Copy, Default)]
struct SquareRows {
    rows: usize,
    callsign_bytes: usize,
}

pub(super) struct DestinationCounts {
    squares: [Vec<SquareRows>; 2],
    pub scattered_rows: u64,
}

impl DestinationCounts {
    pub fn new() -> Self {
        Self {
            squares: std::array::from_fn(|_| {
                vec![SquareRows::default(); (grid::MAX_SQUARE_ID + 1) as usize]
            }),
            scattered_rows: 0,
        }
    }

    pub fn add(&mut self, phase: Phase, square: u64, callsign_bytes: usize) {
        let entry = &mut self.squares[phase.as_u8() as usize][square as usize];
        entry.rows += 1;
        entry.callsign_bytes += callsign_bytes;
    }

    pub fn merge(&mut self, other: Self) {
        self.scattered_rows += other.scattered_rows;
        for (phase, incoming) in self.squares.iter_mut().zip(other.squares) {
            for (entry, addition) in phase.iter_mut().zip(incoming) {
                entry.rows += addition.rows;
                entry.callsign_bytes += addition.callsign_bytes;
            }
        }
    }

    pub fn rows(&self, phase: Phase, square: u64) -> usize {
        self.squares[phase.as_u8() as usize][square as usize].rows
    }

    pub fn square_counts_by_bucket(&self) -> [[usize; SHUFFLE_HASH_BUCKETS as usize]; 2] {
        let mut counts = [[0; SHUFFLE_HASH_BUCKETS as usize]; 2];
        for (phase, squares) in self.squares.iter().enumerate() {
            for (square, _) in squares
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.rows > 0)
            {
                counts[phase][shuffle_bucket(square as u64) as usize] += 1;
            }
        }
        counts
    }

    pub fn largest_gather_allocation(&self) -> usize {
        let mut largest = 0;
        for phase in &self.squares {
            let mut vectors_and_strings = [0; SHUFFLE_HASH_BUCKETS as usize];
            let mut largest_writer = [0; SHUFFLE_HASH_BUCKETS as usize];
            for (square, entry) in phase.iter().enumerate().filter(|(_, entry)| entry.rows > 0) {
                let hash = shuffle_bucket(square as u64) as usize;
                // Gather reserves exactly the counted rows. The per-string allowance
                // covers allocator alignment/bookkeeping; 256 B covers each map entry.
                vectors_and_strings[hash] += entry.rows
                    * (std::mem::size_of::<FlightSegment>() + 32)
                    + entry.callsign_bytes
                    + 256;
                // The streaming writer retains one Arrow batch while source rows remain alive.
                largest_writer[hash] = largest_writer[hash]
                    .max(100 * entry.rows.min(SEGMENT_WRITE_CHUNK_ROWS) + 2 * entry.callsign_bytes);
            }
            largest = largest.max(
                vectors_and_strings
                    .into_iter()
                    .zip(largest_writer)
                    .map(|(rows, writer)| rows + writer)
                    .max()
                    .unwrap_or(0),
            );
        }
        // One scatter part can coexist in IPC, decoded, and serialization buffers.
        largest + 5 * PASS_A_SPILL_BYTES
    }
}
