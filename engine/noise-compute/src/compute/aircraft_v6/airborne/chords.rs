//! Split chords: pieces stored as rows are re-chained into one event for the
//! 20 dB floors and one Noise Segments slot, and drawn as one polyline.
//!
//! A chord longer than `AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M` is stored as
//! `SPLIT_PIECE` rows sharing their endpoint values exactly, the first flagged
//! `CHORD_START`, the last `CHORD_END` (`aircraft-extract::segment::split`).
//! The kernel evaluates every piece unfloored; here the pieces that passed
//! the envelope and reach gates are chained through their shared endpoints
//! into chords, the chord's summed free and received SEL take the two 20 dB
//! floors an unsplit sub-segment takes inside the kernel, the pieces fold into
//! the flight in row order, and a chord competes for one trace slot with the
//! sum of its pieces' energies. Pieces beyond the envelope or the class reach
//! were never evaluated: they are dropped, each below the reach threshold.

use std::collections::HashMap;

use crate::compute::aircraft_v6::state::FlightAccum;
use crate::compute::aircraft_v6::views::AirborneSegmentBatch;
use crate::types::SegmentTrace;

use super::row::{build_row_trace, evaluate_row, flight_accumulator, RowKernel, ScatterContext};
use super::{AIRBORNE_RANK_W, AIRBORNE_TRACE_CUTOFF_DB};

pub const SPLIT_PIECE: u8 = 1 << 3;
pub const CHORD_START: u8 = 1 << 4;

/// One evaluated split piece, kept until its chord is known.
pub(super) struct PieceEval {
    pub batch: usize,
    pub row: usize,
    pub order: u64,
    pub flight_id: u64,
    pub start: (i32, i32),
    pub end: (i32, i32),
    pub flags: u8,
    pub period: usize,
    pub class_weight: f64,
    pub free_raw: f64,
    pub received_raw: f64,
    pub lmax: f64,
    pub energy: f64,
    pub kernel: RowKernel,
}

impl PieceEval {
    pub fn new(
        batch: usize,
        row: usize,
        order: u64,
        view: &AirborneSegmentBatch<'_>,
        kernel: RowKernel,
    ) -> Self {
        let raw = |sel: f64| 10f64.powf(sel / 10.0);
        Self {
            batch,
            row,
            order,
            flight_id: view.flight_id[row],
            start: (view.start_gx[row], view.start_gy[row]),
            end: (view.end_gx[row], view.end_gy[row]),
            flags: view.flags[row],
            period: kernel.period,
            class_weight: kernel.class_weight,
            free_raw: raw(kernel.kernel.free_sel),
            received_raw: raw(kernel.kernel.sel),
            lmax: kernel.lmax,
            energy: kernel.energy,
            kernel,
        }
    }
}

/// A chord that passed the received floor and can hold a trace slot.
pub(super) struct ChordCandidate {
    pub rank_key: f64,
    pub order: u64,
    pieces: Vec<usize>,
}

/// Group `pieces` (chunk order = row order) into chords, apply the floors,
/// fold the survivors into `flights` and return the trace candidates.
/// Returns the count of pieces above the trace cutoff for the "N visible"
/// denominator.
pub(super) fn fold_chords(
    pieces: &[PieceEval],
    batches: &[AirborneSegmentBatch<'_>],
    flights: &mut HashMap<u64, FlightAccum>,
    want_traces: bool,
) -> (Vec<ChordCandidate>, u32) {
    let mut by_end: HashMap<(u64, i32, i32), usize> = HashMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        by_end
            .entry((piece.flight_id, piece.end.0, piece.end.1))
            .or_insert(index);
    }
    // Chord key: the first evaluated piece of the chain (a piece before it may
    // lie beyond reach); chords are visited in first-appearance order.
    let mut chords: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut chord_of_head: HashMap<usize, usize> = HashMap::new();
    for index in 0..pieces.len() {
        let mut head = index;
        let mut steps = 0;
        while pieces[head].flags & CHORD_START == 0 {
            let piece = &pieces[head];
            match by_end.get(&(piece.flight_id, piece.start.0, piece.start.1)) {
                Some(&previous) if previous != head && steps < pieces.len() => {
                    head = previous;
                    steps += 1;
                }
                _ => break,
            }
        }
        let chord = *chord_of_head.entry(head).or_insert_with(|| {
            chords.push((head, Vec::new()));
            chords.len() - 1
        });
        chords[chord].1.push(index);
    }
    let mut candidates = Vec::new();
    let mut above_cutoff = 0u32;
    for (_, members) in chords {
        let sum =
            |value: fn(&PieceEval) -> f64| members.iter().map(|&i| value(&pieces[i])).sum::<f64>();
        // The kernel's free-field floor: an unsplit sub-segment below it is
        // never evaluated at all.
        if 10.0 * sum(|p| p.free_raw).log10() < 20.0 {
            continue;
        }
        for &i in &members {
            let piece = &pieces[i];
            piece.kernel.apply_variants(flight_accumulator(
                flights,
                &batches[piece.batch],
                piece.row,
                piece.class_weight,
            ));
        }
        // The popup's received floor: below it the chord keeps its
        // pre-screen variants but no received energy, peak or trace.
        if 10.0 * sum(|p| p.received_raw).log10() < 20.0 {
            continue;
        }
        let mut rank_key = 0.0;
        let mut visible = 0;
        for &i in &members {
            let piece = &pieces[i];
            piece.kernel.apply_received(flight_accumulator(
                flights,
                &batches[piece.batch],
                piece.row,
                piece.class_weight,
            ));
            rank_key += piece.energy * AIRBORNE_RANK_W[piece.period];
            if piece.lmax >= AIRBORNE_TRACE_CUTOFF_DB {
                visible += 1;
            }
        }
        above_cutoff = above_cutoff.saturating_add(visible);
        if want_traces && visible > 0 {
            candidates.push(ChordCandidate {
                rank_key,
                order: members.iter().map(|&i| pieces[i].order).min().unwrap_or(0),
                pieces: members,
            });
        }
    }
    (candidates, above_cutoff)
}

/// Every piece of a selected chord, drawn as one polyline of N traces.
pub(super) fn chord_traces(
    ctx: &ScatterContext<'_>,
    batches: &[AirborneSegmentBatch<'_>],
    pieces: &[PieceEval],
    chord: &ChordCandidate,
) -> Vec<SegmentTrace> {
    chord
        .pieces
        .iter()
        .filter_map(|&i| {
            let piece = &pieces[i];
            let batch = &batches[piece.batch];
            let row = evaluate_row::<false>(ctx, batch, piece.row)?;
            Some(build_row_trace(ctx, batch, piece.row, &row))
        })
        .collect()
}
