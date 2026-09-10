//! Read-only world finish admission from completed raw spill, before any reconstruction is retired.

use super::*;
use crate::arrow_io::{cruise_buffers_bound, cruise_file_overhead_bound};
use rusqlite::{params, Connection, OpenFlags};
use std::mem::size_of;
use std::os::unix::ffi::OsStrExt;

#[derive(Clone, Copy, Default)]
struct Copies {
    rows: u64,
    candidates: u64,
    callsigns: u64,
    parts: u64,
}

#[derive(Default)]
struct ReplacementPeak {
    old: u64,
    positive_growth: u64,
    largest_overlap: u64,
}

impl ReplacementPeak {
    fn add(&mut self, old: u64, new: u64) {
        self.old += old;
        self.positive_growth += new.saturating_sub(old);
        self.largest_overlap = self.largest_overlap.max(old.min(new));
    }
    fn bytes(&self) -> u64 {
        // Any completed subset contributes at most all positive growth. The
        // in-flight replacement still retains its old files until publication.
        self.old + self.positive_growth + self.largest_overlap
    }
}

fn part_bound(charge: u64, largest_row: u64) -> Result<u64> {
    anyhow::ensure!(
        largest_row <= SPILL_TRIGGER_BYTES as u64,
        "oversized support row"
    );
    if charge == 0 {
        return Ok(0);
    }
    // Every non-final flush contains > trigger-largest_row bytes, irrespective
    // of randomized hash iteration. Each destination writes at most once/flush.
    Ok(1 + charge / (SPILL_TRIGGER_BYTES as u64 - largest_row + 1))
}

fn path_allocation_bound(count: u64, path_bytes: u64) -> u64 {
    // Vec geometric growth and joined PathBuf growth, including singleton
    // capacities and allocator headers; actual gather rechecks exact capacities.
    count * (2 * size_of::<PathBuf>() as u64 + 4 * (path_bytes + 32))
}

fn retained_input_allocation(paths: &Vec<PathBuf>, identities: &Vec<(String, String)>) -> u64 {
    let paths = paths.capacity() * size_of::<PathBuf>()
        + paths.iter().map(PathBuf::capacity).sum::<usize>();
    let identities = identities.capacity() * size_of::<(String, String)>()
        + identities
            .iter()
            .map(|(path, stat)| path.capacity() + stat.capacity())
            .sum::<usize>();
    // The final verification temporarily retains a fresh identity vector and
    // the saved SQLite identities beside the original comparison inputs.
    (paths + 3 * identities) as u64
}

