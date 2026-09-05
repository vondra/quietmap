//! Two-pass destination shuffle: intact airborne support copies and midpoint-owned ground.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::{for_each_segment_batch, read_segments, write_segments};
use crate::flight::{FlightSegment, Phase};
use crate::geo::{midpoint, square_path};
use crate::progress::{Milestone, finished, human, started};
use crate::scope::ScopeBbox;
use crate::spatial::{square_directories, square_id};

/// Partition destination cells across 256 gather tasks; each worker writes
/// one file at a time, independently of the number of supported cells.
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

fn destination_squares<'a>(
    segment: &FlightSegment,
    scope: Option<&'a ScopeBbox>,
) -> Result<impl Iterator<Item = u64> + 'a> {
    let airborne = if segment.phase == Phase::Airborne {
        Some(
            crate::support::airborne_segment_support(segment)
                .context("invalid airborne support")?,
        )
    } else {
        None
    };
    let ground = (segment.phase == Phase::Ground)
        .then(|| square_of_midpoint(segment))
        .flatten();
    Ok(airborne
        .into_iter()
        .flat_map(|support| support.iter().map(|square| grid::square_id(square) as u64))
        .chain(ground)
        .filter(move |&square| scope.is_none_or(|scope| scope.contains_square(square))))
}

fn phase_name(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Airborne => Some("airborne"),
        Phase::Ground => Some("ground"),
        _ => None,
    }
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
/// hybrid GA window's per-day shards (empty for single-window extracts).
/// The two windows merge here because this is the last per-day stage;
/// Stage 2 consumers read one per-z9 pool and weight rows per class.
///
/// `scope` filters destination cells in both passes, including support cells
/// reached from an original segment whose midpoint is outside the scope.
/// The next run removes temporary parts left by an interrupted gather.
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
        &temp_dir,
        scope,
        &counter,
    )?;
    pass_a(ga_day_paths, "ga", true, &temp_dir, scope, &counter)?;
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
    let pass_b_shards = pass_b(&temp_dir, out_dir, scope)?;
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

/// Bound routed row payload per scatter worker. Flush within a source row's
/// support expansion so a long segment cannot multiply a whole decoded day.
const PASS_A_SPILL_BYTES: usize = 512 * 1024 * 1024;

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
    temp_dir: &Path,
    scope: Option<&ScopeBbox>,
    counter: &Milestone,
) -> Result<()> {
    let mut largest_input_bytes = 0;
    for path in day_paths {
        largest_input_bytes = largest_input_bytes.max(path.metadata()?.len());
    }
    // The decoder retains one IPC batch, which can be a whole legacy file.
    // Budget its file bytes and decoded slice at 2x, plus 4x routed payload
    // for Vec capacity and the temporary Arrow buffers during a flush.
    let peak_per_day_gb =
        (largest_input_bytes as f64 * 2.0 + PASS_A_SPILL_BYTES as f64 * 4.0) / 1_000_000_000.0;
    for chunk in day_paths.chunks(crate::memory::max_concurrent_days(
        day_paths.len(),
        peak_per_day_gb,
    )) {
        chunk.par_iter().try_for_each(|day_path| -> Result<()> {
            counter.add(scatter_day(
                day_path,
                pass,
                hybrid,
                temp_dir,
                scope,
                PASS_A_SPILL_BYTES,
            )?);
            Ok(())
        })?;
    }
    Ok(())
}

fn scatter_day(
    day_path: &Path,
    pass: &'static str,
    hybrid: bool,
    temp_dir: &Path,
    scope: Option<&ScopeBbox>,
    spill_bytes: usize,
) -> Result<u64> {
    let day_stem = day_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("missing file stem: {}", day_path.display()))?;
    let expected_date = crate::period::parse_date_id(day_stem)?;
    let mut buckets = HashMap::new();
    let mut buffered_bytes = 0;
    let mut part = 0;
    let mut kept = 0;
    for_each_segment_batch(day_path, |segments| {
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
            let row_bytes = std::mem::size_of::<FlightSegment>() + seg.callsign.len();
            // Keep this set across flushes inside one original observation.
            // Gather expands each hash copy to its cells exactly once.
            let mut hashes = [false; SHUFFLE_HASH_BUCKETS as usize];
            for square in destination_squares(&seg, scope)? {
                let hash = shuffle_bucket(square);
                if !std::mem::replace(&mut hashes[hash as usize], true) {
                    buckets
                        .entry((phase, hash))
                        .or_insert_with(Vec::new)
                        .push(seg.clone());
                    kept += 1;
                    buffered_bytes += row_bytes;
                    if buffered_bytes >= spill_bytes {
                        flush_pass_a(&mut buckets, temp_dir, pass, day_stem, &mut part)?;
                        buffered_bytes = 0;
                    }
                }
            }
        }
        Ok(())
    })
    .with_context(|| format!("scatter {}", day_path.display()))?;
    flush_pass_a(&mut buckets, temp_dir, pass, day_stem, &mut part)?;
    Ok(kept)
}

/// Pass-A temp shard key carries a pass discriminator (`air_` / `ga_`)
/// because the airline and GA hybrid passes share first-of-month day
/// stems (`2025-07-01` …) — an undiscriminated `day_<stem>.arrow` path
/// would race and silently overwrite one pass's segments.
fn flush_pass_a(
    buckets: &mut HashMap<(&'static str, u64), Vec<FlightSegment>>,
    temp_dir: &Path,
    pass: &str,
    day_stem: &str,
    part: &mut u64,
) -> Result<()> {
    for ((phase, hash), segments) in std::mem::take(buckets) {
        let path = pass_a_bucket_dir(temp_dir, phase, hash)
            .join(format!("day_{pass}_{day_stem}_part_{part:016x}.arrow"));
        write_segments(&path, &segments)?;
    }
    *part += 1;
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

fn pass_b(temp_dir: &Path, out_dir: &Path, scope: Option<&ScopeBbox>) -> Result<u64> {
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
                            for square in destination_squares(&seg, scope)? {
                                if shuffle_bucket(square) == hash {
                                    by_square.entry(square).or_default().push(seg.clone());
                                }
                            }
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
    out.sort_unstable();
    Ok(out)
}

#[cfg(test)]
#[path = "shuffle_tests.rs"]
mod tests;
