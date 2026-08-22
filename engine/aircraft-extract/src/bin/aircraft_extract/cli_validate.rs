//! Input-validation, path-listing, manifest-reading, and scope-parsing
//! helpers for the `aircraft-extract` bin. Sits beside the dispatcher
//! (`aircraft_extract.rs`) and `cli_runners.rs`; every guard here fails
//! loud BEFORE a stage runs so a typo / missing upstream artifact never
//! degrades into a silent no-op that wipes cached output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use aircraft_extract::scope::ScopeBbox;

/// Refuse to run the orchestrator at `--from-stage stage0` (default
/// OR explicit) when `--work-dir` already holds outputs from an
/// earlier run. Operators almost always want to REUSE that cache
/// (via `--from-stage stage1` / `shuffle` / `stage1-5` / ...); the
/// orchestrator would otherwise re-do Stage 0 + Stage 1 silently and
/// discard 1-3 hours of cached upstream work. The error message
/// shows the exact dirs that would be overwritten and offers the
/// two valid recovery paths: skip ahead with `--from-stage stageX`,
/// or `rm -rf $WORK_DIR` to start fresh.
pub fn bail_on_populated_work_dir(work_dir: &Path) -> Result<()> {
    let flights_dir = work_dir.join("flights");
    let segments_dir = work_dir.join("segments");
    let by_r4_dir = work_dir.join("segments_by_r4");
    let mut populated: Vec<(&str, &Path)> = Vec::new();
    for (name, dir) in [
        ("flights", flights_dir.as_path()),
        ("segments", segments_dir.as_path()),
        ("segments_by_r4", by_r4_dir.as_path()),
    ] {
        if dir.exists() && dir.is_dir() {
            let any = std::fs::read_dir(dir)
                .with_context(|| format!("read_dir {}", dir.display()))?
                .next()
                .is_some();
            if any {
                populated.push((name, dir));
            }
        }
    }
    if populated.is_empty() {
        return Ok(());
    }
    let listing = populated
        .iter()
        .map(|(n, p)| format!("  - {n}/  ({})", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(anyhow::anyhow!(
        "--work-dir {} already holds outputs from a previous run:\n\
         {listing}\n\n\
         The default `--from-stage stage0` would silently overwrite the cached \
         upstream work. Pick one:\n\
         (a) Reuse the cache — pass `--from-stage stage1` (or later) to skip ahead \
             to the stage whose code actually changed. The other six per-stage \
             subcommands also run standalone against the existing dirs.\n\
         (b) Wipe and rerun fresh — `rm -rf {work_dir}` first, then run-all.\n\
         The safety check fires only when both conditions hold: default `--from-stage` AND \
         a non-empty work_dir. Move the dir, archive it, or commit to (a)/(b) before retrying.",
        work_dir.display(),
        work_dir = work_dir.display(),
    ))
}

/// Fail loud when an operator runs a Stage 2 subcommand against a
/// missing input dir — a typo or a failed upstream shuffle would
/// otherwise become a silent no-op and leave stale `h3r4` outputs in
/// place (`list_r4_shards` swallows NotFound by design for RunAll).
pub fn require_input_dir_exists(flag: &str, dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Err(anyhow::anyhow!(
            "{flag} {} does not exist — did the upstream shuffle / Stage 1 run?",
            dir.display()
        ));
    }
    if !dir.is_dir() {
        return Err(anyhow::anyhow!(
            "{flag} {} is not a directory",
            dir.display()
        ));
    }
    Ok(())
}

/// Collect `segments/<day>.arrow` paths from a directory, sorted by
/// filename. Subcommand input for Stage 2B and Shuffle.
pub fn list_segments_day_paths(segments_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(segments_dir)
        .with_context(|| format!("read_dir {}", segments_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("arrow"))
        .collect();
    out.sort();
    Ok(out)
}

/// Union of [`list_segments_day_paths`] over repeatable `--segments-dir`
/// flags. Duplicate day stems across dirs are caught downstream by the
/// shuffle's stem-uniqueness bail (one Pass-A temp path per stem).
pub fn list_segments_day_paths_multi(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for dir in dirs {
        require_input_dir_exists("--segments-dir/--ga-segments-dir", dir)?;
        out.extend(list_segments_day_paths(dir)?);
    }
    out.sort();
    Ok(out)
}

/// Reads the shuffle day-count manifest (`<by_r4_dir>/n_days`). The shuffle
/// writes it from the segments it actually shuffled, so it is the true Lden
/// normalization window — the count of distinct extracted days present in
/// `segments_by_r4/` — and is invariant to a `--from-stage` re-run that
/// takes a stale `--days` from a shrunk ADS-B cache. A missing manifest
/// (legacy `segments_by_r4/` from before this field) fails loud.
pub fn read_window_n_days(by_r4_dir: &Path) -> Result<u16> {
    let path = by_r4_dir.join("n_days");
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "missing day-count manifest {} — re-run with `--from-stage shuffle` \
             to regenerate it from the segment files",
            path.display()
        )
    })?;
    let n = raw
        .trim()
        .parse::<u16>()
        .with_context(|| format!("parse n_days from {}", path.display()))?;
    if n == 0 {
        anyhow::bail!(
            "day-count manifest {} is 0 — re-run `--from-stage shuffle`",
            path.display()
        );
    }
    Ok(n)
}

