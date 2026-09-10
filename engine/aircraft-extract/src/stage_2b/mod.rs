//! Observed aircraft data processing on the canonical square grid.

mod accum;
mod allocation;
mod census;
mod finish_plan;
pub use finish_plan::plan_cruise_finish;
mod receipt;
pub use census::census_cruise_inputs;
pub(crate) use receipt::receipt_page_limit as spill_receipt_page_limit;
mod spill;
mod support;
use accum::*;
use spill::*;
use support::*;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use noise_compute::emission::aircraft::{NpdLuts, FT_PER_M};
use rayon::prelude::*;

use crate::arrow_io::{for_each_cruise_spill, write_cruise, write_cruise_spill, CruiseSpillRow};
use crate::arrow_schemas::CRUISE_TOP_K;
use crate::flight::{fl_bin_of, CruiseBucket, CruiseTopCandidate, FlightSegment, Phase};
use crate::geo::square_path;
use crate::profile::noise_class_of;
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;
use crate::spatial::cruise_transits;
use grid::cruise::cruise_parent;

/// log10 of 25 m expressed in ft — popup's `lookup_lmax` indexes into a
/// fixed log-d LUT. 25 m = 82.021 ft, log10(82.021) ≈ 1.9139. The popup
/// ranks cruise candidates by NPD Lmax at the 25 m reference, not SEL.
fn log_d_25m_ft() -> f64 {
    (25.0 * FT_PER_M).log10()
}

/// Accumulation target; allocation admission includes coexisting serialization buffers.
const SPILL_TRIGGER_BYTES: usize = 512 * 1024 * 1024;

/// Partition routing; actual bucket counts determine concurrency.
const SPILL_HASH_BUCKETS: u64 = 1024;

fn spill_bucket(square: u64) -> u64 {
    let mut x = square;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    let mixed = x ^ (x >> 31);
    mixed % SPILL_HASH_BUCKETS
}

fn spill_bucket_dir(spill_dir: &Path, bucket: u64) -> std::path::PathBuf {
    spill_dir.join(format!("hash_{bucket:04x}"))
}

