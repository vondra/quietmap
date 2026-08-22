//! Two-pass shuffle from `segments/<day>.arrow` to per-R4 shards.
//!
//! Why two-pass: a naive single-pass approach would either open one
//! writer per R4 per worker (10k R4s × 24 workers = FD exhaustion) or
//! mutex-serialise writes through one writer per R4 (bottleneck).
//!
//! - **Pass A (scatter).** `par_iter` over `segments/<day>.arrow`. Each
//!   worker decodes one day, partitions Airborne + Ground segments by
//!   `(phase, hash(R4) % SHUFFLE_HASH_BUCKETS)` in RAM, then writes one
//!   complete IPC file per non-empty bucket sequentially — 1 FD per
//!   worker at a time. Paths embed the day so workers never collide.
//!   Cruise is left in the per-day shards (Stage 2B reads them directly;
//!   midpoint-shuffling cruise would lose cross-R4 cell routing).
//! - **Pass B (gather).** `par_iter` over `2 × SHUFFLE_HASH_BUCKETS`
//!   `(phase, hash)` pairs. Each worker reads every Pass A part file for
//!   its pair, buckets exactly by R4 in RAM, writes
//!   `<out_dir>/<R4>/<phase>.arrow` sequentially. Per-worker peak RAM
//!   bounded by one (phase, hash) bucket's decoded segments.
//!
//! At full-year global scale, Pass A produces `days × 2 × 256` possible
//! temp files (decode cost dominates write cost). Pass B's per-bucket
//! decoded peak ≈ `total_segments / 256` ≈ ~6 GB per worker; `--max-threads`
//! caps concurrent workers when the host's RAM is below that × cores.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use h3o::Resolution;
use rayon::prelude::*;

use crate::arrow_io::{read_segments, write_segments};
use crate::flight::{FlightSegment, Phase};
use crate::geo::{lat_lon_to_cell, midpoint, r4_hex_str};
use crate::progress::{finished, human, started, Milestone};
use crate::scope::ScopeBbox;

/// Coarse hash buckets the shuffle is partitioned across. 256 is small
/// enough that one Pass B worker holds one bucket's decoded segments in
/// RAM (~6 GB at full-year global scale) without OOM on a 90-128 GB box, and
/// large enough that Pass A's per-worker bucket count stays well under
/// any FD limit (each writer opens one file at a time).
const SHUFFLE_HASH_BUCKETS: u64 = 256;

/// SplitMix64 over the H3 cell index — same rationale as
/// `stage_2b::spill_bucket`. H3 packs mode/resolution/base-cell into
/// high bits with digit bits in low positions; a naive `% N` would
/// correlate buckets with the lowest digits and skew load. The mixer
/// avalanches both halves so adjacent R4 cells land in independent
/// buckets. Constants are the published SplitMix64 mixers.
fn shuffle_bucket(r4: u64) -> u64 {
    let mut x = r4;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    let mixed = x ^ (x >> 31);
    mixed % SHUFFLE_HASH_BUCKETS
}

