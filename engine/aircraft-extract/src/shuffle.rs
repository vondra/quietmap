//! Two-pass shuffle from `segments/<day>.arrow` to per-z9 shards.
//!
//! Why two-pass: a naive single-pass approach would either open one
//! writer per z9 per worker (10k z9s × 24 workers = FD exhaustion) or
//! mutex-serialise writes through one writer per z9 (bottleneck).
//!
//! - **Pass A (scatter).** `par_iter` over `segments/<day>.arrow`. Each
//!   worker decodes one day, partitions Airborne + Ground segments by
//!   `(phase, hash(z9) % SHUFFLE_HASH_BUCKETS)` in RAM, then writes one
//!   complete IPC file per non-empty bucket sequentially — 1 FD per
//!   worker at a time. Paths embed the day so workers never collide.
//!   Cruise is left in the per-day shards (Stage 2B reads them directly;
//!   midpoint-shuffling cruise would lose cross-z9 cell routing).
//! - **Pass B (gather).** `par_iter` over `2 × SHUFFLE_HASH_BUCKETS`
//!   `(phase, hash)` pairs. Each worker reads every Pass A part file for
//!   its pair, buckets exactly by z9 in RAM, writes
//!   `<out_dir>/<z9>/<phase>.arrow` sequentially. Per-worker peak RAM
//!   bounded by one (phase, hash) bucket's decoded segments.
//!
//! At full-year global scale, Pass A produces `days × 2 × 256` possible
//! temp files (decode cost dominates write cost). Pass B's per-bucket
//! decoded peak ≈ `total_segments / 256` ≈ ~6 GB per worker; `--max-threads`
//! caps concurrent workers when the host's RAM is below that × cores.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::{read_segments, write_segments};
use crate::flight::{FlightSegment, Phase};
use crate::geo::{midpoint, square_path};
use crate::progress::{finished, human, started, Milestone};
use crate::scope::ScopeBbox;
use crate::spatial::{square_directories, square_id};

/// Coarse hash buckets the shuffle is partitioned across. 256 is small
/// enough that one Pass B worker holds one bucket's decoded segments in
/// RAM (~6 GB at full-year global scale) without OOM on a 90-128 GB box, and
/// large enough that Pass A's per-worker bucket count stays well under
/// any FD limit (each writer opens one file at a time).
const SHUFFLE_HASH_BUCKETS: u64 = 256;

fn shuffle_bucket(square: u64) -> u64 {
    let mut x = square;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    let mixed = x ^ (x >> 31);
    mixed % SHUFFLE_HASH_BUCKETS
}

fn square_of_midpoint(seg: &FlightSegment) -> Option<u64> {
    let (mid_lat, mid_lon) = midpoint(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon);
    square_id(mid_lat as f64, mid_lon as f64)
}

fn phase_name(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Airborne => Some("airborne"),
        Phase::Ground => Some("ground"),
        _ => None,
    }
}

/// Pass-A temp shard key carries a pass discriminator (`air_` / `ga_`)
/// because the airline and GA hybrid passes share first-of-month day
/// stems (`2025-07-01` …) — an undiscriminated `day_<stem>.arrow` path
/// would race and silently overwrite one pass's segments.
fn pass_a_path(temp_dir: &Path, phase: &str, hash: u64, pass: &str, day_stem: &str) -> PathBuf {
    temp_dir
        .join(phase)
        .join(format!("hash_{hash:03x}"))
        .join(format!("day_{pass}_{day_stem}.arrow"))
}

fn pass_a_bucket_dir(temp_dir: &Path, phase: &str, hash: u64) -> PathBuf {
    temp_dir.join(phase).join(format!("hash_{hash:03x}"))
}

