//! Sorted-streaming fold: rows sort by (square, key) once, then merge sequentially.
//!
//! The hash-table fold held every key of a bucket live at once (tens of GB
//! for the largest bucket, serializing the whole stage on admission). Rows
//! arrive pre-counted, so instead: stream them into a slim sort array plus a
//! packed arena, sort, and merge each key's consecutive run into one live
//! accumulator. Per-bucket memory is ~1.1x the encoded bytes with an exact
//! reservation, and the merge order is a total order — bit-identical across
//! runs, where the hash order was not.

use super::*;

use crate::arrow_io::for_each_cruise_spill;
use crate::flight::CruiseBucket;
use std::path::PathBuf;

pub(super) const SLIM_ROW_BYTES: usize = std::mem::size_of::<FoldSlim>();
pub(super) const ARENA_CAND_BYTES: usize = std::mem::size_of::<PackedCand>();

/// One spill row's sort key, sums and arena references. Packed to keep the
/// sorted array small: the largest world bucket holds tens of millions.
pub(super) struct FoldSlim {
    square: u64,
    key: CruiseKey,
    file: u32,
    row: u32,
    sum_length_m: f32,
    weight: f32,
    rep_alt_m: f32,
    rep_speed_kt: f32,
    rep_profile_idx: u8,
    source_id: u8,
    origin: u8,
    fid_start: u32,
    fid_len: u32,
    cand_start: u32,
    cand_len: u32,
}

/// Candidate with its callsign as a blob span instead of a heap String.
struct PackedCand {
    flight_id: u64,
    callsign_start: u32,
    callsign_len: u32,
    aircraft_type: [u8; 4],
    peak_lmax_25m_db: f32,
    altitude_m: f32,
}

impl PackedCand {
    fn to_candidate(&self, blob: &[u8]) -> CruiseTopCandidate {
        let bytes = &blob[self.callsign_start as usize..][..self.callsign_len as usize];
        CruiseTopCandidate {
            flight_id: self.flight_id,
            callsign: String::from_utf8(bytes.to_vec()).expect("callsign bytes round-trip"),
            aircraft_type: self.aircraft_type,
            peak_lmax_25m_db: self.peak_lmax_25m_db,
            altitude_m: self.altitude_m,
        }
    }
}

pub(super) struct FoldArena {
    fids: Vec<u64>,
    cands: Vec<PackedCand>,
    blob: Vec<u8>,
}

/// Merge one key's run into `accum`: sums add, fids union, candidates
/// replay through the re-entrant cap-K logic so the accumulator holds the
/// true top-K of the union (bounded rank pollution at the Kth slot: two
/// capped top-50 lists union to top-50 of top-100). `rep_profile_idx` /
/// `source_id` / `origin` are NOT invariant per key — different
/// `profile_idx` can map to the same `class`; the first row's win, and
/// downstream remaps `profile_idx` → class so the pick has no measurable
/// effect.
fn absorb_run(accum: &mut CruiseAccum, slim: &[FoldSlim], arena: &FoldArena) {
    for (i, row) in slim.iter().enumerate() {
        if i == 0 {
            accum.rep_profile_idx = row.rep_profile_idx;
            accum.source_id = row.source_id;
            accum.origin = row.origin;
        }
        accum.sum_length_m += row.sum_length_m;
        accum.weight += row.weight;
        accum.rep_alt_m += row.rep_alt_m;
        accum.rep_speed_kt += row.rep_speed_kt;
        for fid in &arena.fids[row.fid_start as usize..][..row.fid_len as usize] {
            accum.fid_set.insert(*fid);
        }
        for cand in &arena.cands[row.cand_start as usize..][..row.cand_len as usize] {
            accum.merge_top_entry(cand.to_candidate(&arena.blob));
        }
    }
}