fn r4_of_midpoint(seg: &FlightSegment) -> Option<u64> {
    let (mid_lat, mid_lon) = midpoint(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon);
    lat_lon_to_cell(mid_lat as f64, mid_lon as f64, Resolution::Four).map(u64::from)
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

/// Shuffle Stage 1 per-day shards into per-R4 shards for Stages 1.5 /
/// 2A / 2C. Output layout:
///
/// ```text
/// <out_dir>/<R4>/airborne.arrow
/// <out_dir>/<R4>/ground.arrow
/// ```
///
/// `day_paths` is the primary (airline) window; `ga_day_paths` is the
/// hybrid GA window's per-day shards (empty for plain single-window
/// extracts — output is then byte-identical to the pre-hybrid shuffle).
/// The two windows merge here because this is the last per-day stage;
/// Stage 2 consumers read one per-R4 pool and weight rows per class.
///
/// `scope`, when set, drops out-of-scope R4s during Pass A so they
/// never hit disk. Cleanup of the temp scratch dir is best-effort —
/// the next run's `if exists` wipe at start handles a crash mid-merge.
pub fn shuffle_per_r4(
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
    // Wipe `out_dir` too: stale `<R4>/{airborne,ground}.arrow` from a
    // crashed previous run or a narrower-scope rerun would otherwise
    // leak into Stage 1.5/2A/2C as zombie data (list_r4_shards has no
    // way to know which shards belong to the current scope).
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)?;
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
        PASS_A_PEAK_PER_DAY_GB,
        &temp_dir,
        scope,
        &counter,
    )?;
    pass_a(
        ga_day_paths,
        "ga",
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

    started(
        "shuffle/passB",
        &format!("{} hash buckets", SHUFFLE_HASH_BUCKETS),
    );
    let pass_b_start = std::time::Instant::now();
    let pass_b_shards = pass_b(&temp_dir, out_dir)?;
    finished(
        "shuffle/passB",
        &format!(
            "{pass_b_shards} R4 phase shards written → {} in {:?}",
            out_dir.display(),
            pass_b_start.elapsed()
        ),
    );

    let _ = std::fs::remove_dir_all(&temp_dir);

    // Day-count manifests: distinct extracted days shuffled into `out_dir`
    // per window = the true Lden normalization denominators. Written here
    // (every recreation of `out_dir` carries them, including the standalone
    // `shuffle` subcommand) rather than derived downstream from `--days`,
    // which a `--from-stage` re-run takes from a possibly-stale ADS-B cache
    // (the 2026-05-24 n_days=7-on-full-year mislabel). `list_r4_shards`
    // skips them (not directories). Both lists are deduped per day by the
    // caller + the stem-uniqueness check above. `n_days` keeps its airline
    // (primary-window) semantics for every existing reader; `ga_n_days` is
    // written only for hybrid extracts so a plain extract's output tree
    // stays byte-identical and `read_ga_n_days` falls back to 0.
    std::fs::write(out_dir.join("n_days"), day_paths.len().to_string())
        .with_context(|| format!("write n_days manifest in {}", out_dir.display()))?;
    if !ga_day_paths.is_empty() {
        std::fs::write(out_dir.join("ga_n_days"), ga_day_paths.len().to_string())
            .with_context(|| format!("write ga_n_days manifest in {}", out_dir.display()))?;
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
    for chunk in day_paths.chunks(pass_a_max_concurrent_days(day_paths.len(), peak_per_day_gb)) {
        chunk.par_iter().try_for_each(|day_path| -> Result<()> {
            let segments =
                read_segments(day_path).with_context(|| format!("read {}", day_path.display()))?;
            let day_stem = day_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("missing file stem: {}", day_path.display()))?;

            let mut buckets: HashMap<(&'static str, u64), Vec<FlightSegment>> = HashMap::new();
            let mut kept = 0u64;
            for seg in segments {
                let Some(phase) = phase_name(seg.phase) else {
                    continue;
                };
                let Some(r4) = r4_of_midpoint(&seg) else {
                    continue;
                };
                if let Some(s) = scope {
                    if !s.contains_r4(r4) {
                        continue;
                    }
                }
                buckets
                    .entry((phase, shuffle_bucket(r4)))
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

/// How many days Pass A decodes into RAM concurrently, sized to the smaller of
/// host RAM and this process's cgroup memory limit (`peak_per_day_gb` effective
/// per concurrent day, 60% of budget) — the same policy as the Stage 0/1 day
/// cap in the orchestrator. Kept local to avoid a bin->lib dependency.
/// TODO: hoist this and the bin's copy into one shared crate util.
fn pass_a_max_concurrent_days(num_days: usize, peak_per_day_gb: f64) -> usize {
    let host = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(16u64 * 1024 * 1024 * 1024);
    let cgroup = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|cg| {
            cg.lines()
                .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
        })
        .and_then(|rel| {
            std::fs::read_to_string(format!("/sys/fs/cgroup{}/memory.max", rel.trim())).ok()
        })
        .and_then(|raw| raw.trim().parse::<u64>().ok());
    let budget_gb = cgroup.map_or(host, |c| host.min(c)) as f64 / 1_000_000_000.0;
    ((budget_gb * 0.60 / peak_per_day_gb).floor() as usize).clamp(1, num_days.max(1))
}

/// How many hash buckets Pass B regroups in RAM concurrently. Each worker
/// loads its WHOLE bucket into a `HashMap<R4, Vec<FlightSegment>>`, so the
/// working set is `concurrency × bucket_ram`. Hash bucketing spreads R4s
/// uniformly, so buckets are near-equal — but at world scale (6.9 B segments,
/// 2026-06-12) a full-width pool meant 24 × ~6.4 GB ≈ 150 GB on a 94 GB host:
/// 63 GB of swap, ~2 shards/min, a de-facto deadlock. Same policy as Pass A:
/// size to 60 % of min(host, cgroup) RAM, est. RAM = 2× the on-disk part
/// bytes of the LARGEST bucket (arrow → Vec expansion, conservative).
/// `QM_SHUFFLE_PASSB_THREADS` overrides for emergencies.
fn pass_b_concurrency(temp_dir: &Path) -> usize {
    if let Some(n) = std::env::var("QM_SHUFFLE_PASSB_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return n.max(1);
    }
    let mut max_bucket_bytes: u64 = 0;
    for phase in ["airborne", "ground"] {
        for hash in 0..SHUFFLE_HASH_BUCKETS {
            let bytes = list_pass_a_parts(&pass_a_bucket_dir(temp_dir, phase, hash))
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| std::fs::metadata(p).ok())
                        .map(|m| m.len())
                        .sum()
                })
                .unwrap_or(0);
            max_bucket_bytes = max_bucket_bytes.max(bytes);
        }
    }
    let est_ram_per_bucket_gb = (max_bucket_bytes as f64 * 2.0) / 1_000_000_000.0;
    if est_ram_per_bucket_gb < 0.1 {
        return rayon::current_num_threads();
    }
    // 60%-of-budget policy shared with pass_a_max_concurrent_days.
    let budget_gb = {
        let host = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<u64>().ok())
            })
            .map(|kb| kb * 1024)
            .unwrap_or(16u64 * 1024 * 1024 * 1024);
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|cg| {
                cg.lines()
                    .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
            })
            .and_then(|rel| {
                std::fs::read_to_string(format!("/sys/fs/cgroup{}/memory.max", rel.trim())).ok()
            })
            .and_then(|raw| raw.trim().parse::<u64>().ok());
        cgroup.map_or(host, |c| host.min(c)) as f64 / 1_000_000_000.0
    };
    ((budget_gb * 0.60 / est_ram_per_bucket_gb).floor() as usize)
        .clamp(1, rayon::current_num_threads())
}