/// Shuffle Stage 1 per-day shards into per-z9 shards for Stages 1.5 /
/// 2A / 2C. Output layout:
///
/// ```text
/// <out_dir>/<z9>/airborne.arrow
/// <out_dir>/<z9>/ground.arrow
/// ```
///
/// `day_paths` is the primary (airline) window; `ga_day_paths` is the
/// hybrid GA window's per-day shards (empty for plain single-window
/// extracts — output is then byte-identical to the pre-hybrid shuffle).
/// The two windows merge here because this is the last per-day stage;
/// Stage 2 consumers read one per-z9 pool and weight rows per class.
///
/// `scope`, when set, drops out-of-scope z9s during Pass A so they
/// never hit disk. Cleanup of the temp scratch dir is best-effort —
/// the next run's `if exists` wipe at start handles a crash mid-merge.
pub fn shuffle_per_square(
    day_paths: &[PathBuf],
    ga_day_paths: &[PathBuf],
    out_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<()> {
    require_unique_day_stems("airline", day_paths)?;
    require_unique_day_stems("GA", ga_day_paths)?;
    let temp_dir = out_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("out_dir has no parent for temp_shuffle sibling"))?
        .join("temp_shuffle");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    for phase in ["airborne", "ground"] {
        for h in 0..SHUFFLE_HASH_BUCKETS {
            std::fs::create_dir_all(pass_a_bucket_dir(&temp_dir, phase, h))?;
        }
    }
    std::fs::create_dir_all(out_dir)?;

    started(
        "shuffle/passA",
        &format!(
            "{} airline + {} GA day shards",
            day_paths.len(),
            ga_day_paths.len()
        ),
    );
    let pass_a_start = std::time::Instant::now();
    let counter = Milestone::new("shuffle/passA", "segments", 1_000_000);
    pass_a(
        day_paths,
        "air",
        !ga_day_paths.is_empty(),
        PASS_A_PEAK_PER_DAY_GB,
        &temp_dir,
        scope,
        &counter,
    )?;
    pass_a(
        ga_day_paths,
        "ga",
        true,
        PASS_A_GA_PEAK_PER_DAY_GB,
        &temp_dir,
        scope,
        &counter,
    )?;
    let pass_a_total = counter.total();
    finished(
        "shuffle/passA",
        &format!(
            "{} segments scattered in {:?}",
            human(pass_a_total),
            pass_a_start.elapsed()
        ),
    );

    // Wipe `out_dir` too: stale `<z9>/{airborne,ground}.arrow` from a
    // crashed previous run or a narrower-scope rerun would otherwise
    // leak into Stage 1.5/2A/2C as zombie data (list_square_shards has no
    // way to know which shards belong to the current scope).
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)?;
    }
    std::fs::create_dir_all(out_dir)?;
    started(
        "shuffle/passB",
        &format!("{} hash buckets", SHUFFLE_HASH_BUCKETS),
    );
    let pass_b_start = std::time::Instant::now();
    let pass_b_shards = pass_b(&temp_dir, out_dir)?;
    finished(
        "shuffle/passB",
        &format!(
            "{pass_b_shards} z9 phase shards written → {} in {:?}",
            out_dir.display(),
            pass_b_start.elapsed()
        ),
    );

    let _ = std::fs::remove_dir_all(&temp_dir);

    for (name, paths) in [("days", day_paths), ("ga_days", ga_day_paths)] {
        let mut days: Vec<_> = paths
            .iter()
            .map(|path| path.file_stem().unwrap().to_str().unwrap())
            .collect();
        days.sort_unstable();
        std::fs::write(out_dir.join(name), days.join("\n"))
            .with_context(|| format!("write {name} in {}", out_dir.display()))?;
    }
    Ok(())
}

/// Pass A per-day decode RAM estimate for full (airline-window) day
/// shards — same 28 GB/day calibration as the Stage 0/1 day cap.
const PASS_A_PEAK_PER_DAY_GB: f64 = 28.0;

/// GA-filtered day shards decode to a small fraction of a full day
/// (only PROP_C172 + HELICOPTER classes survive Stage 0); the full-day
/// estimate would throttle the GA pass to 2-3 concurrent days for no
/// RAM benefit.
const PASS_A_GA_PEAK_PER_DAY_GB: f64 = 6.0;