/// Fold one bucket's parts, emitting each in-scope square's canonical rows
/// in ascending square order with keys ascending (the driver's row sort is
/// subsumed). Returns the canonical row count.
pub(super) fn fold_bucket_sorted(
    parts: &[PathBuf],
    scope: Option<&ScopeBbox>,
    counts: crate::arrow_io::CruiseSpillCounts,
    mut emit: impl FnMut(u64, Vec<CruiseBucket>) -> Result<()>,
) -> Result<u64> {
    let mut slim: Vec<FoldSlim> = Vec::new();
    let mut arena = FoldArena {
        fids: Vec::new(),
        cands: Vec::new(),
        blob: Vec::new(),
    };
    // Exact reservation: the inputs were counted before admission, so no
    // Vec ever grows (no doubling transient) and the charge holds tight.
    slim.reserve_exact(counts.rows);
    arena.fids.reserve_exact(counts.fids);
    arena.cands.reserve_exact(counts.candidates);
    arena.blob.reserve_exact(counts.callsign_bytes);
    for (file, path) in parts.iter().enumerate() {
        let file = u32::try_from(file).context("bucket part count exceeds u32")?;
        let mut row_idx = 0u32;
        for_each_cruise_spill(path, |row| {
            let row_id = row_idx;
            row_idx = row_idx.checked_add(1).context("spill part rows exceed u32")?;
            let fid_start = u32::try_from(arena.fids.len()).context("bucket fids exceed u32")?;
            let fid_len =
                u32::try_from(row.fid_set.len()).context("spill row fids exceed u32")?;
            arena.fids.extend_from_slice(&row.fid_set);
            let cand_start =
                u32::try_from(arena.cands.len()).context("bucket candidates exceed u32")?;
            let cand_len = u32::try_from(row.top_candidates.len())
                .context("spill row candidates exceed u32")?;
            for cand in &row.top_candidates {
                let callsign_start =
                    u32::try_from(arena.blob.len()).context("callsign blob exceeds u32")?;
                let callsign_len = u32::try_from(cand.callsign.len())
                    .context("callsign exceeds u32 bytes")?;
                arena.blob.extend_from_slice(cand.callsign.as_bytes());
                arena.cands.push(PackedCand {
                    flight_id: cand.flight_id,
                    callsign_start,
                    callsign_len,
                    aircraft_type: cand.aircraft_type,
                    peak_lmax_25m_db: cand.peak_lmax_25m_db,
                    altitude_m: cand.altitude_m,
                });
            }
            slim.push(FoldSlim {
                square: row.square,
                key: CruiseKey {
                    cruise_cell_id: row.cruise_cell_id,
                    class: row.class,
                    fl_bin: row.fl_bin,
                    period: row.period,
                    heading_bin: row.heading_bin,
                    secondary_only: row.secondary_only,
                },
                file,
                row: row_id,
                sum_length_m: row.sum_length_m,
                weight: row.weight,
                rep_alt_m: row.rep_alt_m,
                rep_speed_kt: row.rep_speed_kt,
                rep_profile_idx: row.rep_profile_idx,
                source_id: row.source_id,
                origin: row.origin,
                fid_start,
                fid_len,
                cand_start,
                cand_len,
            });
            Ok(())
        })?;
    }
    // Total order: (file, row) disambiguates identical keys, so the merge
    // sequence — and every float sum — is fixed across runs.
    slim.sort_unstable_by(|a, b| {
        (a.square, a.key, a.file, a.row).cmp(&(b.square, b.key, b.file, b.row))
    });
    let mut canonical_rows = 0u64;
    let mut pending: Vec<CruiseBucket> = Vec::new();
    let mut pending_square: Option<u64> = None;
    let mut i = 0;
    while i < slim.len() {
        let square = slim[i].square;
        let key = slim[i].key;
        let mut j = i + 1;
        while j < slim.len() && slim[j].square == square && slim[j].key == key {
            j += 1;
        }
        // One scope check per distinct square (rows arrive grouped).
        if scope.is_some_and(|scope| !scope.contains_square(square)) {
            i = j;
            continue;
        }
        let mut accum = CruiseAccum::default();
        // First-row representative fields win, exactly as merge-into-first.
        absorb_run(&mut accum, &slim[i..j], &arena);
        if pending_square != Some(square) {
            if let Some(prev) = pending_square {
                canonical_rows += pending.len() as u64;
                emit(prev, std::mem::take(&mut pending))?;
            }
            pending_square = Some(square);
        }
        pending.push(accum.finalize(key));
        i = j;
    }
    if let Some(prev) = pending_square {
        canonical_rows += pending.len() as u64;
        emit(prev, pending)?;
    }
    Ok(canonical_rows)
}
