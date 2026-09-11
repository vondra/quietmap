//! Remove unreferenced PMTiles archives after their retirement grace.
//!
//! The packer merge head and every current environment pointer retain their archives.
//! Historical manifests never retain files. Invalid current manifests abort the whole sweep.
//! Pointer writers renew retired archives' mtimes before switching, giving open browser tabs
//! an hour to refresh. The publication timer sweeps even when no repaint is running.
//! The shared `.pack.lock` serializes packing, production transitions and this sweep.
//! Dry-run is the default; `--delete` applies the sweep.

use std::collections::HashSet;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};

use tile_painter::tile_store::manifest::manifest_files;

const DEFAULT_MIN_AGE_SECS: u64 = 3600;

#[derive(Debug)]
struct GcReport {
    checked: usize,
    kept: usize,
    candidates: Vec<String>,
    deleted: Vec<String>,
}

fn main() -> Result<()> {
    let mut positional: Vec<String> = Vec::new();
    let mut delete = false;
    let mut min_age_secs = DEFAULT_MIN_AGE_SECS;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--delete" => delete = true,
            "--dry-run" => delete = false, // explicit no-op spelling of the default
            "--min-age-secs" => {
                min_age_secs = args
                    .next()
                    .context("--min-age-secs needs a value")?
                    .parse()
                    .context("--min-age-secs must be a non-negative integer")?
            }
            _ => positional.push(a),
        }
    }
    let [out_dir]: [String; 1] = positional.try_into().map_err(|_| {
        anyhow::anyhow!("usage: tile-store-gc <pmtiles-dir> [--delete] [--min-age-secs N]")
    })?;
    let out_dir = PathBuf::from(out_dir);

    let report = run_gc(&out_dir, delete, Duration::from_secs(min_age_secs))?;

    if delete {
        for name in &report.deleted {
            println!("tile-store-gc: DELETED {name}");
        }
        for name in report
            .candidates
            .iter()
            .filter(|n| !report.deleted.contains(*n))
        {
            eprintln!("tile-store-gc: rm {name} failed (non-fatal, next run retries)");
        }
    } else {
        for name in &report.candidates {
            println!("tile-store-gc: would delete {name} (dry-run — pass --delete to remove)");
        }
    }
    println!(
        "tile-store-gc: {} archive(s) checked, {} kept (referenced), {} candidate(s) {}",
        report.checked,
        report.kept,
        report.candidates.len(),
        if delete { "deleted" } else { "found (dry-run)" }
    );
    Ok(())
}

/// Validate all current roots before deleting anything, under the packer/GC lock.
fn run_gc(out_dir: &Path, delete: bool, min_age: Duration) -> Result<GcReport> {
    let _lock = {
        let lock = fs::File::create(out_dir.join(".pack.lock"))
            .with_context(|| format!("create {}/.pack.lock", out_dir.display()))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("flock .pack.lock: {}", std::io::Error::last_os_error());
        }
        lock
    };

    let archive_names: Vec<String> = fs::read_dir(out_dir)
        .with_context(|| format!("read_dir {}", out_dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|name| name.ends_with(".pmtiles"))
        .collect();

    if archive_names.is_empty() {
        return Ok(GcReport {
            checked: 0,
            kept: 0,
            candidates: Vec::new(),
            deleted: Vec::new(),
        });
    }

    // (a) The packer's own merge head. REQUIRED the moment any archive exists on disk — a
    // pack must have run to produce that archive, and it always writes current.json last.
    // Its absence here (or corruption) means the tool cannot prove ANYTHING is safe to
    // delete: abort loudly rather than guess.
    let merge_head = out_dir.join("current.json");
    if !merge_head.exists() {
        bail!(
            "{} archive(s) exist under {} but there is no current.json (packer merge-head) — \
             refusing to guess what is live",
            archive_names.len(),
            out_dir.display()
        );
    }
    let mut keep: HashSet<String> = HashSet::new();
    keep.extend(read_manifest_files(
        &merge_head,
        "packer merge-head current.json",
    )?);

    // (b) Every per-environment pointer actually present. DISCOVERED, not hardcoded — see
    // module doc. Missing is fine (an env not seeded/promoted yet); present-but-corrupt
    // aborts the whole run.
    for entry in fs::read_dir(out_dir).with_context(|| format!("read_dir {}", out_dir.display()))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(env) = name
            .strip_prefix("current.")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if env.is_empty() {
            continue;
        }
        keep.extend(read_manifest_files(
            &out_dir.join(&name),
            &format!("environment pointer {name}"),
        )?);
    }

    let now = SystemTime::now();
    let mut candidates = Vec::new();
    for name in &archive_names {
        if keep.contains(name) {
            continue;
        }
        let path = out_dir.join(name);
        let age = match fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(mtime) => now.duration_since(mtime).unwrap_or_default(),
            Err(e) => {
                eprintln!("tile-store-gc: stat {name} failed (skipping): {e}");
                continue;
            }
        };
        if age < min_age {
            continue; // too young — a browser may still hold a snapshot naming it
        }
        candidates.push(name.clone());
    }

    let mut deleted = Vec::new();
    if delete {
        for name in &candidates {
            match fs::remove_file(out_dir.join(name)) {
                Ok(()) => deleted.push(name.clone()),
                Err(e) => eprintln!("tile-store-gc: rm {name} failed (non-fatal): {e}"),
            }
        }
    }

    Ok(GcReport {
        checked: archive_names.len(),
        kept: archive_names.len() - candidates.len(),
        candidates,
        deleted,
    })
}