fn spill_part_path(spill_dir: &Path, bucket: u64, id: u64) -> std::path::PathBuf {
    spill_bucket_dir(spill_dir, bucket).join(format!("part_{id:016x}.arrow"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CruisePhase {
    All,
    Spill,
    Finish,
}

pub fn run_stage_2b(
    day_paths: &[PathBuf],
    prepared_year_dir: &Path,
    n_days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga_cruise: bool,
) -> Result<usize> {
    run_stage_2b_phase(
        day_paths,
        prepared_year_dir,
        n_days,
        scope,
        fail_on_ga_cruise,
        CruisePhase::All,
    )
}

pub fn run_stage_2b_phase(
    day_paths: &[PathBuf],
    prepared_year_dir: &Path,
    n_days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga_cruise: bool,
    phase: CruisePhase,
) -> Result<usize> {
    let spill_dir = prepared_year_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("prepared_year_dir has no parent for spill_cruise sibling"))?
        .join("spill_cruise");
    let stage_start = std::time::Instant::now();
    let part_id = AtomicU64::new(0);
    let identities = receipt::input_identities(day_paths)?;
    if phase == CruisePhase::Finish {
        receipt::verify(&spill_dir, &identities, n_days, scope, fail_on_ga_cruise)?;
    } else {
        // A fresh producer owns this directory exclusively; ambiguous partial work
        // requires a new output tree instead of silently destroying retained spill.
        crate::arrow_io::create_directory_all_synced(
            spill_dir.parent().context("missing spill parent")?,
        )?;
        std::fs::create_dir(&spill_dir)
            .context("existing cruise spill requires finish or a new output directory")?;
        std::fs::File::open(spill_dir.parent().context("missing spill parent")?)?.sync_all()?;
        for b in 0..SPILL_HASH_BUCKETS {
            std::fs::create_dir_all(spill_bucket_dir(&spill_dir, b))?;
        }

        started(
            "stage2b/spill",
            &format!("{} day shards (cruise)", day_paths.len()),
        );

        let spill_seg_counter = Milestone::new("stage2b/spill", "cruise segments", 1_000_000);
        // GA-class flights shouldn't physically reach the 8 000 m cruise
        // gate (`classify.rs` CRUISE_ENTER_AGL_M); any hit is a hybrid
        // class-filter leak or a fallback-table cross on bad data (the
        // PROP_C172 class absorbs PA**/P28*/P32*/P46* fallbacks). Counted
        // unconditionally — processing is unchanged so plain extracts stay
        // byte-identical — warned after the spill phase, fatal behind
        // `fail_on_ga_cruise`.
        let ga_class_cruise = AtomicU64::new(0);
        let npd_luts = NpdLuts::shared();
        let largest_batch = day_paths
            .iter()
            .map(|path| {
                crate::arrow_io::inspect_ipc_allocation(path).map(|facts| facts.largest_batch_bytes)
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        let spill_allocation = if day_paths.is_empty() {
            0
        } else {
            4 * SPILL_TRIGGER_BYTES as u64
                + 2 * largest_batch
                + (crate::arrow_io::SEGMENT_READ_CHUNK_ROWS
                    * (std::mem::size_of::<FlightSegment>() + 32)) as u64
        };
        let workers = crate::memory::max_concurrent_tasks(
            day_paths.len().min(rayon::current_num_threads()),
            spill_allocation,
        )?;
        let spill_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()?;
        eprintln!(
            "{} [stage2b/spill] {workers} workers; per-worker allocation {spill_allocation} B",
            ts()
        );
        spill_pool.install(|| {
            day_paths.par_iter().try_for_each(|day_path| -> Result<()> {
                let mut local: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
                let mut charged_bytes = 0usize;
                let mut cruise_kept = 0u64;
                crate::arrow_io::for_each_segment_batch(day_path, |segments| {
                    for seg in &segments {
                        if seg.phase != Phase::Cruise || seg.veh_kind != 0 {
                            continue;
                        }
                        if crate::profile::is_ga_sampled_profile(seg.profile_idx) {
                            ga_class_cruise.fetch_add(1, Ordering::Relaxed);
                        }
                        let addition = allocation::transit_allocation(seg.callsign.len());
                        anyhow::ensure!(
                            addition <= SPILL_TRIGGER_BYTES,
                            "one cruise transit exceeds the accumulator allocation"
                        );
                        for (cell, clip_m) in
                            cruise_transits(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon)
                        {
                            if charged_bytes + addition > SPILL_TRIGGER_BYTES {
                                flush_to_spill(&mut local, &spill_dir, &part_id)?;
                                charged_bytes = 0;
                            }
                            add_transit(seg, cell, clip_m, &mut local, npd_luts);
                            charged_bytes += addition;
                        }
                        cruise_kept += 1;
                    }
                    Ok(())
                })
                .with_context(|| format!("stage2b spill day {}", day_path.display()))?;
                if !local.is_empty() {
                    flush_to_spill(&mut local, &spill_dir, &part_id)?;
                }
                spill_seg_counter.add(cruise_kept);
                Ok(())
            })
        })?;
        let t_phase1 = stage_start.elapsed();
        let parts_written = part_id.load(Ordering::Relaxed);
        finished(
            "stage2b/spill",
            &format!(
                "{} cruise segments → {parts_written} spill parts in {t_phase1:?}",
                human(spill_seg_counter.total())
            ),
        );
        let ga_cruise = ga_class_cruise.load(Ordering::Relaxed);
        if ga_cruise > 0 {
            eprintln!(
                "{} [stage2b] WARNING: {ga_cruise} GA-class (PROP_C172/HELICOPTER) cruise \
             segment(s) in the Stage 2B input — hybrid class-filter leak or data error \
             (ga-365d-hybrid-plan.md delta 4)",
                ts()
            );
            // Reject a bad input pool before replacing any prepared destination.
            if fail_on_ga_cruise {
                anyhow::bail!(
                    "--fail-on-ga-cruise: {ga_cruise} GA-class cruise segment(s) in Stage 2B \
                 input (expected 0 in a hybrid airline pass)"
                );
            }
        }

        anyhow::ensure!(
            receipt::input_identities(day_paths)? == identities,
            "primary inputs changed during cruise spill"
        );
        receipt::create(&spill_dir, &identities, n_days, scope, ga_cruise)?;
    }
    // Each canonical key belongs to one fold worker. Finalize once, then
    // spill intact copies by destination; only gather writes prepared cells.
    started(
        "stage2b/fold",
        &format!("{SPILL_HASH_BUCKETS} hash buckets"),
    );
    let merge_start = std::time::Instant::now();
    let support_dir = spill_dir.join("support");
    let fold_bucket_counter = Milestone::new("stage2b/fold", "buckets", 10);
    let fold_row_counter = Milestone::new("stage2b/fold", "cruise rows", 100_000);
    let inputs = allocation::fold_inputs(&spill_dir)?;
    let largest = inputs
        .iter()
        .map(|input| input.allocation_bytes)
        .max()
        .unwrap_or(0)
        + allocation::retained_paths_allocation(&inputs);
    let spill_bytes: u64 = inputs.iter().map(|input| input.file_bytes).sum();
    receipt::record_fold_plan(
        &spill_dir,
        largest,
        spill_bytes,
        inputs.iter().map(|input| input.allocated_bytes).sum(),
    )?;
    if phase == CruisePhase::Spill {
        eprintln!("{} [stage2b/spill] retained {spill_bytes} B raw spill; largest prospective fold allocation {largest} B", ts());
        return Ok(0);
    }
    let workers = crate::memory::max_concurrent_tasks(rayon::current_num_threads(), largest)?;
    eprintln!("{} [stage2b/fold] {workers} workers; largest allocation {largest} B; raw spill {spill_bytes} B", ts());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    receipt::begin_fold(&spill_dir)?;
    pool.install(|| {
        inputs.par_iter().try_for_each(|input| -> Result<()> {
            let parts = &input.parts;
            if parts.is_empty() {
                fold_bucket_counter.add(1);
                return Ok(());
            }
            let by_square = fold_raw_parts(parts)?;
            let canonical_rows = scatter_finalized_cruise(
                by_square
                    .into_values()
                    .flat_map(|map| map.into_iter().map(|(key, accum)| accum.finalize(key))),
                &support_dir,
                &part_id,
                n_days,
                scope,
            )?;
            // Support parts are synced before retiring their reconstructible raw input.
            for path in parts {
                std::fs::remove_file(path)?;
            }
            fold_bucket_counter.add(1);
            fold_row_counter.add(canonical_rows);
            Ok(())
        })
    })?;
    let (n, copied_rows) = gather_finalized_cruise(&support_dir, prepared_year_dir, scope)?;
    let t_merge = merge_start.elapsed();
    // Completed output is durable; a failed cleanup leaves an explicitly non-resumable state.
    let _ = std::fs::remove_dir_all(&spill_dir);

    finished(
        "stage2b/fold",
        &format!(
            "{n} destination z9s, {} canonical cruise rows → {copied_rows} support copies (merge {t_merge:?}, total {:?})",
            human(fold_row_counter.total()),
            stage_start.elapsed()
        ),
    );
    Ok(n)
}

fn fold_raw_parts(parts: &[PathBuf]) -> Result<HashMap<u64, HashMap<CruiseKey, CruiseAccum>>> {
    let mut by_square: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    for path in parts {
        for_each_cruise_spill(path, |row| {
            let key = CruiseKey {
                cruise_cell_id: row.cruise_cell_id,
                class: row.class,
                fl_bin: row.fl_bin,
                period: row.period,
            };
            let square = row.square;
            let incoming = accum_from_spill(row);
            match by_square.entry(square).or_default().entry(key) {
                Entry::Vacant(v) => {
                    v.insert(incoming);
                }
                Entry::Occupied(mut o) => o.get_mut().merge(incoming),
            }
            Ok(())
        })?;
    }
    Ok(by_square)
}

fn add_transit(
    seg: &FlightSegment,
    cell: u64,
    clip_m: f32,
    by_square: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    npd_luts: &NpdLuts,
) {
    let square = cruise_parent(cell);
    let key = CruiseKey {
        cruise_cell_id: cell,
        class: noise_class_of(seg.profile_idx),
        fl_bin: fl_bin_of((seg.start_alt_m + seg.end_alt_m) * 0.5),
        period: seg.period,
    };
    by_square
        .entry(square)
        .or_default()
        .entry(key)
        .or_insert_with(|| CruiseAccum {
            source_id: seg.source_id,
            origin: seg.origin,
            rep_profile_idx: seg.profile_idx,
            ..Default::default()
        })
        .add(seg, clip_m, npd_luts);
}

#[cfg(test)]
fn process_segment(
    seg: &FlightSegment,
    by_square: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    npd_luts: &NpdLuts,
) {
    for (cell, clip_m) in cruise_transits(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon) {
        add_transit(seg, cell, clip_m, by_square, npd_luts);
    }
}

#[cfg(test)]
mod tests;
