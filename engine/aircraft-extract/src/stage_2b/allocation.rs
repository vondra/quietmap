//! Admit complete fold buckets from produced counts before allocating their exact-flight sets.

use super::*;
use crate::arrow_io::{inspect_ipc_allocation, CruiseSpillCounts};
use std::mem::size_of;
use std::os::unix::fs::MetadataExt;

pub(super) struct FoldBucket {
    pub parts: Vec<PathBuf>,
    pub counts: CruiseSpillCounts,
    pub file_bytes: u64,
    pub allocated_bytes: u64,
    pub allocation_bytes: u64,
}

/// Sorted-streaming fold working set: the slim sort array plus the packed
/// arenas, reserved exactly (no Vec doubling transient), with 5/4 slack for
/// the live key accumulator, per-square row vecs and allocator rounding.
/// The decoded Arrow batch and the per-square output builders coexist.
pub(super) fn fold_sort_allocation(counts: CruiseSpillCounts, largest_batch: u64) -> u64 {
    let slim = counts.rows.saturating_mul(super::fold::SLIM_ROW_BYTES);
    let arena = counts
        .fids
        .saturating_mul(size_of::<u64>())
        .saturating_add(counts.candidates.saturating_mul(super::fold::ARENA_CAND_BYTES))
        .saturating_add(counts.callsign_bytes);
    // Per-square finalized rows and Arrow builders before each square file.
    const SQUARE_BUILDERS_BYTES: u64 = 1024 * 1024 * 1024;
    (slim.saturating_add(arena) as u64)
        .saturating_mul(5)
        .saturating_div(4)
        .saturating_add(largest_batch)
        .saturating_add(SQUARE_BUILDERS_BYTES)
}

// A separately allocated one-entry table has four slots; larger tables need
// fewer slots per entry. Include control bytes, alignment and allocator headers.
fn table_entries<T>(entries: usize) -> usize {
    entries * (4 * (size_of::<T>() + 1) + 32)
}

pub(super) fn fold_map_allocation(counts: CruiseSpillCounts) -> usize {
    table_entries::<(u64, HashMap<CruiseKey, CruiseAccum>)>(counts.rows)
        + table_entries::<(CruiseKey, CruiseAccum)>(counts.rows)
        + table_entries::<u64>(counts.fids)
        + table_entries::<(u64, CruiseTopCandidate)>(counts.candidates)
        + counts.candidates * 32
        + counts.callsign_bytes
}

pub(super) fn fold_inputs(spill_dir: &Path) -> Result<Vec<FoldBucket>> {
    let overhead = crate::arrow_io::spill_file_overhead_bound()?;
    (0..SPILL_HASH_BUCKETS as usize)
        .into_par_iter()
        .map(|bucket| {
            let parts = list_spill_parts(&spill_bucket_dir(spill_dir, bucket as u64))?;
            let mut counts = CruiseSpillCounts::default();
            let mut file_bytes = 0;
            let mut allocated_bytes = 0;
            let mut largest_batch = 0;
            for path in &parts {
                let part = CruiseSpillCounts::read(path)?;
                let ipc = inspect_ipc_allocation(path)?;
                anyhow::ensure!(part.rows == ipc.rows, "spill count disagrees with IPC rows");
                anyhow::ensure!(
                    ipc.file_bytes <= part.encoded_buffers_bytes() as u64 + overhead,
                    "spill byte count disagrees with produced counts"
                );
                counts.merge(part);
                file_bytes += ipc.file_bytes;
                allocated_bytes += path.metadata()?.blocks() * 512;
                largest_batch = largest_batch.max(ipc.largest_batch_bytes);
            }
            let allocation_bytes = if parts.is_empty() {
                0
            } else {
                fold_sort_allocation(counts, largest_batch)
            };
            Ok(FoldBucket {
                parts,
                counts,
                file_bytes,
                allocated_bytes,
                allocation_bytes,
            })
        })
        .collect()
}

pub(super) fn transit_allocation(callsign_bytes: usize) -> usize {
    fold_map_allocation(CruiseSpillCounts {
        rows: 1,
        fids: 1,
        candidates: 1,
        callsign_bytes,
    })
}

// All bucket path lists remain alive while workers fold. Charge their allocated
// capacities once in the global plan instead of hiding them in worker buffers.
pub(super) fn retained_part_paths_allocation(parts: &Vec<PathBuf>) -> u64 {
    (parts.capacity() * size_of::<PathBuf>()
        + parts.iter().map(|path| path.capacity() + 32).sum::<usize>()) as u64
}

pub(super) fn retained_paths_allocation(inputs: &[FoldBucket]) -> u64 {
    inputs
        .iter()
        .map(|input| retained_part_paths_allocation(&input.parts))
        .sum::<u64>()
        + std::mem::size_of_val(inputs) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_flight_sets_and_string_bytes_raise_allocation_allowance() {
        let base = CruiseSpillCounts {
            rows: 1,
            fids: 50,
            candidates: 50,
            callsign_bytes: 400,
        };
        let many_flights = CruiseSpillCounts {
            fids: 50_000,
            ..base
        };
        assert!(
            fold_map_allocation(many_flights)
                >= fold_map_allocation(base) + 49_950 * size_of::<u64>()
        );
        assert_eq!(transit_allocation(4096) - transit_allocation(0), 4096);
    }
}
