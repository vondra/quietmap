//! Observed aircraft data processing on the canonical square grid.

mod accum;
mod spill;
use accum::*;
use spill::*;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use noise_compute::emission::aircraft::{NpdLuts, FT_PER_M};
use rayon::prelude::*;

use crate::arrow_io::{read_cruise_spill, write_cruise, write_cruise_spill, CruiseSpillRow};
use crate::arrow_schemas::CRUISE_TOP_K;
use crate::flight::{fl_bin_of, CruiseBucket, CruiseTopCandidate, FlightSegment, Phase};
use crate::geo::square_path;
use crate::profile::noise_class_of;
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;
use crate::spatial::{cruise_parent, cruise_transits};

/// log10 of 25 m expressed in ft — popup's `lookup_lmax` indexes into a
/// fixed log-d LUT. 25 m = 82.021 ft, log10(82.021) ≈ 1.9139. The popup
/// ranks cruise candidates by NPD Lmax at the 25 m reference, not SEL.
fn log_d_25m_ft() -> f64 {
    (25.0 * FT_PER_M).log10()
}

/// Estimated bytes per `CruiseTopEntry` in the worker accumulator.
/// 32 B base struct (4 B Lmax + 4 B alt + 4 B typecode + 16 B String
/// header + padding) + ~8 B avg callsign content = ~40 B.
const TOP_ENTRY_BYTES: usize = 40;

/// Flush a worker's thread-local accumulator to spill once its
/// estimated size crosses this. Chosen so a 90-128 GB / 24-32 core box
/// keeps `cores × SPILL_TRIGGER_BYTES` below ~20 GB peak in the fold
/// phase. Smaller would multiply file count without RAM benefit.
const SPILL_TRIGGER_BYTES: usize = 512 * 1024 * 1024;

const SIZE_CHECK_INTERVAL: usize = 100_000;

/// Number of coarse hash buckets the spill is partitioned across. Each
/// merge worker pulls one bucket fully into RAM, so the per-bucket size
/// is the merge-phase peak. 1024 keeps a ~1.6 TB total spill at ~1.6 GB
/// per bucket — 32 workers × 1.6 GB ≈ 51 GB, within budget.
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