/// Bail on duplicate day stems within one pass list: Pass A keys temp
/// shards by `(pass, day_stem)`, so two same-stem inputs in one list
/// would race on one temp path and silently drop segments — the
/// within-pass analog of the cross-pass collision above.
fn require_unique_day_stems(pass: &str, day_paths: &[PathBuf]) -> Result<()> {
    let mut seen = std::collections::HashSet::with_capacity(day_paths.len());
    for path in day_paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("missing file stem: {}", path.display()))?;
        crate::period::parse_date_id(stem)?;
        if !seen.insert(stem.to_string()) {
            anyhow::bail!(
                "duplicate day stem '{stem}' in the {pass} shuffle input ({}) — two \
                 shards would collide on one Pass-A temp path and silently drop \
                 segments; dedupe the input day lists",
                path.display()
            );
        }
    }
    Ok(())
}

fn pass_a(
    day_paths: &[PathBuf],
    pass: &'static str,
    hybrid: bool,
    peak_per_day_gb: f64,
    temp_dir: &Path,
    scope: Option<&ScopeBbox>,
    counter: &Milestone,
) -> Result<()> {
    // Bound how many days are decoded into RAM at once. Each worker holds a
    // full day's segments + per-bucket HashMaps (~16 GB); a naive par_iter over
    // all 7 global days hit ~114 GB anon and was cgroup-OOM-killed. Chunk by the
    // same ~28 GB/day budget as Stage 0/1 (sized to host RAM or the cgroup
    // limit, whichever is smaller). Within a chunk par_iter still fills cores.
    for chunk in day_paths.chunks(crate::memory::max_concurrent_days(
        day_paths.len(),
        peak_per_day_gb,
    )) {
        chunk.par_iter().try_for_each(|day_path| -> Result<()> {
            let segments =
                read_segments(day_path).with_context(|| format!("read {}", day_path.display()))?;
            let day_stem = day_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("missing file stem: {}", day_path.display()))?;

            let expected_date = crate::period::parse_date_id(day_stem)?;
            let mut buckets: HashMap<(&'static str, u64), Vec<FlightSegment>> = HashMap::new();
            let mut kept = 0u64;
            for seg in segments {
                anyhow::ensure!(
                    seg.date_id == expected_date,
                    "segment date disagrees with {}",
                    day_path.display()
                );
                let ga_class =
                    seg.veh_kind == 0 && crate::profile::is_ga_sampled_profile(seg.profile_idx);
                anyhow::ensure!(
                    !hybrid || ga_class == (pass == "ga"),
                    "{pass} hybrid pass contains a segment from the other sampling window: {}",
                    day_path.display()
                );
                let Some(phase) = phase_name(seg.phase) else {
                    continue;
                };
                let Some(square) = square_of_midpoint(&seg) else {
                    continue;
                };
                if let Some(s) = scope {
                    if !s.contains_square(square) {
                        continue;
                    }
                }
                buckets
                    .entry((phase, shuffle_bucket(square)))
                    .or_default()
                    .push(seg);
                kept += 1;
            }

            // Sequential per-bucket write — paths are unique per
            // (phase, hash, pass, day) so no worker writes the same file.
            for ((phase, hash), segs) in buckets {
                write_segments(&pass_a_path(temp_dir, phase, hash, pass, day_stem), &segs)?;
            }
            counter.add(kept);
            Ok(())
        })?;
    }
    Ok(())
}

