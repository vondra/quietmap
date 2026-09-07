//! The on-disk obstacle-index cache's housekeeping — budget, eviction, volume
//! check — kept OUT of `structure_store.rs`, whose bytes are folded into the
//! cache key: a budget or logging change here must not rotate the world cache.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// `QM_OBSTACLE_INDEX_CACHE=0` turns the disk cache off — the A/B lever for
/// measuring what it is worth, and the bisection escape hatch, from ONE binary.
pub(crate) fn index_cache_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("QM_OBSTACLE_INDEX_CACHE").is_ok_and(|v| v == "0"))
}

/// Where cached indexes live: beside the other derived, year-independent
/// prepared artifacts (`prepared/dem`, `prepared/rasters`).
/// `QM_OBSTACLE_INDEX_DIR` moves them to another volume.
pub(crate) fn index_cache_root(data_dir: &Path) -> Option<PathBuf> {
    if !index_cache_enabled() {
        return None;
    }
    if let Ok(dir) = std::env::var("QM_OBSTACLE_INDEX_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(data_dir.join("obstacle-index"))
}

/// Disk budget for the cached indexes: the whole prepared world, so no popup
/// ever pays a cold build (Paris 19 s, London 12 s of builds on 2026-09-06).
/// 2026 world: 121 790 cells, 503 GB of `structures.arrow`, indexes at 1.08×
/// their Arrow (measured over 789 cached cells) ≈ 545 GB; the rest is headroom
/// for the files other checkouts on another engine version write into the same
/// root (a data refresh overwrites in place — the file name carries the engine
/// version, the header the data version). `obstacle-index-warm` fills it from
/// production; past it the least-recently-USED file is dropped.
const CACHE_BUDGET_BYTES: u64 = 640 << 30;

pub(crate) const CACHE_FILE_EXT: &str = "qoix";

/// A `.tmp` older than this is an orphan from a killed process, not a write in
/// flight — [`evict_to_budget`] reaps it. Generous by two orders: writing one
/// index is a few hundred MB of sequential IO.
const TMP_ORPHAN_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Drop least-recently-used cache files until `incoming` more bytes fit in
/// [`CACHE_BUDGET_BYTES`]. Best effort throughout — a cache that cannot be
/// pruned must not break a popup.
///
/// Safe to run while another process (or another checkout's server) has one of
/// these files mapped: unlinking keeps the inode alive until the last mapping
/// drops, and a rebuild lands on a NEW inode through the rename, so no live
/// query ever sees its index change underneath it.
///
/// Also the only reaper of ORPHANED `.tmp` files. [`store_cached_index`] removes
/// its own on a write error, but a process killed between `create` and `rename`
/// cannot — and those bytes were invisible to this budget (the filter took
/// `.qoix` alone), so a crash loop could fill the disk with files nothing would
/// ever look at again. Anything older than [`TMP_ORPHAN_AGE`] is not a write in
/// flight: one index is a few hundred MB, seconds of IO.
/// Cache bytes as last counted, and when. With the world cached the directory
/// holds ~120 k files, so a full count per store would cost more than the
/// store; between counts the total is advanced by the bytes written.
static CACHE_BYTES: Mutex<Option<(u64, std::time::Instant)>> = Mutex::new(None);
const CACHE_RECOUNT: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn evict_to_budget(root: &Path, incoming: u64) {
    let mut counted = CACHE_BYTES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((total, at)) = *counted {
        if at.elapsed() < CACHE_RECOUNT && total + incoming <= CACHE_BUDGET_BYTES {
            *counted = Some((total + incoming, at));
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let meta = e.metadata().ok()?;
            let mtime = meta.modified().ok()?;
            match path.extension()?.to_str()? {
                CACHE_FILE_EXT => Some((mtime, meta.len(), path)),
                "tmp" => {
                    // Old enough to be a corpse: unlink now and leave it out of
                    // the budget. A young one stays counted but untouched, so a
                    // concurrent writer's bytes still push the eviction.
                    if now.duration_since(mtime).is_ok_and(|d| d > TMP_ORPHAN_AGE) {
                        let _ = std::fs::remove_file(&path);
                        None
                    } else {
                        Some((mtime, meta.len(), path))
                    }
                }
                _ => None,
            }
        })
        .collect();
    // Young `.tmp` bytes COUNT (they are about to become cache) but are never
    // EVICTED: unlinking one would make its writer's rename land on a path this
    // loop had already reclaimed.
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total + incoming > CACHE_BUDGET_BYTES {
        files.retain(|(_, _, p)| p.extension().is_some_and(|x| x == CACHE_FILE_EXT));
        files.sort_by_key(|(mtime, _, _)| *mtime);
        let before = total;
        let mut evicted = 0usize;
        for (_, len, path) in files {
            if total + incoming <= CACHE_BUDGET_BYTES {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                evicted += 1;
            }
        }
        // Every eviction is announced: a shrinking world cache must be
        // attributable (2026-09-07: 119 k files vanished with no trace).
        eprintln!(
            "index_cache: evicted {evicted} cached indexes ({:.1} GB) from {} to keep {:.0} GB under the {:.0} GB budget",
            (before - total) as f64 / 1e9,
            root.display(),
            (total + incoming) as f64 / 1e9,
            CACHE_BUDGET_BYTES as f64 / 1e9,
        );
    }
    *counted = Some((total + incoming, std::time::Instant::now()));
}

/// Refuse a world sweep onto a volume that cannot hold it: the budget is
/// enforced against itself, never against free space, so a cache root left
/// on the nearly full data volume would be filled to the brim. `Ok` carries
/// (cached bytes, free bytes) for the sweep's summary.
pub fn check_index_cache_volume(data_dir: &Path) -> Result<(u64, u64), String> {
    let root = index_cache_root(data_dir)
        .ok_or("the index cache is disabled (QM_OBSTACLE_INDEX_CACHE=0)")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let cached: u64 = std::fs::read_dir(&root)
        .map_err(|e| format!("read {}: {e}", root.display()))?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == CACHE_FILE_EXT))
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .sum();
    let c_root = std::ffi::CString::new(root.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("{}: {e}", root.display()))?;
    // SAFETY: `st` is a plain C struct the call fills in; the path is a valid
    // NUL-terminated string for the duration of the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_root.as_ptr(), &mut st) } != 0 {
        return Err(format!(
            "statvfs {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        ));
    }
    let free = st.f_bavail as u64 * st.f_frsize as u64;
    if cached + free < CACHE_BUDGET_BYTES {
        return Err(format!(
            "{} holds {:.0} GB with {:.0} GB free on its volume; the world needs the \
             {:.0} GB budget — point the cache root at a larger volume first",
            root.display(),
            cached as f64 / 1e9,
            free as f64 / 1e9,
            CACHE_BUDGET_BYTES as f64 / 1e9,
        ));
    }
    Ok((cached, free))
}
