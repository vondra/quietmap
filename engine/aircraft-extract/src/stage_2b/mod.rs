//! Stage 2B — Cruise segments → per-R4 `cruise.arrow`.
//!
//! For each Phase::Cruise segment we densify along the great-circle
//! line (`DENSIFY_STEP_M` = 250 m), find every R7 cell touched, compute
//! the analytical line-cell clip length (geo::line_cell_clip_length),
//! and accumulate per (R7, fl_bin, class, period, is_dep=true) bucket.
//!
//! Key choice — **all cruise rows are forced `is_dep = true`**: per Doc
//! 29 §A.3.2 en-route flights use Departure NPDs (no cruise NPD set is
//! published), so bucketing on the inherited flight-level flag would
//! mis-attribute cruise to Approach NPDs and produce ~5 dB stochastic
//! error.
//!
//! **Disk spill.** The per-`(R4, CruiseKey)` accumulator carries a
//! `flight_meta: HashMap<flight_id, (typecode, callsign)>` whose size
//! scales as `O(unique flight_ids × R7 cells touched)`. Back-of-
//! envelope at full-year global scale: ~40 M unique cruise flight_ids × ~500
//! R7 cells touched/flight × ~80 B/entry ≈ ~1.6 TB if held entirely
//! in memory. Workers flush their thread-local map to
//! `spill_cruise/hash_<R4 hash % N>/part_<atomic_id>.arrow` when it
//! crosses a threshold; a merge pass per hash bucket reconstructs the
//! per-R4 `CruiseBucket` set without ever materialising the global map.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use h3o::{CellIndex, LatLng, Resolution};
use noise_compute::emission::aircraft::{NpdLuts, FT_PER_M};
use rayon::prelude::*;

use crate::arrow_io::{read_cruise_spill, write_cruise, write_cruise_spill, CruiseSpillRow};
use crate::arrow_schemas::CRUISE_TOP_K;
use crate::flight::{fl_bin_of, CruiseBucket, CruiseTopCandidate, FlightSegment, Phase};
use crate::geo::{flat_dist, line_cell_clip_length, r4_hex_str, signed_lon_diff};
use crate::profile::noise_class_of;
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;

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

/// Step length for densifying a cruise segment along its great-circle
/// arc. R7 cells average ~1.22 km edge (~2.44 km diameter); a sub-cell
/// step keeps consecutive samples in the same or an adjacent cell so
/// the clipped-length sum stays within a few % of the segment length.
/// 250 m is conservative for R7 (~10 samples per cell) and would still
/// catch all touched cells at R8 (~4 samples per cell) if Stage 2B were
/// re-tuned to a finer hex.
pub const DENSIFY_STEP_M: f32 = 250.0;

/// Flush a worker's thread-local accumulator to spill once its
/// estimated size crosses this. Chosen so a 90-128 GB / 24-32 core box
/// keeps `cores × SPILL_TRIGGER_BYTES` below ~20 GB peak in the fold
/// phase. Smaller would multiply file count without RAM benefit.
const SPILL_TRIGGER_BYTES: usize = 512 * 1024 * 1024;

/// Check the worker accumulator size every N segments. Bigger reduces
/// the size-estimate overhead; smaller reduces peak overshoot before
/// the next spill. 100k segments × ~3 R7 cells each ≈ 300k bucket
/// inserts between checks — well below the 512 MB trigger at typical
/// densities, so a check this often catches the threshold cleanly.
const SIZE_CHECK_INTERVAL: usize = 100_000;

/// Number of coarse hash buckets the spill is partitioned across. Each
/// merge worker pulls one bucket fully into RAM, so the per-bucket size
/// is the merge-phase peak. 1024 keeps a ~1.6 TB total spill at ~1.6 GB
/// per bucket — 32 workers × 1.6 GB ≈ 51 GB, within budget.
const SPILL_HASH_BUCKETS: u64 = 1024;