/// The expected producer digest must come from the admitted launch receipt.
/// This planner never makes its own executable acceptable to the finish command.
pub fn plan_cruise_finish(
    directory: &Path,
    producer: &Path,
    expected_producer_sha256: &str,
    output: &Path,
) -> Result<()> {
    let directory = directory.canonicalize()?;
    let producer_digest = receipt::hash_file(producer)?;
    let hex: String = producer_digest.iter().map(|b| format!("{b:02x}")).collect();
    anyhow::ensure!(
        hex == expected_producer_sha256,
        "producer executable differs from admitted digest"
    );
    let source = Connection::open_with_flags(
        directory.join("state.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (days, scope): (u16, String) =
        source.query_row("SELECT n_days,scope FROM state", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    anyhow::ensure!(scope.is_empty(), "finish planner requires a world spill");
    let paths: Vec<PathBuf> = source
        .prepare("SELECT path FROM inputs ORDER BY path")?
        .query_map([], |r| r.get::<_, String>(0).map(PathBuf::from))?
        .collect::<rusqlite::Result<_>>()?;
    let identities = receipt::input_identities(&paths)?;
    anyhow::ensure!(
        identities.len() == usize::from(days),
        "primary window count differs"
    );
    let input_allocation = retained_input_allocation(&paths, &identities);
    crate::memory::max_concurrent_tasks(1, input_allocation + 16 * 1024 * 1024)?;
    receipt::verify_producer(&directory, &identities, days, None, true, &producer_digest)?;
    let raw_receipt_digest = receipt::hash_file(&directory.join("state.sqlite"))?;
    let (raw_parts, longest_relative): (u64, u64) = source.query_row(
        "SELECT COUNT(*),COALESCE(MAX(LENGTH(CAST(path AS BLOB))),0) FROM raw_parts",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let cells = grid::MAX_SQUARE_ID as usize + 1;
    let fixed = (2 * cells * size_of::<Copies>()) as u64 + 16 * 1024 * 1024 + input_allocation;
    let paths_bound = path_allocation_bound(
        raw_parts,
        directory.as_os_str().len() as u64 + 1 + longest_relative,
    );
    crate::memory::max_concurrent_tasks(1, paths_bound + fixed)?;
    let inputs = allocation::fold_inputs(&directory)?;
    let largest_fold = inputs.iter().map(|i| i.allocation_bytes).max().unwrap_or(0)
        + allocation::retained_paths_allocation(&inputs);
    let planner_memory = largest_fold + fixed;
    crate::memory::max_concurrent_tasks(1, planner_memory)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output_parent = parent.canonicalize()?;
    anyhow::ensure!(
        !output_parent.starts_with(&directory),
        "plan must be outside raw spill"
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut db = Connection::open(output)?;
    db.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-8192;
        CREATE TABLE state(phase TEXT, producer_sha256 BLOB, planner_sha256 BLOB, raw_receipt_sha256 BLOB, raw_directory TEXT, n_days INTEGER);
        CREATE TABLE bounds(name TEXT PRIMARY KEY, bytes INTEGER NOT NULL);
        CREATE TABLE counts(name TEXT PRIMARY KEY, count INTEGER NOT NULL);
        CREATE TABLE destinations(square INTEGER PRIMARY KEY, rows INTEGER, candidates INTEGER, callsign_bytes INTEGER, support_parts_bound INTEGER, support_bytes_bound INTEGER, final_bytes_bound INTEGER, gather_allocation_bound INTEGER);")?;
    // Bound diagnostic DB growth to one KiB per possible destination plus
    // sixteen metadata records. Refusal retains the entire original spill.
    db.pragma_update(
        None,
        "max_page_count",
        ((cells as u64 + 16) * 1024).div_ceil(4096),
    )?;
    db.execute(
        "INSERT INTO state VALUES ('planning',?1,?2,?3,?4,?5)",
        params![
            producer_digest,
            receipt::hash_file(&std::env::current_exe()?)?,
            raw_receipt_digest,
            directory.to_str(),
            days
        ],
    )?;
    eprintln!(
        "{} [stage2b/finish-plan] one worker; allocation {planner_memory} B; raw {raw_parts} parts",
        ts()
    );
    let mut totals = vec![Copies::default(); cells];
    let mut local = vec![Copies::default(); cells];
    let overhead = cruise_file_overhead_bound()?;
    let filesystem = filesystem_block_size(&directory)?;
    let mut fold_peak = ReplacementPeak::default();
    let mut canonical_rows = 0u64;
    let progress = Milestone::new("stage2b/finish-plan", "hash buckets", 10);
    for input in &inputs {
        local.fill(Copies::default());
        let mut charge = 0;
        let mut largest_row = 0;
        for (key, accumulator) in fold_raw_parts(&input.parts)?
            .into_values()
            .flat_map(HashMap::into_iter)
        {
            let bucket = accumulator.finalize(key);
            canonical_rows += 1;
            let row_charge = support_row_allocation(&bucket) as u64;
            largest_row = largest_row.max(row_charge);
            let (lon, lat) = grid::cruise::cruise_centroid(bucket.cruise_cell_id);
            for square in
                noise_compute::emission::aircraft::cruise_support_cells(lat, lon, bucket.rep_len_m)
                    .context("invalid finalized cruise support")?
                    .iter()
            {
                let counts = &mut local[grid::square_id(square) as usize];
                counts.rows += 1;
                counts.candidates += bucket.top_candidates.len() as u64;
                counts.callsigns += bucket
                    .top_candidates
                    .iter()
                    .map(|c| c.callsign.len() as u64)
                    .sum::<u64>();
                charge += row_charge;
            }
        }
        let flushes = part_bound(charge, largest_row)?;
        let mut support_blocks = 0;
        for (sum, counts) in totals.iter_mut().zip(&local) {
            if counts.rows == 0 {
                continue;
            }
            let parts = counts.rows.min(flushes);
            support_blocks +=
                cruise_buffers_bound(counts.rows, counts.candidates, counts.callsigns)
                    + parts * (overhead + filesystem - 1);
            sum.rows += counts.rows;
            sum.candidates += counts.candidates;
            sum.callsigns += counts.callsigns;
            sum.parts += parts;
        }
        fold_peak.add(input.allocated_bytes, support_blocks);
        progress.add(1);
    }
    let mut gather_peak = ReplacementPeak::default();
    let mut support_bytes = 0;
    let mut final_bytes = 0;
    let mut support_parts = 0;
    let mut largest_gather = 0;
    let mut retained_paths = 0;
    let mut destinations = 0;
    let transaction = db.transaction()?;
    for (square, counts) in totals.iter().enumerate().filter(|(_, c)| c.rows > 0) {
        let body = cruise_buffers_bound(counts.rows, counts.candidates, counts.callsigns);
        let bytes = body + counts.parts * overhead;
        let final_file = body
            + overhead
                * counts
                    .rows
                    .div_ceil(arrow_batching::TARGET_ROWS_PER_BATCH as u64);
        let gather = 5 * bytes
            + counts.rows * size_of::<support::IndexedCruiseRow>() as u64
            + counts.parts
                * (size_of::<arrow::record_batch::RecordBatch>() + size_of::<usize>()) as u64
            + (arrow_batching::TARGET_ROWS_PER_BATCH * size_of::<(usize, usize)>()) as u64;
        let path = directory
            .join("support")
            .join(square_path(square as u64))
            .join("part_ffffffffffffffff.arrow");
        retained_paths += path_allocation_bound(counts.parts, path.as_os_str().len() as u64);
        support_parts += counts.parts;
        largest_gather = largest_gather.max(gather);
        support_bytes += bytes;
        final_bytes += final_file;
        gather_peak.add(
            bytes + counts.parts * (filesystem - 1),
            final_file + filesystem - 1,
        );
        destinations += 1;
        transaction.execute(
            "INSERT INTO destinations VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                square,
                counts.rows,
                counts.candidates,
                counts.callsigns,
                counts.parts,
                bytes,
                final_file,
                gather
            ],
        )?;
    }
    // The caller retains its raw input path lists throughout support gather.
    let gather_memory = largest_gather
        + retained_paths
        + allocation::retained_paths_allocation(&inputs)
        + destinations * size_of::<(u64, Vec<PathBuf>, usize, usize)>() as u64;
    let data_peak = fold_peak.bytes().max(gather_peak.bytes());
    for (name, bytes) in [
        ("planner_allocation", planner_memory),
        ("planner_retained_input_allocation", input_allocation),
        ("fold_allocation", largest_fold),
        ("gather_allocation", gather_memory),
        ("support_file_bytes", support_bytes),
        ("final_file_bytes", final_bytes),
        (
            "final_data_blocks",
            final_bytes + destinations * (filesystem - 1),
        ),
        ("raw_allocated_bytes", fold_peak.old),
        ("single_worker_data_block_peak", data_peak),
        (
            "additional_data_blocks_above_raw",
            data_peak.saturating_sub(fold_peak.old),
        ),
        ("filesystem_block_size", filesystem),
        ("file_overhead_bound", overhead),
    ] {
        transaction.execute("INSERT INTO bounds VALUES (?1,?2)", params![name, bytes])?;
    }
    for (name, count) in [
        ("support_parts_bound", support_parts),
        ("canonical_rows", canonical_rows),
        ("support_rows", totals.iter().map(|c| c.rows).sum()),
        ("workers", 1),
        ("destinations", destinations),
    ] {
        transaction.execute("INSERT INTO counts VALUES (?1,?2)", params![name, count])?;
    }
    transaction.commit()?;
    receipt::verify_producer(
        &directory,
        &receipt::input_identities(&paths)?,
        days,
        None,
        true,
        &producer_digest,
    )?;
    anyhow::ensure!(
        receipt::hash_file(producer)? == producer_digest
            && receipt::hash_file(&directory.join("state.sqlite"))? == raw_receipt_digest,
        "producer or raw receipt changed during planning"
    );
    db.execute("UPDATE state SET phase='complete'", [])?;
    drop(db);
    std::fs::File::open(output)?.sync_all()?;
    std::fs::File::open(parent)?.sync_all()?;
    eprintln!("{} [stage2b/finish-plan] complete; single-worker fold {largest_fold} B, gather {gather_memory} B; data-block peak {data_peak} B. Filesystem metadata and independent final copy require separate reservation.", ts());
    Ok(())
}

fn filesystem_block_size(path: &Path) -> Result<u64> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    anyhow::ensure!(
        unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0,
        "statvfs failed"
    );
    let size = unsafe { stat.assume_init() }.f_frsize;
    anyhow::ensure!(size > 0, "zero filesystem allocation unit");
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_peak_covers_either_retirement_order() {
        let mut bound = ReplacementPeak::default();
        bound.add(100, 10);
        bound.add(20, 150);
        assert_eq!(bound.bytes(), 270);
        // Grow the second first: 120+150; then retire its20 and add10.
        assert!(bound.bytes() >= 120 + 150);
        assert!(bound.bytes() >= 100 + 150 + 10);
    }
    #[test]
    fn flush_bound_handles_full_row_and_empty_hash() {
        assert_eq!(part_bound(0, 0).unwrap(), 0);
        assert_eq!(
            part_bound(SPILL_TRIGGER_BYTES as u64 * 2, SPILL_TRIGGER_BYTES as u64).unwrap(),
            SPILL_TRIGGER_BYTES as u64 * 2 + 1
        );
        assert!(part_bound(1, SPILL_TRIGGER_BYTES as u64 + 1).is_err());
    }
}
