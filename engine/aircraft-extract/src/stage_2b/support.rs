//! Copy finalized cruise rows to receiver support cells without aggregating publication copies.

use super::*;
use crate::arrow_io::{
    inspect_ipc_allocation, read_record_batches, required_column, write_record_batch_stream,
};
use arrow::array::{Float64Array, UInt8Array};
use arrow::compute::interleave_record_batch;

pub(super) type IndexedCruiseRow = ((u64, u8, u8, u8), (usize, usize));

pub(super) fn scatter_finalized_cruise(
    buckets: impl Iterator<Item = CruiseBucket>,
    directory: &Path,
    part_id: &AtomicU64,
    n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<u64> {
    let mut copies: HashMap<u64, Vec<CruiseBucket>> = HashMap::new();
    let mut buffered_bytes = 0;
    let mut canonical_rows = 0;
    for bucket in buckets {
        canonical_rows += 1;
        let (lon, lat) = grid::cruise::cruise_centroid(bucket.cruise_cell_id);
        let support =
            noise_compute::emission::aircraft::cruise_support_cells(lat, lon, bucket.rep_len_m)
                .context("invalid finalized cruise support")?;
        // Capacity growth for destination vectors and singleton maps, plus each
        // independently allocated candidate vector/string, counts before cloning.
        let row_bytes = support_row_allocation(&bucket);
        for square in support.iter().map(|square| grid::square_id(square) as u64) {
            if scope.is_some_and(|scope| !scope.contains_square(square)) {
                continue;
            }
            if buffered_bytes + row_bytes > SPILL_TRIGGER_BYTES {
                flush_copies(&mut copies, directory, part_id, n_days)?;
                buffered_bytes = 0;
            }
            anyhow::ensure!(
                row_bytes <= SPILL_TRIGGER_BYTES,
                "one cruise support row exceeds the staging allocation"
            );
            copies.entry(square).or_default().push(bucket.clone());
            buffered_bytes += row_bytes;
        }
    }
    flush_copies(&mut copies, directory, part_id, n_days)?;
    Ok(canonical_rows)
}

pub(super) fn support_row_allocation(bucket: &CruiseBucket) -> usize {
    2 * std::mem::size_of::<CruiseBucket>()
        + 4 * (std::mem::size_of::<(u64, Vec<CruiseBucket>)>() + 1)
        + 64
        + bucket
            .top_candidates
            .iter()
            .map(|candidate| {
                std::mem::size_of::<CruiseTopCandidate>() + candidate.callsign.len() + 32
            })
            .sum::<usize>()
}

fn flush_copies(
    copies: &mut HashMap<u64, Vec<CruiseBucket>>,
    directory: &Path,
    part_id: &AtomicU64,
    n_days: u16,
) -> Result<()> {
    for (square, rows) in std::mem::take(copies) {
        let id = part_id.fetch_add(1, Ordering::Relaxed);
        let path = directory
            .join(square_path(square))
            .join(format!("part_{id:016x}.arrow"));
        write_cruise(&path, &rows, n_days)?;
    }
    Ok(())
}

pub(super) fn gather_finalized_cruise(
    directory: &Path,
    prepared: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<(usize, u64)> {
    let destinations = crate::spatial::square_directories(directory)?;
    let mut inputs = Vec::with_capacity(destinations.len());
    let mut largest_allocation = 0u64;
    let mut support_bytes = 0u64;
    let mut retained_paths = 0u64;
    for (square, path) in destinations {
        let parts = list_spill_parts(&path)?;
        let mut bytes = 0;
        let mut rows = 0;
        let mut batches = 0;
        for part in &parts {
            // Refuse obviously oversized files before reading even their footer.
            crate::memory::max_concurrent_tasks(1, part.metadata()?.len().saturating_mul(2))?;
            let facts = inspect_ipc_allocation(part)?;
            bytes += facts.file_bytes;
            rows += facts.rows;
            batches += facts.batches;
        }
        // Retained IPC + at most double-capacity interleave and serialization
        // buffers. Sorted row indices reserve the counted size exactly.
        let allocation = 5 * bytes
            + (rows * std::mem::size_of::<IndexedCruiseRow>()) as u64
            + (batches
                * (std::mem::size_of::<arrow::record_batch::RecordBatch>()
                    + std::mem::size_of::<usize>())) as u64
            + (arrow_batching::TARGET_ROWS_PER_BATCH * std::mem::size_of::<(usize, usize)>())
                as u64;
        largest_allocation = largest_allocation.max(allocation);
        support_bytes += bytes;
        retained_paths += allocation::retained_part_paths_allocation(&parts);
        crate::memory::max_concurrent_tasks(1, largest_allocation + retained_paths)?;
        inputs.push((square, parts, rows, batches));
    }
    largest_allocation += retained_paths
        + (inputs.capacity() * std::mem::size_of::<(u64, Vec<PathBuf>, usize, usize)>()) as u64;
    let workers =
        crate::memory::max_concurrent_tasks(rayon::current_num_threads(), largest_allocation)?;
    eprintln!("{} [stage2b/gather] {workers} workers; largest allocation {largest_allocation} B; support parts {support_bytes} B", ts());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    // Wipe stale cruise.arrow from in-scope z9s before workers write
    // fresh files. z9s that have no cruise activity this run would
    // otherwise retain a prior-run file (possibly older schema) and
    // the popup reader would fatal-fail on schema_version mismatch.
    let wiped = crate::wipe::wipe_stale_arrows_for_scope(prepared, "cruise.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2b] wiped {wiped} stale cruise.arrow file(s) before write",
            ts()
        );
    }
    let written_rows = AtomicU64::new(0);
    pool.install(|| {
        inputs
            .par_iter()
            .try_for_each(|(square, parts, row_count, batch_count)| -> Result<()> {
                let mut batches = Vec::with_capacity(*batch_count);
                let mut indexed_rows: Vec<IndexedCruiseRow> = Vec::with_capacity(*row_count);
                for path in parts {
                    for batch in read_record_batches(path)?.1 {
                        let lon = required_column::<Float64Array>(&batch, "lon")?;
                        let lat = required_column::<Float64Array>(&batch, "lat")?;
                        let class = required_column::<UInt8Array>(&batch, "class")?;
                        let fl = required_column::<UInt8Array>(&batch, "fl_bin")?;
                        let period = required_column::<UInt8Array>(&batch, "period")?;
                        for row in 0..batch.num_rows() {
                            let key = (
                                grid::cruise::cruise_cell_id(lat.value(row), lon.value(row)),
                                class.value(row),
                                fl.value(row),
                                period.value(row),
                            );
                            indexed_rows.push((key, (batches.len(), row)));
                        }
                        if let Some(first) = batches.first() {
                            let first: &arrow::record_batch::RecordBatch = first;
                            anyhow::ensure!(
                                batch.schema() == first.schema(),
                                "cruise copy schemas disagree"
                            );
                        }
                        batches.push(batch);
                    }
                }
                anyhow::ensure!(
                    indexed_rows.len() == *row_count,
                    "cruise row count changed during gather"
                );
                indexed_rows.sort_unstable_by_key(|(key, _)| *key);
                anyhow::ensure!(
                    indexed_rows.windows(2).all(|pair| pair[0].0 != pair[1].0),
                    "duplicate finalized cruise key in {}",
                    square_path(*square)
                );
                if indexed_rows.is_empty() {
                    return Ok(());
                }
                let refs: Vec<_> = batches.iter().collect();
                let schema = batches[0].schema();
                let output = indexed_rows
                    .chunks(arrow_batching::TARGET_ROWS_PER_BATCH)
                    .map(|chunk| {
                        let indices: Vec<_> = chunk.iter().map(|(_, index)| *index).collect();
                        Ok(interleave_record_batch(&refs, &indices)?)
                    });
                write_record_batch_stream(
                    &prepared.join(square_path(*square)).join("cruise.arrow"),
                    schema.as_ref(),
                    output,
                )?;
                // Durable destination commit precedes retiring its reconstruction parts.
                for path in parts {
                    std::fs::remove_file(path)?;
                }
                written_rows.fetch_add(*row_count as u64, Ordering::Relaxed);
                Ok(())
            })
    })?;
    Ok((inputs.len(), written_rows.load(Ordering::Relaxed)))
}

#[cfg(test)]
#[path = "support_tests.rs"]
mod tests;