/// SplitMix64-style avalanche over the H3 cell index. H3 indices pack
/// the mode (4 b), resolution (4 b), and base-cell (7 b) into high
/// bits, with digit-path bits in low positions — a naive `% N` would
/// correlate buckets with the lowest digits and skew load. The mixer
/// avalanches both halves so adjacent R4 cells land in independent
/// buckets. Constants are the published SplitMix64 mixers.
fn spill_bucket(r4: u64) -> u64 {
    let mut x = r4;
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

/// Run Stage 2B against `segments/<day>.arrow` per-day shards. Each
/// worker decodes one day, folds cruise segments into a thread-local
/// accumulator that spills to a hash-partitioned scratch dir when
/// oversized, then a merge pass per bucket writes one `cruise.arrow`
/// per R4 under `h3r4_dir`. Cruise is read from per-day shards (not
/// the shuffled per-R4 ones) because cruise output R4 derives from
/// each touched R7 cell's parent — midpoint-shuffling would lose
/// cross-R4 cell routing.
///
/// `fail_on_ga_cruise` upgrades the GA-class-segment warning (see the
/// counter below) to a hard failure — the cross-check for hybrid runs
/// where Stage 2B input must be the airline pass only.
pub fn run_stage_2b(
    day_paths: &[PathBuf],
    h3r4_dir: &Path,
    n_days: u16,
    scope: Option<&ScopeBbox>,
    fail_on_ga_cruise: bool,
) -> Result<usize> {
    // Wipe stale cruise.arrow from in-scope R4s before workers write
    // fresh files. R4s that have no cruise activity this run would
    // otherwise retain a prior-run file (possibly older schema) and
    // the popup reader would fatal-fail on schema_version mismatch.
    let wiped = crate::wipe::wipe_stale_arrows_for_scope(h3r4_dir, "cruise.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2b] wiped {wiped} stale cruise.arrow file(s) before write",
            ts()
        );
    }
    let spill_dir = h3r4_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("h3r4_dir has no parent for spill_cruise sibling"))?
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

    // Phase 2: per coarse hash bucket, merge every spilled part into
    // an R4-keyed accumulator, finalise, write `cruise.arrow`. The
    // per-bucket map IS the merge-phase RAM ceiling.
    started(
        "stage2b/fold",
        &format!("{SPILL_HASH_BUCKETS} hash buckets"),
    );
    let merge_start = std::time::Instant::now();
    let n_r4 = AtomicUsize::new(0);
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
            let mut by_r4: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
            for path in &parts {
                for row in read_cruise_spill(path)? {
                    // Scope filter ran in process_segment; spill never
                    // received out-of-scope rows.
                    let key = CruiseKey {
                        r7_hex: row.r7_hex,
                        class: row.class,
                        fl_bin: row.fl_bin,
                        period: row.period,
                    };
                    let r4 = row.r4;
                    let incoming = accum_from_spill(row);
                    match by_r4.entry(r4).or_default().entry(key) {
                        Entry::Vacant(v) => {
                            v.insert(incoming);
                        }
                        Entry::Occupied(mut o) => o.get_mut().merge(incoming),
                    }
                }
            }
            let mut local_count = 0usize;
            let mut local_rows = 0u64;
            for (r4, buckets_map) in by_r4 {
                if buckets_map.is_empty() {
                    continue;
                }
                let buckets: Vec<CruiseBucket> = buckets_map
                    .into_iter()
                    .map(|(k, a)| a.finalize(k))
                    .collect();
                local_rows += buckets.len() as u64;
                let dir = h3r4_dir.join(r4_hex_str(r4));
                std::fs::create_dir_all(&dir)?;
                write_cruise(&dir.join("cruise.arrow"), &buckets, n_days)?;
                local_count += 1;
            }
            n_r4.fetch_add(local_count, Ordering::Relaxed);
            fold_bucket_counter.add(1);
            fold_row_counter.add(local_rows);
            Ok(())
        })?;
    let t_merge = merge_start.elapsed();
    // Best-effort cleanup — leaving the dir after a crash is fine, the
    // next run wipes it before writing.
    let _ = std::fs::remove_dir_all(&spill_dir);

    let n = n_r4.load(Ordering::Relaxed);
    finished(
        "stage2b/fold",
        &format!(
            "{n} R4s, {} cruise rows (merge {t_merge:?}, total {:?})",
            human(fold_row_counter.total()),
            stage_start.elapsed()
        ),
    );
    Ok(n)
}

