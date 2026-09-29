//! Stage 2A — midpoint-owned airborne shards → per-z9 `airborne.arrow`.
//!
//! One row per sub-segment (each carrying its own period / date / flags so a
//! long crossing that straddles 19:00 still gets the correct Lden weighting),
//! stored once in the square that owns its midpoint; flight identity is one
//! dictionary per file. Consumes the per-z9 shards produced by
//! [`crate::shuffle::shuffle_per_square`] — one input file per z9 means each
//! worker owns its z9's rows, no global merge.
//!
//! Each row retains the start/end terrain elevations sampled in Stage 1.
//! Intermediate chord terrain is not stored; see SPEC §6.1's filtering contract.

use std::path::Path;

use anyhow::{Context, Result};

use crate::arrow_io::{for_each_segment_batch, require_owner_shard};
use crate::flight::{FlightSegment, Phase};
use crate::geo::square_path;
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;
use crate::shuffle::list_square_shards;

/// Run Stage 2A against the shuffled per-z9 airborne shards under
/// `segments_by_square_dir/<z9>/airborne.arrow`. When `scope` is set, z9
/// subdirs outside it are skipped — scope was applied during shuffle
/// already, so this is defensive and cheap.
pub fn run_stage_2a(
    segments_by_square_dir: &Path,
    prepared_year_dir: &Path,
    // Stamped into every airborne.arrow so popup and heatmap normalise
    // primary rows by the baseline days and secondary-only rows by the
    // increment days.
    window: &noise_compute::emission::aircraft::SamplingWindow,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    let square_inputs = list_square_shards(segments_by_square_dir, "airborne.arrow", scope)?;
    let allocation_allowances = square_inputs
        .iter()
        .map(|(_, path)| shard_allocation_allowance(path))
        .collect::<Result<Vec<u64>>>()?;
    let largest_allocation = allocation_allowances.iter().copied().max().unwrap_or(0);
    let concurrent_budget = crate::memory::concurrent_allocation_budget_for(largest_allocation)?;
    let threads = rayon::current_num_threads();
    // Validate every input before replacing any prior output.
    let wiped =
        crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, "airborne.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2a] wiped {wiped} stale airborne.arrow file(s) before write",
            ts()
        );
    }
    let n_square = square_inputs.len();
    started("stage2a", &format!("{n_square} z9 cells; {threads} threads share {concurrent_budget} B of allocation allowances; largest cell {largest_allocation} B"));
    let stage_start = std::time::Instant::now();

    let square_counter = Milestone::new("stage2a", "z9 cells", 100);
    let seg_counter = Milestone::new("stage2a", "segments in", 1_000_000);
    let row_counter = Milestone::new("stage2a", "rows out", 1_000_000);
    let written = std::sync::atomic::AtomicUsize::new(0);
    // Each cell writes only its own file, so the start order cannot change any output byte.
    crate::largest_first_memory_admission::run_largest_first_within_memory_budget(
        &allocation_allowances,
        threads,
        concurrent_budget,
        |task| {
            let (square, shard_path) = &square_inputs[task];
            let mut rows = Vec::new();
            for_each_segment_batch(shard_path, |segments| {
                seg_counter.add(segments.len() as u64);
                rows.extend(segments.into_iter().filter(is_aircraft_airborne));
                Ok(())
            })
            .with_context(|| format!("read {}", shard_path.display()))?;
            square_counter.add(1);
            if rows.is_empty() {
                return Ok(());
            }
            // Stable by flight: the dictionary lists flights in ascending id
            // order and a flight's rows keep their shard order, so the bytes
            // are reproducible.
            rows.sort_by_key(|row| row.flight_id);
            row_counter.add(rows.len() as u64);
            let dir = prepared_year_dir.join(square_path(*square));
            std::fs::create_dir_all(&dir)?;
            crate::arrow_io::write_airborne(&dir.join("airborne.arrow"), &rows, window)?;
            written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        },
    )?;
    let written = written.load(std::sync::atomic::Ordering::Relaxed);
    let empty = n_square.saturating_sub(written);
    finished(
        "stage2a",
        &format!(
            "{written}/{n_square} z9s wrote {} rows from {} segments ({} empty after gates) in {:?}",
            human(row_counter.total()),
            human(seg_counter.total()),
            empty,
            stage_start.elapsed()
        ),
    );
    Ok(written)
}

/// Ground rows and ground-support equipment take other paths (Stage 2C).
fn is_aircraft_airborne(segment: &FlightSegment) -> bool {
    segment.phase == Phase::Airborne && segment.veh_kind == 0
}

/// Raw IPC bytes cover the decoded rows and their Arrow columns during
/// spatial sorting; flight runs bound the dictionary and hash overhead. The
/// source shard itself is released one decoded batch at a time. Also the
/// input gate: a shard of the retired support-copy layout is refused here,
/// before any prior output is wiped.
fn shard_allocation_allowance(path: &Path) -> Result<u64> {
    let file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    let reader = arrow::ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)?;
    let schema = reader.schema();
    crate::arrow_schemas::assert_schema_version(schema.metadata())?;
    require_owner_shard(schema.metadata()).with_context(|| path.display().to_string())?;
    let count = |key: &str| -> Result<u64> {
        schema
            .metadata()
            .get(key)
            .with_context(|| {
                format!(
                    "{} lacks {key}; run shuffle with the current writer",
                    path.display()
                )
            })?
            .parse()
            .with_context(|| format!("invalid {key} in {}", path.display()))
    };
    Ok(3 * bytes
        + 768 * count("flight_runs")?
        + 2 * count("flight_run_callsign_bytes")?
        + (1 << 30))
}

#[cfg(test)]
#[path = "stage_2a_tests.rs"]
mod tests;