fn read_manifest_files(path: &Path, context: &str) -> Result<Vec<String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read {context} ({})", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parse {context} ({})", path.display()))?;
    manifest_files(&json, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn manifest(build: &str, files: &[(&str, &str)]) -> serde_json::Value {
        let mut layers = serde_json::Map::new();
        for (layer, file) in files {
            layers.insert(layer.to_string(), serde_json::json!({"file": file}));
        }
        serde_json::json!({"build": build, "layers": layers})
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }

    /// Backdate a file's mtime so age-grace tests don't need a real 1h sleep. Uses
    /// `utimensat` directly (no `filetime` crate — this project already depends on `libc`).
    fn set_mtime_seconds_ago(path: &Path, seconds_ago: u64) {
        let epoch_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - seconds_ago as i64;
        let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let spec = libc::timespec {
            tv_sec: epoch_secs,
            tv_nsec: 0,
        };
        let rc =
            unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), [spec, spec].as_ptr(), 0) };
        assert_eq!(
            rc,
            0,
            "utimensat failed: {}",
            std::io::Error::last_os_error()
        );
    }

    fn touch_old(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"x").unwrap();
        set_mtime_seconds_ago(&dir.join(name), 100_000);
    }

    #[test]
    fn dry_run_reports_candidates_but_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles");
        write_json(&dir.join("current.json"), &manifest("b1", &[]));
        write_json(
            &dir.join("history/prod/old.json"),
            &manifest("b1", &[("road", "road.b1.pmtiles")]),
        );

        let report = run_gc(dir, false, Duration::from_secs(0)).unwrap();
        assert_eq!(report.candidates, vec!["road.b1.pmtiles".to_string()]);
        assert!(report.deleted.is_empty());
        assert!(
            dir.join("road.b1.pmtiles").exists(),
            "dry-run must not delete"
        );
    }

    #[test]
    fn merge_head_protects_its_own_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles");
        write_json(
            &dir.join("current.json"),
            &manifest("b1", &[("road", "road.b1.pmtiles")]),
        );

        let report = run_gc(dir, true, Duration::from_secs(0)).unwrap();
        assert!(report.candidates.is_empty(), "merge head keeps it alive");
        assert!(dir.join("road.b1.pmtiles").exists());
    }

    #[test]
    fn environment_pointer_protects_an_archive_the_merge_head_no_longer_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles"); // superseded at the merge head, still pinned by prod
        touch_old(dir, "road.b2.pmtiles");
        write_json(
            &dir.join("current.json"),
            &manifest("b2", &[("road", "road.b2.pmtiles")]),
        );
        write_json(
            &dir.join("current.prod.json"),
            &manifest("b1", &[("road", "road.b1.pmtiles")]),
        );

        let report = run_gc(dir, true, Duration::from_secs(0)).unwrap();
        assert!(
            report.candidates.is_empty(),
            "prod's pin must protect its archive even though the merge head moved on"
        );
        assert!(dir.join("road.b1.pmtiles").exists());
        assert!(dir.join("road.b2.pmtiles").exists());
    }

    #[test]
    fn age_grace_protects_a_young_unreferenced_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("road.b1.pmtiles"), b"x").unwrap(); // fresh mtime, unreferenced
        write_json(&dir.join("current.json"), &manifest("b2", &[]));

        let report = run_gc(dir, true, Duration::from_secs(3600)).unwrap();
        assert!(report.candidates.is_empty(), "too young to sweep yet");
        assert!(dir.join("road.b1.pmtiles").exists());
    }

    #[test]
    fn fail_closed_on_a_corrupt_environment_pointer_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles");
        write_json(&dir.join("current.json"), &manifest("b1", &[]));
        fs::write(dir.join("current.dev1.json"), b"{ not json").unwrap();

        let err = run_gc(dir, true, Duration::from_secs(0)).unwrap_err();
        assert!(err.to_string().contains("dev1") || format!("{err:#}").contains("dev1"));
        assert!(
            dir.join("road.b1.pmtiles").exists(),
            "a corrupt manifest must abort before any delete"
        );
    }

    #[test]
    fn fail_closed_when_archives_exist_but_the_merge_head_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles");

        let err = run_gc(dir, true, Duration::from_secs(0)).unwrap_err();
        assert!(format!("{err:#}").contains("current.json"));
        assert!(dir.join("road.b1.pmtiles").exists());
    }

    #[test]
    fn empty_out_dir_is_a_clean_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let report = run_gc(tmp.path(), true, Duration::from_secs(0)).unwrap();
        assert_eq!(report.checked, 0);
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn actually_deletes_when_delete_flag_is_set() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch_old(dir, "road.b1.pmtiles");
        write_json(&dir.join("current.json"), &manifest("b2", &[]));

        let report = run_gc(dir, true, Duration::from_secs(0)).unwrap();
        assert_eq!(report.deleted, vec!["road.b1.pmtiles".to_string()]);
        assert!(!dir.join("road.b1.pmtiles").exists());
    }
}