pub fn run_stage_2b(
    day_paths: &[PathBuf],
    prepared_year_dir: &Path,
    n_days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga_cruise: bool,
) -> Result<usize> {
    let spill_dir = prepared_year_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("prepared_year_dir has no parent for spill_cruise sibling"))?
        .join("spill_cruise");
    // Wipe any stale spill from a crashed previous run; pre-create the
    // hash subdirs so workers can write without `create_dir_all` per file.
    if spill_dir.exists() {
        std::fs::remove_dir_all(&spill_dir)?;
    }
    for b in 0..SPILL_HASH_BUCKETS {
        std::fs::create_dir_all(spill_bucket_dir(&spill_dir, b))?;
    }

    started(
        "stage2b/spill",
        &format!("{} day shards (cruise)", day_paths.len()),
    );
    let stage_start = std::time::Instant::now();
    let part_id = AtomicU64::new(0);

    let spill_seg_counter = Milestone::new("stage2b/spill", "cruise segments", 1_000_000);
    // GA-class flights shouldn't physically reach the 8 000 m cruise
    // gate (`classify.rs` CRUISE_ENTER_AGL_M); any hit is a hybrid
    // class-filter leak or a fallback-table cross on bad data (the
    // PROP_C172 class absorbs PA**/P28*/P32*/P46* fallbacks). Counted
    // unconditionally — processing is unchanged so plain extracts stay
    // byte-identical — warned after the spill phase, fatal behind
    // `fail_on_ga_cruise`.
    let ga_class_cruise = AtomicU64::new(0);
    // Phase 1: par_iter days. Each worker holds a thread-local
    // accumulator for the duration of its day(s); the periodic size
    // check + final flush keep RAM bounded. NpdLuts is a `&'static`
    // singleton — shared across workers without cloning.
    let npd_luts = NpdLuts::shared();
    day_paths
        .par_iter()
        .try_for_each(|day_path| -> Result<()> {
            let mut local: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
            let mut segs_since_check = 0usize;
            let mut cruise_kept = 0u64;
            // Stream the day shard so the whole-day `Vec<FlightSegment>` never
            // resides in RAM (12 days × full-day load peaked at 126 GB and
            // global-OOM'd the box). `for_each_segment_batch` slices the decode
            // to ~20 MB; a LEGACY single-batch shard still pins its ~8 GB arrow
            // batch per worker, so MAX_THREADS caps how many load at once.
            // (Chunk-written shards are small per batch.) The accumulator spills.
            crate::arrow_io::for_each_segment_batch(day_path, |segments| {
                for seg in &segments {
                    if seg.phase != Phase::Cruise || seg.veh_kind != 0 {
                        continue;
                    }
                    if crate::profile::is_ga_sampled_profile(seg.profile_idx) {
                        ga_class_cruise.fetch_add(1, Ordering::Relaxed);
                    }
                    process_segment(seg, &mut local, scope, npd_luts);
                    cruise_kept += 1;
                    segs_since_check += 1;
                    if segs_since_check >= SIZE_CHECK_INTERVAL {
                        segs_since_check = 0;
                        if estimate_worker_bytes(&local) > SPILL_TRIGGER_BYTES {
                            flush_to_spill(&mut local, &spill_dir, &part_id)?;
                        }
                    }
                }
                Ok(())
            })
            .with_context(|| format!("stage2b spill day {}", day_path.display()))?;
            if !local.is_empty() {
                flush_to_spill(&mut local, &spill_dir, &part_id)?;
            }
            spill_seg_counter.add(cruise_kept);
            Ok(())
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
        // Bailing here (before the fold) saves the merge cost; the
        // in-scope cruise.arrow wipe already ran, which is acceptable —
        // a firing guard means the input pool is wrong and must be
        // re-extracted anyway.
        if fail_on_ga_cruise {
            anyhow::bail!(
                "--fail-on-ga-cruise: {ga_cruise} GA-class cruise segment(s) in Stage 2B \
                 input (expected 0 in a hybrid airline pass)"
            );
        }
    }

    // Wipe stale cruise.arrow from in-scope z9s before workers write
    // fresh files. z9s that have no cruise activity this run would
    // otherwise retain a prior-run file (possibly older schema) and
    // the popup reader would fatal-fail on schema_version mismatch.
    let wiped = crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, "cruise.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2b] wiped {wiped} stale cruise.arrow file(s) before write",
            ts()
        );
    }
    // Phase 2: per coarse hash bucket, merge every spilled part into
    // an z9-keyed accumulator, finalise, write `cruise.arrow`. The
    // per-bucket map IS the merge-phase RAM ceiling.
    started(
        "stage2b/fold",
        &format!("{SPILL_HASH_BUCKETS} hash buckets"),
    );
    let merge_start = std::time::Instant::now();
    let n_square = AtomicUsize::new(0);
    let fold_bucket_counter = Milestone::new("stage2b/fold", "buckets", 10);
    let fold_row_counter = Milestone::new("stage2b/fold", "cruise rows", 100_000);
    (0..SPILL_HASH_BUCKETS)
        .into_par_iter()
        .try_for_each(|bucket| -> Result<()> {
            let parts = list_spill_parts(&spill_bucket_dir(&spill_dir, bucket))?;
            if parts.is_empty() {
                fold_bucket_counter.add(1);
                return Ok(());
            }
            let mut by_square: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
            for path in &parts {
                for row in read_cruise_spill(path)? {
                    // Scope filter ran in process_segment; spill never
                    // received out-of-scope rows.
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
                }
            }
            let mut local_count = 0usize;
            let mut local_rows = 0u64;
            for (square, buckets_map) in by_square {
                if buckets_map.is_empty() {
                    continue;
                }
                let mut buckets: Vec<CruiseBucket> = buckets_map
                    .into_iter()
                    .map(|(k, a)| a.finalize(k))
                    .collect();
                buckets.sort_unstable_by_key(|bucket| {
                    (
                        bucket.cruise_cell_id,
                        bucket.class,
                        bucket.fl_bin,
                        bucket.period,
                    )
                });
                local_rows += buckets.len() as u64;
                let dir = prepared_year_dir.join(square_path(square));
                std::fs::create_dir_all(&dir)?;
                write_cruise(&dir.join("cruise.arrow"), &buckets, n_days)?;
                local_count += 1;
            }
            n_square.fetch_add(local_count, Ordering::Relaxed);
            fold_bucket_counter.add(1);
            fold_row_counter.add(local_rows);
            Ok(())
        })?;
    let t_merge = merge_start.elapsed();
    // Best-effort cleanup — leaving the dir after a crash is fine, the
    // next run wipes it before writing.
    let _ = std::fs::remove_dir_all(&spill_dir);

    let n = n_square.load(Ordering::Relaxed);
    finished(
        "stage2b/fold",
        &format!(
            "{n} z9s, {} cruise rows (merge {t_merge:?}, total {:?})",
            human(fold_row_counter.total()),
            stage_start.elapsed()
        ),
    );
    Ok(n)
}

fn process_segment(
    seg: &FlightSegment,
    by_square: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    scope: Option<&ScopeBbox>,
    npd_luts: &NpdLuts,
) {
    let class = noise_class_of(seg.profile_idx);
    let avg_alt_m = (seg.start_alt_m + seg.end_alt_m) * 0.5;
    let fl_bin = fl_bin_of(avg_alt_m);
    let period = seg.period;

    for (cell, clip_m) in cruise_transits(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon) {
        let square = cruise_parent(cell);
        if let Some(s) = scope {
            if !s.contains_square(square) {
                continue;
            }
        }
        let key = CruiseKey {
            cruise_cell_id: cell,
            class,
            fl_bin,
            period,
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
}

#[cfg(test)]
mod tests;