/// Sibling of [`read_window_n_days`] for the GA-window manifest
/// (`<by_r4_dir>/ga_n_days`, written only by hybrid shuffles). A
/// missing file is 0 = plain single-window extract — all downstream
/// stamping then degenerates to today's behavior.
pub fn read_ga_n_days(by_r4_dir: &Path) -> Result<u16> {
    let path = by_r4_dir.join("ga_n_days");
    let raw = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        other => other.with_context(|| format!("read {}", path.display()))?,
    };
    raw.trim()
        .parse::<u16>()
        .with_context(|| format!("parse ga_n_days from {}", path.display()))
}

/// Shared `--scope-bbox` parser, identical surface across Stage2 + RunAll.
pub fn parse_scope(s: Option<&str>) -> Result<Option<ScopeBbox>> {
    s.map(ScopeBbox::parse)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--scope-bbox: {e}"))
}

/// Hard-fail when `--adsb-cache` points at a bbox/radius subset path
/// and the operator forgot `--scope-bbox`. The subset caches keep
/// whole daily traces for any in-scope flight; without a scope filter
/// Stage 2A/2B/2C would silently overwrite global R4 files with those
/// out-of-scope trajectories — the exact corruption that bit us when
/// the first Canary re-extract overwrote 95 Praha `841e3*` R4s.
pub fn require_scope_for_subset_cache(
    adsb_cache: &std::path::Path,
    scope: Option<&ScopeBbox>,
) -> Result<()> {
    if scope.is_some() {
        return Ok(());
    }
    let s = adsb_cache.to_string_lossy();
    if s.contains("/bbox/") || s.contains("/radius/") {
        return Err(anyhow::anyhow!(
            "--adsb-cache {} looks like a subset cache (path contains /bbox/ or \
             /radius/) but --scope-bbox is not set. Either pass --scope-bbox \
             min_lat,min_lon,max_lat,max_lon matching the subset filter, or \
             point --adsb-cache at the global archive root.",
            s
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_ga_n_days_missing_file_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_ga_n_days(tmp.path()).unwrap(), 0);
    }

    #[test]
    fn read_ga_n_days_parses_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ga_n_days"), "365\n").unwrap();
        assert_eq!(read_ga_n_days(tmp.path()).unwrap(), 365);
    }

    #[test]
    fn read_ga_n_days_rejects_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ga_n_days"), "not-a-number").unwrap();
        assert!(read_ga_n_days(tmp.path()).is_err());
    }
}