fn list_spill_parts(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in read {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("arrow") {
            out.push(path);
        }
    }
    Ok(out)
}

/// Rough byte estimate for a worker's thread-local accumulator —
/// per-bucket and per-fid overheads dominate; HashMap fill-factor
/// constant absorbed into the per-bucket multiplier. Checked at
/// SIZE_CHECK_INTERVAL so per-call cost amortises.
fn estimate_worker_bytes(by_r4: &HashMap<u64, HashMap<CruiseKey, CruiseAccum>>) -> usize {
    let mut n_buckets = 0usize;
    let mut n_fids = 0usize;
    let mut n_top_entries = 0usize;
    for inner in by_r4.values() {
        n_buckets += inner.len();
        for accum in inner.values() {
            n_fids += accum.fid_set.len();
            n_top_entries += accum.top.len();
        }
    }
    // 200 B per (R4, CruiseKey) bucket = CruiseAccum + 2× HashMap entries
    // (outer + inner). 24 B per fid (HashSet<u64> entry: 8 B u64 + 16 B
    // hash table overhead). TOP_ENTRY_BYTES per top entry (capped at K).
    n_buckets * 200 + n_fids * 24 + n_top_entries * TOP_ENTRY_BYTES
}

/// Consume the worker's accumulator into spill files. Takes `&mut` and
/// drains via `std::mem::take` so callsign Strings move (no per-fid
/// clone — ~10–100 fids per bucket × millions of buckets at global
/// scope makes the clone cost real).
fn flush_to_spill(
    local: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    spill_dir: &Path,
    part_id: &AtomicU64,
) -> Result<()> {
    let drained = std::mem::take(local);
    let mut by_bucket: HashMap<u64, Vec<CruiseSpillRow>> = HashMap::new();
    for (r4, by_key) in drained {
        let bucket = spill_bucket(r4);
        let dst = by_bucket.entry(bucket).or_default();
        for (key, accum) in by_key {
            dst.push(spill_row_consume(r4, key, accum));
        }
    }
    for (bucket, rows) in by_bucket {
        let id = part_id.fetch_add(1, Ordering::Relaxed);
        write_cruise_spill(&spill_part_path(spill_dir, bucket, id), &rows)?;
    }
    Ok(())
}

fn spill_row_consume(r4: u64, key: CruiseKey, accum: CruiseAccum) -> CruiseSpillRow {
    // fid_set: sort ascending for deterministic on-disk bytes.
    let mut fid_set: Vec<u64> = accum.fid_set.into_iter().collect();
    fid_set.sort_unstable();
    // top_candidates: keep insertion order — merge re-emits via the
    // re-entrant cap-K logic so the destination accumulator computes
    // the true top-K after consuming all spill parts.
    let top_candidates: Vec<CruiseTopCandidate> = accum.top.into_values().collect();
    CruiseSpillRow {
        r4,
        r7_hex: key.r7_hex,
        class: key.class,
        fl_bin: key.fl_bin,
        period: key.period,
        rep_profile_idx: accum.rep_profile_idx,
        source_id: accum.source_id,
        origin: accum.origin,
        sum_length_m: accum.sum_length_m,
        weight: accum.weight,
        rep_alt_m: accum.rep_alt_m,
        rep_speed_kt: accum.rep_speed_kt,
        rep_len_m: accum.rep_len_m,
        rep_len_w: accum.rep_len_w,
        fid_set,
        top_candidates,
    }
}