fn pass_b(temp_dir: &Path, out_dir: &Path) -> Result<u64> {
    let phases = ["airborne", "ground"];
    let workers = pass_b_concurrency(temp_dir);
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
                    let mut by_r4: HashMap<u64, Vec<FlightSegment>> = HashMap::new();
                    for part in &parts {
                        let segs = read_segments(part)
                            .with_context(|| format!("read {}", part.display()))?;
                        for seg in segs {
                            let Some(r4) = r4_of_midpoint(&seg) else {
                                continue;
                            };
                            by_r4.entry(r4).or_default().push(seg);
                        }
                    }
                    for (r4, segs) in by_r4 {
                        let r4_dir = out_dir.join(r4_hex_str(r4));
                        std::fs::create_dir_all(&r4_dir)?;
                        write_segments(&r4_dir.join(format!("{phase}.arrow")), &segs)?;
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

/// Walk `<segments_by_r4_dir>/<R4>/<shard_name>` for in-scope R4s —
/// the dual of `shuffle_per_r4`'s output. Stage 1.5 / 2A / 2C each call
/// this once per stage to drive their per-R4 par_iter.
pub fn list_r4_shards(
    segments_by_r4_dir: &Path,
    shard_name: &str,
    scope: Option<&ScopeBbox>,
) -> Result<Vec<(u64, PathBuf)>> {
    let read = match std::fs::read_dir(segments_by_r4_dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("read_dir {}", segments_by_r4_dir.display()))
        }
    };
    let mut out = Vec::new();
    for entry in read {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(r4) = u64::from_str_radix(name, 16) else {
            continue;
        };
        if let Some(s) = scope {
            if !s.contains_r4(r4) {
                continue;
            }
        }
        let shard = path.join(shard_name);
        if shard.exists() {
            out.push((r4, shard));
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
mod tests {
    use super::*;
    use crate::flight::Phase;

    fn seg(flight_id: u64, phase: Phase, lat: f32, lon: f32) -> FlightSegment {
        FlightSegment {
            callsign: format!("FL{flight_id:04}"),
            aircraft_type: [b'A', b'3', b'2', b'0'],
            flight_id,
            profile_idx: 0,
            source_id: 0,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: 0,
            phase,
            flags: 0,
            start_lat: lat,
            start_lon: lon,
            start_alt_m: 5000.0,
            end_lat: lat + 0.001,
            end_lon: lon + 0.001,
            end_alt_m: 5100.0,
            speed_kt: 300.0,
            length_m: 200.0,
            agl_avg_m: 1000.0,
            start_elev_m: 0.0,
            end_elev_m: 0.0,
        }
    }

    #[test]
    fn round_trip_airborne_and_ground() {
        let tmp = tempfile::tempdir().unwrap();
        let segments_dir = tmp.path().join("segments");
        std::fs::create_dir_all(&segments_dir).unwrap();
        // One day with mixed phases at one location; cruise is dropped
        // by shuffle.
        let day_path = segments_dir.join("2025-01-21.arrow");
        write_segments(
            &day_path,
            &[
                seg(1, Phase::Airborne, 50.10, 14.26),
                seg(2, Phase::Ground, 50.10, 14.26),
                seg(3, Phase::Cruise, 50.10, 14.26),
            ],
        )
        .unwrap();

        let out_dir = tmp.path().join("segments_by_r4");
        shuffle_per_r4(&[day_path], &[], &out_dir, None).unwrap();

        // temp_shuffle must be cleaned up.
        assert!(!tmp.path().join("temp_shuffle").exists());
        // Single-window extract: no ga_n_days manifest (read_ga_n_days → 0).
        assert!(!out_dir.join("ga_n_days").exists());
        // Exactly one R4 directory should contain both shards.
        let r4_dirs: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(r4_dirs.len(), 1);
        let dir = r4_dirs[0].path();
        let airborne = read_segments(&dir.join("airborne.arrow")).unwrap();
        let ground = read_segments(&dir.join("ground.arrow")).unwrap();
        assert_eq!(airborne.len(), 1);
        assert_eq!(ground.len(), 1);
        assert_eq!(airborne[0].flight_id, 1);
        assert_eq!(ground[0].flight_id, 2);
        // Cruise must NOT appear in either shard.
    }

    #[test]
    fn scope_filters_out_of_scope_r4s() {
        let tmp = tempfile::tempdir().unwrap();
        let segments_dir = tmp.path().join("segments");
        std::fs::create_dir_all(&segments_dir).unwrap();
        let day_path = segments_dir.join("2025-01-21.arrow");
        write_segments(
            &day_path,
            &[
                seg(1, Phase::Airborne, 50.10, 14.26), // CZ
                seg(2, Phase::Airborne, 35.0, 139.0),  // Tokyo — out of scope
            ],
        )
        .unwrap();

        let out_dir = tmp.path().join("segments_by_r4");
        let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        shuffle_per_r4(&[day_path], &[], &out_dir, Some(&scope)).unwrap();

        // Only the CZ R4 must appear.
        let r4_dirs: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(r4_dirs.len(), 1);
        let airborne = read_segments(&r4_dirs[0].path().join("airborne.arrow")).unwrap();
        assert_eq!(airborne.len(), 1);
        assert_eq!(airborne[0].flight_id, 1);
    }

    #[test]
    fn empty_input_writes_no_shards() {
        let tmp = tempfile::tempdir().unwrap();
        let out_dir = tmp.path().join("segments_by_r4");
        shuffle_per_r4(&[], &[], &out_dir, None).unwrap();
        assert!(out_dir.exists(), "out_dir must be created");
        // No R4 shard dirs — only the n_days manifest, which records a
        // zero-day window for empty input.
        let subdirs = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(subdirs, 0, "no R4 shard dirs for empty input");
        assert_eq!(
            std::fs::read_to_string(out_dir.join("n_days")).unwrap(),
            "0"
        );
        assert!(!out_dir.join("ga_n_days").exists());
        assert!(!tmp.path().join("temp_shuffle").exists());
    }

    /// Hybrid merge with COLLIDING day stems — `2025-07-01` exists in
    /// both passes (first-of-month overlap). Both segments must reach
    /// the R4 shard: the former undiscriminated Pass-A
    /// temp path raced between passes and one silently vanished. Day
    /// counts come from the two input lists, never a combined `len()`.
    #[test]
    fn hybrid_colliding_day_stems_merge_and_write_dual_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let air_dir = tmp.path().join("segments");
        let ga_dir = tmp.path().join("ga_segments");
        std::fs::create_dir_all(&air_dir).unwrap();
        std::fs::create_dir_all(&ga_dir).unwrap();
        let air_day = air_dir.join("2025-07-01.arrow");
        let ga_day = ga_dir.join("2025-07-01.arrow");
        // Same location → same (phase, hash, day-stem) Pass-A bucket.
        write_segments(&air_day, &[seg(1, Phase::Airborne, 50.10, 14.26)]).unwrap();
        write_segments(&ga_day, &[seg(2, Phase::Airborne, 50.10, 14.26)]).unwrap();

        let out_dir = tmp.path().join("segments_by_r4");
        shuffle_per_r4(&[air_day], &[ga_day], &out_dir, None).unwrap();

        assert_eq!(
            std::fs::read_to_string(out_dir.join("n_days")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(out_dir.join("ga_n_days")).unwrap(),
            "1"
        );
        let r4_dirs: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(r4_dirs.len(), 1);
        let mut fids: Vec<u64> = read_segments(&r4_dirs[0].path().join("airborne.arrow"))
            .unwrap()
            .iter()
            .map(|s| s.flight_id)
            .collect();
        fids.sort_unstable();
        assert_eq!(
            fids,
            [1, 2],
            "both passes' segments must survive the stem collision"
        );
    }

    /// Duplicate day stems WITHIN one pass list would collide on one
    /// Pass-A temp path — refuse loudly instead of dropping segments.
    #[test]
    fn duplicate_day_stem_within_one_pass_bails() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let day_a = dir_a.join("2025-07-01.arrow");
        let day_b = dir_b.join("2025-07-01.arrow");
        write_segments(&day_a, &[seg(1, Phase::Airborne, 50.10, 14.26)]).unwrap();
        write_segments(&day_b, &[seg(2, Phase::Airborne, 50.10, 14.26)]).unwrap();
        let out_dir = tmp.path().join("segments_by_r4");
        let err = shuffle_per_r4(&[day_a, day_b], &[], &out_dir, None).unwrap_err();
        assert!(err.to_string().contains("duplicate day stem"), "{err}");
    }
}