/// How many hash buckets Pass B regroups in RAM concurrently. Each worker
/// loads its WHOLE bucket into a `HashMap<z9, Vec<FlightSegment>>`, so the
/// working set is `concurrency × bucket_ram`. Hash bucketing spreads z9s
/// uniformly, so buckets are near-equal — but at world scale (6.9 B segments,
/// 2026-06-12) a full-width pool meant 24 × ~6.4 GB ≈ 150 GB on a 94 GB host:
/// 63 GB of swap, ~2 shards/min, a de-facto deadlock. Same policy as Pass A:
/// size to 60 % of min(host, cgroup) RAM, est. RAM = 2× the on-disk part
/// bytes of the LARGEST bucket (arrow → Vec expansion, conservative).
fn pass_b_concurrency(temp_dir: &Path) -> Result<usize> {
    let mut max_bucket_bytes: u64 = 0;
    for phase in ["airborne", "ground"] {
        for hash in 0..SHUFFLE_HASH_BUCKETS {
            let mut bytes = 0;
            for path in list_pass_a_parts(&pass_a_bucket_dir(temp_dir, phase, hash))? {
                bytes += std::fs::metadata(&path)?.len();
            }
            max_bucket_bytes = max_bucket_bytes.max(bytes);
        }
    }
    let est_ram_per_bucket_gb = (max_bucket_bytes as f64 * 2.0) / 1_000_000_000.0;
    if est_ram_per_bucket_gb < 0.1 {
        return Ok(rayon::current_num_threads());
    }
    Ok(crate::memory::max_concurrent_days(
        rayon::current_num_threads(),
        est_ram_per_bucket_gb,
    ))
}

fn pass_b(temp_dir: &Path, out_dir: &Path) -> Result<u64> {
    let phases = ["airborne", "ground"];
    let workers = pass_b_concurrency(temp_dir)?;
    started(
        "shuffle/passB",
        &format!("{workers} concurrent buckets (RAM-bounded)"),
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .context("build passB pool")?;
    let bucket_counter = Milestone::new("shuffle/passB", "buckets", 25);
    let shard_counter = Milestone::new("shuffle/passB", "shards", 1_000);
    pool.install(|| {
        (0..SHUFFLE_HASH_BUCKETS)
            .into_par_iter()
            .try_for_each(|hash| -> Result<()> {
                let mut shards_this_bucket = 0u64;
                for phase in phases {
                    let parts = list_pass_a_parts(&pass_a_bucket_dir(temp_dir, phase, hash))?;
                    if parts.is_empty() {
                        continue;
                    }
                    let mut by_square: HashMap<u64, Vec<FlightSegment>> = HashMap::new();
                    for part in &parts {
                        let segs = read_segments(part)
                            .with_context(|| format!("read {}", part.display()))?;
                        for seg in segs {
                            let Some(square) = square_of_midpoint(&seg) else {
                                continue;
                            };
                            by_square.entry(square).or_default().push(seg);
                        }
                    }
                    for (square, segs) in by_square {
                        let square_dir = out_dir.join(square_path(square));
                        std::fs::create_dir_all(&square_dir)?;
                        write_segments(&square_dir.join(format!("{phase}.arrow")), &segs)?;
                        shards_this_bucket += 1;
                    }
                }
                bucket_counter.add(1);
                shard_counter.add(shards_this_bucket);
                Ok(())
            })
    })?;
    Ok(shard_counter.total())
}

/// Walk `<segments_by_square_dir>/<z9>/<shard_name>` for in-scope z9s —
/// the dual of `shuffle_per_square`'s output. Stage 1.5 / 2A / 2C each call
/// this once per stage to drive their per-z9 par_iter.
pub fn list_square_shards(
    segments_by_square_dir: &Path,
    shard_name: &str,
    scope: Option<&ScopeBbox>,
) -> Result<Vec<(u64, PathBuf)>> {
    let mut out = Vec::new();
    for (id, path) in square_directories(segments_by_square_dir)? {
        if scope.is_some_and(|scope| !scope.contains_square(id)) {
            continue;
        }
        let shard = path.join(shard_name);
        match std::fs::metadata(&shard) {
            Ok(metadata) if metadata.is_file() => out.push((id, shard)),
            Ok(_) => anyhow::bail!("{} is not a shard file", shard.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("stat {}", shard.display())),
        }
    }
    Ok(out)
}

fn list_pass_a_parts(dir: &Path) -> Result<Vec<PathBuf>> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in read {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("arrow") {
            out.push(path);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "shuffle_tests.rs"]
mod tests;