fn accum_from_spill(row: CruiseSpillRow) -> CruiseAccum {
    let mut fid_set: std::collections::HashSet<u64> =
        std::collections::HashSet::with_capacity(row.fid_set.len());
    for fid in row.fid_set {
        fid_set.insert(fid);
    }
    let mut top: HashMap<u64, CruiseTopCandidate> =
        HashMap::with_capacity(row.top_candidates.len());
    let mut top_min_lmax = f32::INFINITY;
    for cand in row.top_candidates {
        if cand.peak_lmax_25m_db < top_min_lmax {
            top_min_lmax = cand.peak_lmax_25m_db;
        }
        top.insert(cand.flight_id, cand);
    }
    if top.is_empty() {
        top_min_lmax = f32::NEG_INFINITY;
    }
    CruiseAccum {
        sum_length_m: row.sum_length_m,
        rep_len_m: row.rep_len_m,
        rep_len_w: row.rep_len_w,
        rep_alt_m: row.rep_alt_m,
        rep_speed_kt: row.rep_speed_kt,
        weight: row.weight,
        rep_profile_idx: row.rep_profile_idx,
        fid_set,
        top,
        top_min_lmax,
        source_id: row.source_id,
        origin: row.origin,
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
struct CruiseKey {
    r7_hex: u64,
    class: u8,
    fl_bin: u8,
    period: u8,
}

/// Per-bucket worker accumulator (v14). `fid_set` tracks the full
/// `unique_count`; `top` keeps a bounded top-K snapshot ranked by
/// source-side peak Lmax at 25 m. Tail fids beyond K=`CRUISE_TOP_K`
/// drop out of band counters at the popup but still contribute to
/// `unique_count`.
#[derive(Clone)]
struct CruiseAccum {
    sum_length_m: f32,
    rep_len_m: f32, // weighted mean of original segment lengths
    rep_len_w: f32, // weight accumulator
    rep_alt_m: f32,
    rep_speed_kt: f32,
    weight: f32,
    rep_profile_idx: u8,
    /// Distinct fids that touched this bucket. `.len()` → `unique_count`.
    fid_set: std::collections::HashSet<u64>,
    /// Bounded top-K candidates keyed on fid for O(1) re-entrance.
    /// A linear scan over K=50 entries finds the min for eviction —
    /// cheap at this size vs the constant factor of a BTreeMap.
    top: HashMap<u64, CruiseTopCandidate>,
    /// Smallest `peak_lmax_25m_db` currently in `top`. Avoids the
    /// full scan when a new candidate is below the cap.
    top_min_lmax: f32,
    source_id: u8,
    origin: u8,
}

impl Default for CruiseAccum {
    fn default() -> Self {
        Self {
            sum_length_m: 0.0,
            rep_len_m: 0.0,
            rep_len_w: 0.0,
            rep_alt_m: 0.0,
            rep_speed_kt: 0.0,
            weight: 0.0,
            rep_profile_idx: 0,
            fid_set: std::collections::HashSet::new(),
            top: HashMap::new(),
            top_min_lmax: f32::NEG_INFINITY,
            source_id: 0,
            origin: 0,
        }
    }
}

impl CruiseAccum {
    fn add(&mut self, seg: &FlightSegment, clip_len_m: f32, npd_luts: &NpdLuts) {
        self.sum_length_m += clip_len_m;
        // rep_alt / rep_speed: clip-length-weighted mean.
        let mid_alt = 0.5 * (seg.start_alt_m + seg.end_alt_m);
        self.rep_alt_m += clip_len_m * mid_alt;
        self.rep_speed_kt += clip_len_m * seg.speed_kt;
        self.weight += clip_len_m;
        // rep_len_m: weighted mean of source-segment length, used as
        // ΔF input. We weight by clip-length so a segment slicing many
        // cells contributes its full length to each cell's mean.
        self.rep_len_m += clip_len_m * seg.length_m;
        self.rep_len_w += clip_len_m;
        self.fid_set.insert(seg.flight_id);
        self.rep_profile_idx = seg.profile_idx;
        self.source_id = seg.source_id;
        self.origin = seg.origin;
        // Source-side peak Lmax at 25 m. Doc 29 §A.3.2 — cruise rows
        // use the Departure NPD curve. NPD `lookup_lmax` indexes by
        // log10(d_ft); 25 m → 82 ft → log10 ≈ 1.914.
        let class_idx = noise_class_of(seg.profile_idx) as usize;
        let log_d = log_d_25m_ft();
        let lmax_db = npd_luts.lookup_lmax(class_idx, true, log_d) as f32;
        self.update_top(seg, lmax_db, mid_alt);
    }

    /// Re-entrant top-K maintenance from a live segment. Builds a
    /// `CruiseTopCandidate` from the segment fields and delegates to
    /// [`merge_top_entry`] so the add and merge paths share one
    /// cap-K + re-entrant implementation.
    fn update_top(&mut self, seg: &FlightSegment, lmax_db: f32, altitude_m: f32) {
        self.merge_top_entry(CruiseTopCandidate {
            flight_id: seg.flight_id,
            callsign: seg.callsign.clone(),
            aircraft_type: seg.aircraft_type,
            peak_lmax_25m_db: lmax_db,
            altitude_m,
        });
    }

    /// Symmetric merge for the Stage 2B fold/reduce. Both `add` and
    /// `merge` must produce the same final accumulator state regardless
    /// of split point — tested by `merge_matches_sequential`.
    fn merge(&mut self, other: CruiseAccum) {
        self.sum_length_m += other.sum_length_m;
        self.rep_alt_m += other.rep_alt_m;
        self.rep_speed_kt += other.rep_speed_kt;
        self.weight += other.weight;
        self.rep_len_m += other.rep_len_m;
        self.rep_len_w += other.rep_len_w;
        for fid in other.fid_set {
            self.fid_set.insert(fid);
        }
        // Replay other's top entries through the cap-K logic so the
        // final accumulator has the true top-K of the union (rev 2
        // accepts that two capped top-50 lists union to top-50 of
        // top-100 — bounded rank pollution at the Kth slot).
        for cand in other.top.into_values() {
            self.merge_top_entry(cand);
        }
        // `rep_profile_idx` / `source_id` / `origin` are NOT
        // invariant per bucket key — different `profile_idx` can map
        // to the same `class`. Both `add` and `merge` pick
        // arbitrarily; downstream remaps `profile_idx` → class so
        // the pick has no measurable effect.
    }

    /// Cap-K + re-entrant top-K maintenance. If the fid is already in
    /// `top`, lift its Lmax on the max-wins rule (loudest segment of
    /// this fid dominates display). If new and `top` is below capacity,
    /// insert. If new and at capacity, evict the current min — but
    /// only when the incoming Lmax beats it.
    fn merge_top_entry(&mut self, cand: CruiseTopCandidate) {
        if let Some(existing) = self.top.get_mut(&cand.flight_id) {
            if cand.peak_lmax_25m_db > existing.peak_lmax_25m_db {
                existing.peak_lmax_25m_db = cand.peak_lmax_25m_db;
                existing.altitude_m = cand.altitude_m;
                // Recompute top_min_lmax — the bumped fid might have
                // been the previous min.
                self.recompute_top_min_lmax();
            }
            return;
        }
        if self.top.len() < CRUISE_TOP_K {
            let lmax = cand.peak_lmax_25m_db;
            self.top.insert(cand.flight_id, cand);
            if lmax < self.top_min_lmax || self.top.len() == 1 {
                self.top_min_lmax = lmax;
            }
            return;
        }
        if cand.peak_lmax_25m_db <= self.top_min_lmax {
            return;
        }
        // Linear scan over K=50: find the current min fid + evict.
        let mut victim_fid = 0u64;
        let mut victim_lmax = f32::INFINITY;
        for (fid, c) in self.top.iter() {
            if c.peak_lmax_25m_db < victim_lmax {
                victim_lmax = c.peak_lmax_25m_db;
                victim_fid = *fid;
            }
        }
        self.top.remove(&victim_fid);
        self.top.insert(cand.flight_id, cand);
        self.recompute_top_min_lmax();
    }

    /// Reset `top_min_lmax` to the current minimum across `top`.
    /// Called after any operation that could have removed or
    /// lifted the previous min.
    fn recompute_top_min_lmax(&mut self) {
        self.top_min_lmax = self
            .top
            .values()
            .map(|c| c.peak_lmax_25m_db)
            .fold(f32::INFINITY, f32::min);
    }

    fn finalize(self, key: CruiseKey) -> CruiseBucket {
        let w = self.weight.max(1e-6);
        let lw = self.rep_len_w.max(1e-6);
        let unique_count = self.fid_set.len() as u32;
        let mut top_candidates: Vec<CruiseTopCandidate> = self.top.into_values().collect();
        // Sort by Lmax descending (tiebreak fid ascending) so on-disk
        // bytes stay deterministic across re-extracts.
        top_candidates.sort_by(|a, b| {
            b.peak_lmax_25m_db
                .partial_cmp(&a.peak_lmax_25m_db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.flight_id.cmp(&b.flight_id))
        });
        // Doc 29 §A.3.2 — cruise rows always use the Departure NPD curve
        // (en-route flights inherit Departure NPD; no cruise NPD set
        // is published). The kernel hardcodes `is_departure: true` on
        // the synth `AircraftSegment` it builds from each row, so no
        // per-row column is needed.
        CruiseBucket {
            r7_hex: key.r7_hex,
            class: key.class,
            rep_profile_idx: self.rep_profile_idx,
            fl_bin: key.fl_bin,
            period: key.period,
            sum_length_m: self.sum_length_m,
            rep_len_m: self.rep_len_m / lw,
            rep_alt_m: self.rep_alt_m / w,
            rep_speed_kt: self.rep_speed_kt / w,
            unique_count,
            top_candidates,
            source_id: self.source_id,
            origin: self.origin,
        }
    }
}

/// Test-only symmetric merger over two `(R4 → bucket)` maps, used to
/// verify `CruiseAccum::merge` matches the sequential `add` path. The
/// production spill-merge does the same per-entry `merge` inline.
#[cfg(test)]
fn merge_by_r4(
    mut a: HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    mut b: HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
) -> HashMap<u64, HashMap<CruiseKey, CruiseAccum>> {
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    for (r4, b_inner) in b {
        let entry = a.entry(r4).or_default();
        for (key, b_accum) in b_inner {
            match entry.get_mut(&key) {
                Some(existing) => existing.merge(b_accum),
                None => {
                    entry.insert(key, b_accum);
                }
            }
        }
    }
    a
}

/// Densify a cruise segment, find every R7 cell along the way, compute
/// analytical clip length, and accumulate into the right (R4 → bucket)
/// map entry. Out-of-scope R7 cells are dropped here so they never
/// inflate the worker accumulator or hit disk during spill.
fn process_segment(
    seg: &FlightSegment,
    by_r4: &mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    scope: Option<&ScopeBbox>,
    npd_luts: &NpdLuts,
) {
    let cells = densify_to_cells(
        seg.start_lat,
        seg.start_lon,
        seg.end_lat,
        seg.end_lon,
        Resolution::Seven,
    );
    let class = noise_class_of(seg.profile_idx);
    let avg_alt_m = (seg.start_alt_m + seg.end_alt_m) * 0.5;
    let fl_bin = fl_bin_of(avg_alt_m);
    let period = seg.period;

    for cell in cells {
        let clip_m =
            line_cell_clip_length(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon, cell);
        if clip_m < 1.0 {
            continue;
        }
        let r4 = u64::from(cell.parent(Resolution::Four).unwrap_or(cell));
        if let Some(s) = scope {
            if !s.contains_r4(r4) {
                continue;
            }
        }
        let key = CruiseKey {
            r7_hex: u64::from(cell),
            class,
            fl_bin,
            period,
        };
        by_r4
            .entry(r4)
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

/// Walk a great-circle arc from (lat0, lon0) to (lat1, lon1) at
/// [`DENSIFY_STEP_M`] increments, collect every distinct R7 cell along
/// the path. Antimeridian-safe via `signed_lon_diff` interpolation.
pub fn densify_to_cells(
    lat0: f32,
    lon0: f32,
    lat1: f32,
    lon1: f32,
    res: Resolution,
) -> Vec<CellIndex> {
    let total = flat_dist(lat0, lon0, lat1, lon1);
    let n = ((total / DENSIFY_STEP_M).ceil() as usize).max(1);
    let dlat = lat1 - lat0;
    let dlon = signed_lon_diff(lon0, lon1);
    let mut seen: Vec<CellIndex> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let lat = lat0 + dlat * t;
        let mut lon = lon0 + dlon * t;
        if lon > 180.0 {
            lon -= 360.0;
        } else if lon <= -180.0 {
            lon += 360.0;
        }
        if let Ok(ll) = LatLng::new(lat as f64, lon as f64) {
            let cell = ll.to_cell(res);
            if !seen.contains(&cell) {
                seen.push(cell);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests;
