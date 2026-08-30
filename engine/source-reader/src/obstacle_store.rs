//! Vector obstacle loading for the popup (geodata-v2 1.4).
//!
//! Each query assembles an
//! [`ObstacleSet`] from PER-CELL [`ObstacleIndex`]es covering the query
//! cell's `grid_disk(1)` — the halo the ingest contract requires
//! (centroid-assigned footprints; `scripts/obstacles/ingest-overture-obstacles.py`).
//!
//! Two hard rules:
//! - **Bounded cost.** Per-cell indexes are built ONCE per process and
//!   LRU-cached (`CELL_CACHE_CAP`); a query only Arc-clones ≤7 of them.
//!   The naive per-query rebuild measured 448 MB RSS / 0.47 s per popup.
//! - **All-or-raster.** Any shard read/parse error, and by default any
//!   MISSING ring-1 cell, aborts the whole load → the query keeps the
//!   legacy raster path (loudly, via stderr). A partial index would delete
//!   raster buildings where coverage is absent — silent under-screening.
//!   `QM_OBSTACLES_ALLOW_PARTIAL=1` relaxes ONLY the missing-cell rule for
//!   dev A/B runs inside a partially staged world; shard errors always abort.
//!   EXCEPTION (ingested-empty proof): a shard-less cell whose every
//!   overlapped 1-degree tile is listed in the world ingest manifest
//!   (`.ingested-tiles`) was provably swept and contributed zero footprints
//!   — it is EMPTY, not missing, and vector mode proceeds without it
//!   (`propagation::obstacle_ingest_coverage`).
//!
//! Shard roots, per cell, first hit wins: the PROMOTED tree
//! (`…/prepared/{year}/h3r4/<cell>/obstacles*.arrow`, post-Wave-1) and the
//! ENRICHMENT staging tree the ingest writes today. `QM_OBSTACLES_DIR`
//! overrides both (tests).
//!
//! Built indexes are also kept ON DISK (`noise_compute::propagation::obstacle_index_file`)
//! and mapped back on the next cold start — a São Paulo popup indexes 40 M
//! edges from ~1 GB of Arrow, which cost ~6 s of the FIRST click and was then
//! thrown away with the process. The cached file is the in-memory layout, so a
//! reload is an `mmap` plus a header check and the kernel faults in only the
//! grid cells the rays walk.
//!
//! **Both caches key on [`cell_data_ver`], the full identity of the index** —
//! never on the cell. The memo in front of the disk cache once keyed on the
//! cell alone and so answered a query about one shard root with an index built
//! from another (2026-08-05); the popup is the project's acoustic reference, and
//! a cache that returns the answer to a different question is worse than no
//! cache. The ring and `QM_OBSTACLES_ALLOW_PARTIAL` are deliberately NOT in that
//! key: they decide which cells are assembled into a set, per query, in
//! [`load_obstacle_set`] — a cache entry is one cell's index built from that
//! cell's own shards, so no dev A/B run can leave a file a strict query would
//! use (`missing_ring_cell_falls_back_unless_partial_allowed` asserts it).

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use arrow::array::{Array, BinaryArray, Float32Array, Float64Array, UInt8Array};
use arrow::ipc::reader::FileReader;
use h3o::{CellIndex, LatLng, Resolution};
use noise_compute::envelope::{effective_envelope_class, EnvelopeClass};
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::propagation::obstacle_index_file::{fnv1a64, IndexBlob, BUILDER_CODE_VER};

/// Per-cell index cache capacity. A dense metro cell's index runs to low
/// hundreds of MB; popups cluster spatially, so a small LRU covers the
/// active area while bounding worst-case RSS.
const CELL_CACHE_CAP: usize = 8;

/// Everything that decides a cached index's BYTES: the engine's builder and
/// grid (`BUILDER_CODE_VER`) folded with THIS file, which owns the loader's own
/// decisions — the obstacle id ordering, the shard order, and which rows are
/// offered to the height cap. Editing either side rotates the version and every file written by the
/// old code is refused, exactly as `scripts/layer-codever.py` re-stales tiles on
/// a source change. Over-invalidating costs a rebuild; under-invalidating puts a
/// silently wrong screen in the map.
///
/// The low-profile cap needs no fold of its own: the rule lives in
/// `noise_compute::low_profile`, which [`BUILDER_CODE_VER`] hashes — so a change
/// to its class list, its match geometry or its cap rotates this version without
/// anyone naming the constants here.
const CACHE_CODE_VER: u64 = fnv1a64(BUILDER_CODE_VER, include_bytes!("obstacle_store.rs"));

/// Disk budget for the cached indexes. One dense metro cell is a few hundred
/// MB, so this holds tens of cities' worth — far more than a popup session
/// visits — while keeping a nearly-full data volume out of danger. Past it the
/// least-recently-USED file is dropped and its next cold start pays one rebuild.
const CACHE_BUDGET_BYTES: u64 = 24 << 30;

const CACHE_FILE_EXT: &str = "qoix";

/// A `.tmp` older than this is an orphan from a killed process, not a write in
/// flight — [`evict_to_budget`] reaps it. Generous by two orders: writing one
/// index is a few hundred MB of sequential IO.
const TMP_ORPHAN_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Mapped cache file. The mapping's address and contents are fixed for its
/// life, which is what [`IndexBlob`] requires.
struct MappedIndexFile(memmap2::Mmap);

// SAFETY: `Mmap` derefs to a fixed address/length for its whole life and this
// wrapper never exposes a `&mut`, so every `as_bytes` returns the same
// immutable bytes — the `IndexBlob` contract.
unsafe impl IndexBlob for MappedIndexFile {
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// `QM_OBSTACLE_INDEX_CACHE=0` turns the disk cache off — the A/B lever for
/// measuring what it is worth, and the bisection escape hatch, from ONE binary.
fn index_cache_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("QM_OBSTACLE_INDEX_CACHE").is_ok_and(|v| v == "0"))
}

/// Where cached indexes live: beside the other derived, year-independent
/// prepared artifacts (`prepared/dem`, `prepared/rasters`, `h3r4-admin.bin`).
/// `QM_OBSTACLE_INDEX_DIR` moves them to another volume.
fn index_cache_root(data_dir: &Path) -> Option<PathBuf> {
    if !index_cache_enabled() {
        return None;
    }
    if let Ok(dir) = std::env::var("QM_OBSTACLE_INDEX_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(data_dir.join("obstacle-index"))
}

/// The FULL identity of one cell's index — everything that decides its bytes,
/// in one u64. Both caches (the process memo and the file on disk) key on it,
/// and nothing may be served under a key that does not carry all of:
///
/// * [`CACHE_CODE_VER`] — the builder, the grid, this loader's own rules;
/// * the CELL, whose centre is the index's metric origin (and which is the
///   only thing the file name would otherwise bind);
/// * every shard the index is built from, plus the `buildings.arrow` the
///   low-profile cap reads, as (path, length, mtime) — the path because two
///   shard ROOTS (a staging A/B, `QM_OBSTACLES_DIR`, a moved mount) hold
///   different obstacles for the same cell.
///
/// That list is closed by construction: `build_cell_index` reads its cell, its
/// shards and that one arrow, and nothing else — no env, no clock, no map
/// iteration order (`ObstacleIndex::build` is a Vec walk). Whatever a future
/// edit adds to it lands in THIS file, and this file's content is already in
/// `CACHE_CODE_VER`, so an unfingerprinted input cannot be introduced without
/// also rotating the version.
///
/// (length, mtime) rather than a content hash is the shape of
/// `world-stamps.py`'s `_data_ver`, which decides tile staleness from the same
/// arrows' mtimes; re-hashing a gigabyte per click would cost more than the
/// rebuild it guards.
///
/// `None` ⇒ some input's metadata is unreadable, so nothing may be cached at
/// all: an index whose provenance cannot be pinned must never outlive the
/// query, let alone the process.
fn cell_data_ver(
    cell: CellIndex,
    shards: &[PathBuf],
    buildings_arrow: Option<&Path>,
) -> Option<u64> {
    let mut h = fnv1a64(CACHE_CODE_VER, b"obstacle-index-inputs-v2");
    h = fnv1a64(h, &u64::from(cell).to_le_bytes());
    let mut fold = |path: &Path, present_marker: u8| -> Option<()> {
        h = fnv1a64(h, path.as_os_str().as_encoded_bytes());
        h = fnv1a64(h, &[present_marker]);
        if present_marker == 0 {
            return Some(());
        }
        let meta = std::fs::metadata(path).ok()?;
        h = fnv1a64(h, &meta.len().to_le_bytes());
        let mtime = meta.modified().ok()?;
        let since_epoch = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .ok()?;
        h = fnv1a64(h, &since_epoch.as_nanos().to_le_bytes());
        Some(())
    };
    for p in shards {
        fold(p, 1)?;
    }
    // A missing buildings.arrow means "no capping" — a DIFFERENT index from the
    // same shards with one present, so absence has to hash differently.
    if let Some(p) = buildings_arrow {
        let marker = u8::from(p.exists());
        fold(p, marker)?;
    }
    Some(h)
}

/// `<cell>.<code_ver>.qoix`. The builder version is in the NAME, not just the
/// header, because prod and dev1-3 share one `prepared/` node: two checkouts on
/// different engine versions would otherwise fight over one path, each deleting
/// and rebuilding the other's file forever. Superseded versions are ordinary
/// cache files and age out through the LRU budget.
fn cache_file_path(root: &Path, cell: CellIndex) -> PathBuf {
    root.join(format!("{cell}.{CACHE_CODE_VER:016x}.{CACHE_FILE_EXT}"))
}

/// Map a cached index, or `None` for any reason at all — absent, stale,
/// truncated, unreadable. Every `None` simply means "rebuild".
fn load_cached_index(path: &Path, data_ver: u64) -> Option<ObstacleIndex> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: the store is written atomically (tmp + rename) and never mutated
    // in place, so no other writer can change these bytes under the mapping.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let blob: Arc<dyn IndexBlob> = Arc::new(MappedIndexFile(mmap));
    match ObstacleIndex::from_blob(blob, CACHE_CODE_VER, data_ver) {
        Ok(idx) => {
            // LRU by USE, not by write: without this the city visited every day
            // is evicted before one indexed once and never opened again.
            let now = std::time::SystemTime::now();
            let _ = file.set_times(std::fs::FileTimes::new().set_modified(now));
            Some(idx)
        }
        Err(e) => {
            eprintln!("obstacle_store: ignoring cached {}: {e}", path.display());
            let _ = std::fs::remove_file(path);
            None
        }
    }
}

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
fn evict_to_budget(root: &Path, incoming: u64) {
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
    if total + incoming <= CACHE_BUDGET_BYTES {
        return;
    }
    files.retain(|(_, _, p)| p.extension().is_some_and(|x| x == CACHE_FILE_EXT));
    files.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in files {
        if total + incoming <= CACHE_BUDGET_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// Persist a freshly built index. Failures are reported and swallowed: the
/// cache is an accelerator, never a dependency.
fn store_cached_index(root: &Path, cell: CellIndex, index: &ObstacleIndex, data_ver: u64) {
    let parts = index.file_parts(CACHE_CODE_VER, data_ver);
    let total = parts.total_len() as u64;
    if let Err(e) = std::fs::create_dir_all(root) {
        eprintln!("obstacle_store: no index cache at {}: {e}", root.display());
        return;
    }
    evict_to_budget(root, total);
    let final_path = cache_file_path(root, cell);
    // Same-directory tmp + rename: a reader either maps the whole previous
    // file or the whole new one, never a half-written index. Two NAPI worker
    // threads can miss the process LRU on the SAME cell at the same time, so
    // the temp name carries a per-write sequence — sharing one `<pid>.tmp`
    // would let them interleave into a file that then passes the header check.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = root.join(format!("{cell}.{}.{seq}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&parts.header)?;
        for section in &parts.sections {
            f.write_all(section)?;
        }
        f.flush()?;
        drop(f);
        std::fs::rename(&tmp, &final_path)
    };
    if let Err(e) = write() {
        eprintln!(
            "obstacle_store: could not cache index {}: {e}",
            final_path.display()
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Process-local memo of built indexes, keyed on the SAME identity the disk
/// file carries ([`cell_data_ver`]) — not on the cell.
///
/// Keying it on the cell alone was a live defect (2026-08-05): the second
/// query for a cell got the first query's index no matter which shard root it
/// asked about, so an A/B run against a second staging tree, or a
/// `QM_OBSTACLES_DIR` switch, screened the popup against obstacles that are not
/// there. The disk cache never had this hole — its header carries the
/// fingerprint — which is exactly why the memo in front of it had to grow one.
struct CellCache {
    /// (cell, identity) → (index, LRU stamp). Failed builds are NOT cached —
    /// transient IO must stay retryable; missing cells stay a per-query
    /// decision.
    map: HashMap<(CellIndex, u64), (Arc<ObstacleIndex>, u64)>,
    stamp: u64,
}

static CELL_CACHE: OnceLock<Mutex<CellCache>> = OnceLock::new();

fn memo() -> &'static Mutex<CellCache> {
    CELL_CACHE.get_or_init(|| {
        Mutex::new(CellCache {
            map: HashMap::new(),
            stamp: 0,
        })
    })
}

fn memo_get(cell: CellIndex, ver: u64) -> Option<Arc<ObstacleIndex>> {
    let mut c = memo().lock().unwrap_or_else(|e| e.into_inner());
    c.stamp += 1;
    let stamp = c.stamp;
    let (idx, touched) = c.map.get_mut(&(cell, ver))?;
    *touched = stamp;
    Some(Arc::clone(idx))
}

fn memo_put(cell: CellIndex, ver: u64, idx: &Arc<ObstacleIndex>) {
    let mut c = memo().lock().unwrap_or_else(|e| e.into_inner());
    c.stamp += 1;
    let stamp = c.stamp;
    if c.map.len() >= CELL_CACHE_CAP {
        if let Some((&evict, _)) = c.map.iter().min_by_key(|(_, (_, t))| *t) {
            c.map.remove(&evict);
        }
    }
    c.map.insert((cell, ver), (Arc::clone(idx), stamp));
}

fn staging_root(data_dir: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("QM_OBSTACLES_DIR") {
        return PathBuf::from(dir);
    }
    data_dir
        .parent()
        .map(|d| d.join("enrichment/global/overture-obstacles/h3r4"))
        .unwrap_or_else(|| PathBuf::from("data/enrichment/global/overture-obstacles/h3r4"))
}

/// The cell's shard directory: promoted tree first (post-Wave-1 layout),
/// staging second. `Ok(None)` when the cell is nowhere ingested; `Err` on
/// any listing failure other than absence.
fn cell_dir(
    h3r4_dir: Option<&Path>,
    data_dir: &Path,
    cell: CellIndex,
) -> Result<Option<PathBuf>, String> {
    if std::env::var("QM_OBSTACLES_DIR").is_err() {
        if let Some(h3r4) = h3r4_dir {
            let promoted = h3r4.join(cell.to_string());
            if !shard_paths(&promoted)?.is_empty() {
                return Ok(Some(promoted));
            }
        }
    }
    let staged = staging_root(data_dir).join(cell.to_string());
    Ok((!shard_paths(&staged)?.is_empty()).then_some(staged))
}

/// Sorted shard listing — deterministic iteration keeps the query-local
/// obstacle ordinals stable for one on-disk state. A missing directory is a
/// legitimate "not ingested" (`Ok(empty)`); any OTHER I/O failure is an
/// error — a permission or disk fault must not read as "cell missing" and
/// admit an incomplete index under partial mode.
fn shard_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read_dir {}: {e}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir entry in {}: {e}", dir.display()))?;
        let p = entry.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("obstacles") && n.ends_with(".arrow"))
        {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// Assemble the query's [`ObstacleSet`]. `None` ⇒ caller keeps the raster
/// path (not ingested here, incomplete ring coverage under the strict
/// default, or any shard error).
pub fn load_obstacle_set(
    h3r4_dir: Option<&Path>,
    data_dir: &Path,
    lat: f64,
    lon: f64,
) -> Result<ObstacleSet, String> {
    load_obstacle_set_with_logging(h3r4_dir, data_dir, lat, lon)
}

/// Assemble an obstacle set without logging expected raster fallback paths.
/// The building-height hover endpoint calls this variant because a moving
/// pointer can otherwise print one fallback line per debounced request.
fn load_obstacle_set_with_logging(
    h3r4_dir: Option<&Path>,
    data_dir: &Path,
    lat: f64,
    lon: f64,
) -> Result<ObstacleSet, String> {
    let allow_partial = std::env::var("QM_OBSTACLES_ALLOW_PARTIAL").is_ok_and(|v| v == "1");
    let cell = LatLng::new(lat, lon)
        .map_err(|e| format!("obstacle_store: {lat},{lon} is not a point on earth: {e}"))?
        .to_cell(Resolution::Four);
    let manifest = ingest_manifest(h3r4_dir, data_dir);
    let mut indexes = Vec::new();
    for c in cell.grid_disk::<Vec<_>>(1) {
        let dir = match cell_dir(h3r4_dir, data_dir, c) {
            Err(e) => return Err(format!("obstacle_store: {e}")),
            Ok(Some(dir)) => dir,
            Ok(None) => {
                if manifest.is_some_and(|m| m.covers_cell(c)) {
                    // INGESTED-EMPTY, proven by the world ingest manifest: the
                    // Overture sweep reached this cell and it contributed no
                    // footprints. Empty is an answer; missing is not.
                    continue;
                }
                if c == cell {
                    return Err(format!(
                        "obstacle_store: cell {c} has no obstacle shard and the ingest \
                         manifest does not prove it empty — buildings are vector-only, \
                         so this query cannot be answered"
                    ));
                }
                if allow_partial {
                    // Never silent — the same rule as the pipeline loader.
                    eprintln!(
                        "obstacle_store: QM_OBSTACLES_ALLOW_PARTIAL: answering without \
                         ring cell {c} — incomplete screening, dev A/B only"
                    );
                    continue;
                }
                return Err(format!(
                    "obstacle_store: ring cell {c} not ingested (set \
                     QM_OBSTACLES_ALLOW_PARTIAL=1 only for dev A/B)"
                ));
            }
        };
        let buildings_arrow = h3r4_dir.map(|h| h.join(c.to_string()).join("buildings.arrow"));
        match cell_index(c, &dir, buildings_arrow.as_deref(), data_dir) {
            Ok(idx) => indexes.push(idx),
            Err(e) => return Err(format!("obstacle_store: {e}")),
        }
    }
    // Zero edges is a legitimate answer, whether it came from manifest-proven
    // empty cells or from a staged shard that indexed no footprint. A shard
    // that exists HAS been asked and HAS answered; treating its emptiness as a
    // fault would take whole countries silent the moment an Overture release
    // rejects their heights (raised in review, 2026-08-30).
    Ok(ObstacleSet { indexes })
}

/// The world-ingest manifest next to the staging tree, when present (the
/// tile-painter loader's twin). Absent ⇒ coverage unknown ⇒ the strict
/// all-or-raster fallback keeps today's behavior.
fn ingest_manifest(
    h3r4_dir: Option<&Path>,
    data_dir: &Path,
) -> Option<&'static noise_compute::propagation::obstacle_ingest_coverage::IngestManifest> {
    // The tile-painter staging_root derivation (h3r4 ancestors) and the
    // popup's (data_dir parent) meet at the same tree; prefer whichever
    // resolves with the manifest file actually present.
    let mut candidates = Vec::new();
    if let Some(h3r4) = h3r4_dir {
        if std::env::var("QM_OBSTACLES_DIR").is_err() {
            candidates.extend(
                noise_compute::propagation::obstacle_ingest_coverage::ingested_tiles_paths(h3r4),
            );
        }
    }
    candidates.push(
        staging_root(data_dir)
            .parent()
            .map(|p| p.join(".ingested-tiles"))
            .unwrap_or_else(|| std::path::PathBuf::from(".ingested-tiles")),
    );
    for path in candidates {
        if let Some(m) =
            noise_compute::propagation::obstacle_ingest_coverage::IngestManifest::load_cached(&path)
        {
            return Some(m);
        }
    }
    None
}

/// Tallest structure the height probe will report (m) — comfortably above the
/// tallest building on Earth (828 m), so the bisection's upper bracket is
/// never the answer in practice.
const MAX_PROBE_HEIGHT_M: f32 = 1_000.0;
/// Height-probe resolution (m). Store heights are metre-scale (mapped values,
/// the 3 m low-profile cap, the 8 m default), so 5 cm is far below anything a
/// popup would display.
const HEIGHT_PROBE_RESOLUTION_M: f32 = 0.05;

/// Height of the tallest vector footprint containing the receiver, regardless
/// of envelope class. This is CNOSSOS fix-pack Fix 4's popup half and the
/// lockstep twin of tile-painter's `bake_tile_interior_mask`: change one,
/// change both so popup and heatmap keep shared inside/hole/overlap semantics.
/// The indoor calculation uses [`point_inside_enclosed`].
///
/// DISPLAY ONLY: the popup keeps computing and reporting the same dB values;
/// this function only labels them. What an indoor receiver should report
/// (facade exposure rather than interior noise) is a separate product decision.
///
/// Runs on the already-loaded query set — zero extra I/O. The height comes out
/// of the containment test itself: `ObstacleIndex::contains_built(…, min_h)`
/// answers "inside a footprint TALLER than `min_h`", which is monotone in
/// `min_h`, so the tallest containing footprint is the threshold where it
/// flips — ~15 in-memory probes. That keeps the exact same polygon test (and
/// its hole/overlap semantics) as the heatmap mask and enclosure probe;
/// a height-returning containment query on `ObstacleIndex` itself would be
/// the cheaper shape, and is the named follow-up for whoever next opens
/// `propagation::obstacle_index`.
pub fn point_inside_obstacle(set: &ObstacleSet, lat: f64, lon: f64) -> Option<f32> {
    let mut seen: Vec<(u32, u32, f32)> = Vec::new();
    let mut inside = |min_h: f32| {
        set.indexes
            .iter()
            .any(|i| i.contains_built(lat, lon, min_h, &mut seen))
    };
    // `min_height_m = 0` admits every indexed footprint (the builder already
    // drops height ≤ 0) — "inside any obstacle polygon", the mask's rule.
    if !inside(0.0) {
        return None;
    }
    let (mut lo, mut hi) = (0.0f32, MAX_PROBE_HEIGHT_M);
    while hi - lo > HEIGHT_PROBE_RESOLUTION_M {
        let mid = 0.5 * (lo + hi);
        if inside(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(0.5 * (lo + hi))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnclosedEnvelopeWinner {
    pub stored_class: EnvelopeClass,
    pub effective_class: EnvelopeClass,
    pub height_m: f32,
}

/// Select the display-envelope winner using the painter's exact order: only
/// enclosed footprints participate, then tallest height, lower index ordinal,
/// and lower footprint ordinal win. `stored_class` is the source
/// classification; `effective_class` is the paint/popup delta choice and is
/// never written back to the Arrow data.
pub fn point_inside_enclosed(
    set: &ObstacleSet,
    lat: f64,
    lon: f64,
) -> Option<EnclosedEnvelopeWinner> {
    let mut seen = Vec::new();
    set.indexes
        .iter()
        .enumerate()
        .filter_map(|(index_ordinal, index)| {
            index.containing_enclosed(lat, lon, 0.0, &mut seen).map(
                |(stored_class, height_m, footprint_ordinal)| {
                    (stored_class, height_m, index_ordinal, footprint_ordinal)
                },
            )
        })
        .max_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.3.cmp(&a.3))
        })
        .map(|(stored_class, height_m, _, _)| EnclosedEnvelopeWinner {
            stored_class,
            effective_class: effective_envelope_class(stored_class, height_m),
            height_m,
        })
}

/// Hover-only winner over every visible footprint, including Outdoor-class
/// carports and roof structures. The popup's indoor calculation deliberately
/// keeps using [`point_inside_enclosed`] so Outdoor does not become an indoor
/// attenuation estimate.
pub fn point_inside_footprint(
    set: &ObstacleSet,
    lat: f64,
    lon: f64,
) -> Option<(EnvelopeClass, f32)> {
    let mut seen = Vec::new();
    set.indexes
        .iter()
        .filter_map(|index| index.containing_footprint(lat, lon, 0.0, &mut seen))
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
        .map(|(class, height, _)| (class, height))
}

/// One cell's index, from the nearest source that still holds it: the process
/// memo, then the on-disk index cache, then a rebuild from the Arrow shards
/// (which is also written back). Build errors are not cached; successful
/// builds are immutable and shared.
///
/// The inputs are listed and fingerprinted BEFORE either cache is consulted:
/// both are keyed on that fingerprint, so a hit is only ever the index this
/// process would have built from these very files. The listing costs one
/// `read_dir` and a handful of `stat`s per cell per query — three orders below
/// the rebuild it guards, and the price of a cache that answers the question it
/// was asked.
fn cell_index(
    cell: CellIndex,
    dir: &Path,
    buildings_arrow: Option<&Path>,
    data_dir: &Path,
) -> Result<Arc<ObstacleIndex>, String> {
    let shards = shard_paths(dir)?;
    if shards.is_empty() {
        return Err(format!("shard dir emptied under us: {}", dir.display()));
    }
    let ver = cell_data_ver(cell, &shards, buildings_arrow);
    let t0 = std::time::Instant::now();
    if let Some(ver) = ver {
        if let Some(idx) = memo_get(cell, ver) {
            return Ok(idx);
        }
        if let Some(root) = index_cache_root(data_dir) {
            if let Some(idx) = load_cached_index(&cache_file_path(&root, cell), ver) {
                let idx = Arc::new(idx);
                log_cell_load(cell, "mapped", idx.edge_count(), t0);
                memo_put(cell, ver, &idx);
                return Ok(idx);
            }
        }
    }

    let built = Arc::new(build_cell_index(cell, &shards, buildings_arrow)?);
    // No fingerprint ⇒ no memo and no file. An index whose inputs could not be
    // pinned is used for THIS query and forgotten.
    if let Some(ver) = ver {
        if let Some(root) = index_cache_root(data_dir) {
            store_cached_index(&root, cell, &built, ver);
        }
        memo_put(cell, ver, &built);
    }
    log_cell_load(cell, "built", built.edge_count(), t0);
    Ok(built)
}

/// Per-cell provenance under `POPUP_TIMING=1` — the same lever
/// `query_noise_impl` uses for its stage timings. `mapped` vs `built` is the
/// entire difference this cache makes, so it belongs in one log line instead of
/// being inferred from a wall clock that also carries the Arrow hex load.
fn log_cell_load(cell: CellIndex, how: &str, edges: usize, t0: std::time::Instant) {
    if std::env::var("POPUP_TIMING").as_deref() == Ok("1") {
        eprintln!(
            "obstacle-index {how} cell={cell} edges={edges} in {:.0} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// Read a cell's `buildings.arrow` into the low-profile cap's lookup — the Arrow
/// half of [`noise_compute::low_profile`], which carries the rule itself (shared
/// with the tile painter's loader, so popup and tiles cap the same footprints).
///
/// ABSENT is not the same as UNREADABLE. No file (`NotFound`, or no path at all)
/// means the cell has ML-only coverage: nothing to cap against, so an empty
/// lookup is the right answer. An older schema without the four columns is the
/// same story — a correction layer that cannot be applied is not an error. But a
/// transient read/parse failure is: swallowing it caps NOTHING, and
/// [`cell_index`] then writes that uncapped index to disk AND to the memo under
/// the NORMAL fingerprint, so every later query reports garages at 8 m instead
/// of 3 m until the file's mtime happens to move (2026-08-08 review; the tile
/// painter's twin has always failed loud here, and popup ≠ tiles at every capped
/// footprint is exactly what this rule exists to prevent).
fn load_low_profile(buildings_arrow: Option<&Path>) -> Result<LowProfileLookup, String> {
    let empty = || Ok(LowProfileLookup::default());
    let Some(path) = buildings_arrow else {
        return empty();
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return empty(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", path.display()))?;
    let mut lookup = LowProfileLookup::default();
    for batch in reader {
        let batch = batch.map_err(|e| format!("arrow batch {}: {e}", path.display()))?;
        let (Some(lats), Some(lons), Some(types), Some(areas)) = (
            batch
                .column_by_name("centroid_lat")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
            batch
                .column_by_name("centroid_lon")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
            batch
                .column_by_name("building_type")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
        ) else {
            return empty(); // older schema → no capping, never an error
        };
        for i in 0..batch.num_rows() {
            if lats.is_null(i) || lons.is_null(i) || types.is_null(i) || areas.is_null(i) {
                continue;
            }
            lookup.insert_if_low(types.value(i), lats.value(i), lons.value(i), areas.value(i));
        }
    }
    Ok(lookup)
}

/// Build one cell's index from its sorted shards. The index origin is the
/// CELL CENTRE (not the query point) so the cache entry is query-independent;
/// crossings project the ray per call, so mixed origins across a set are fine.
///
/// `shards` is the caller's listing, not a fresh one: the same list must decide
/// the cache fingerprint AND the obstacle ordinals, or a shard appearing between
/// the two reads would key an index under the wrong inputs.
fn build_cell_index(
    cell: CellIndex,
    shards: &[PathBuf],
    buildings_arrow: Option<&Path>,
) -> Result<ObstacleIndex, String> {
    let centre = LatLng::from(cell);
    let mut builder = ObstacleIndex::builder(centre.lat(), centre.lng());
    let mut next_id: u32 = 0;
    let low_profile = load_low_profile(buildings_arrow)?;
    for path in shards {
        let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let reader = FileReader::try_new(Cursor::new(bytes), None)
            .map_err(|e| format!("arrow open {}: {e}", path.display()))?;
        for batch in reader {
            let batch = batch.map_err(|e| format!("arrow batch {}: {e}", path.display()))?;
            let wkb = batch
                .column_by_name("polygon_wkb")
                .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
                .ok_or_else(|| format!("{}: missing polygon_wkb", path.display()))?;
            let heights = batch
                .column_by_name("height_m")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .ok_or_else(|| format!("{}: missing height_m", path.display()))?;
            let tiers = batch
                .column_by_name("height_tier")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
            let clats = batch
                .column_by_name("centroid_lat")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
            let clons = batch
                .column_by_name("centroid_lon")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
            for i in 0..batch.num_rows() {
                if wkb.is_null(i) || heights.is_null(i) {
                    return Err(format!("{}: null row {i}", path.display()));
                }
                let mut height = heights.value(i);
                if let (Some(tiers), Some(clats), Some(clons)) = (tiers, clats, clons) {
                    if !tiers.is_null(i) && !clats.is_null(i) && !clons.is_null(i) {
                        height = low_profile.capped_height(
                            height,
                            tiers.value(i),
                            clats.value(i),
                            clons.value(i),
                            noise_compute::wkb::outer_ring_area_m2(wkb.value(i)),
                        );
                    }
                }
                let class = batch
                    .column_by_name("envelope_class")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
                    .filter(|a| !a.is_null(i))
                    .map(|a| EnvelopeClass::from_u8(a.value(i)))
                    .unwrap_or(EnvelopeClass::Default);
                builder.add_polygon_wkb(
                    wkb.value(i),
                    height,
                    ObstacleKind::Building,
                    next_id,
                    class,
                );
                next_id = next_id.wrapping_add(1);
            }
        }
    }
    Ok(builder.build())
}

/// One footprint as the MODEL uses it (display twin of `build_cell_index`):
/// the outer ring in lat/lon plus the AS-USED height — after the low-profile
/// cap — its ingest tier, and whether the cap fired. Feeds the
/// building-height debug overlay so the map shows exactly what the
/// propagation engine screens with (owner ask 2026-08-02).
pub struct FootprintView {
    /// (lat, lon) outer-ring vertices (closed or open as stored).
    pub outer: Vec<(f64, f64)>,
    pub height_m: f32,
    pub tier: u8,
    pub capped: bool,
}

/// Footprints intersecting the bbox (by centroid, padded one bucket) with
/// as-used heights. Cells resolved exactly like a query: the res-4 cells of the
/// bbox corners/centre plus their ring, deduped.
///
/// This overlay draws what the engine screens with, so it follows the physics
/// loader's rule rather than a softer one: a cell that is provably empty
/// contributes nothing, and anything ELSE that stops us reading it is an error.
/// Returning an empty list on a broken shard would paint a transparent tile,
/// and on a noise map an absent building is indistinguishable from a quiet
/// place (raised in review, 2026-08-30).
pub fn footprints_in_bbox(
    h3r4_dir: Option<&Path>,
    data_dir: &Path,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> Result<Vec<FootprintView>, String> {
    let mut cells: Vec<CellIndex> = Vec::new();
    for (la, lo) in [
        (south, west),
        (south, east),
        (north, west),
        (north, east),
        ((south + north) / 2.0, (west + east) / 2.0),
    ] {
        if let Ok(ll) = LatLng::new(la, lo) {
            for c in ll.to_cell(Resolution::Four).grid_disk::<Vec<_>>(1) {
                if !cells.contains(&c) {
                    cells.push(c);
                }
            }
        }
    }
    let pad = 0.01;
    let mut out = Vec::new();
    let manifest = ingest_manifest(h3r4_dir, data_dir);
    for cell in cells {
        let dir = match cell_dir(h3r4_dir, data_dir, cell) {
            Err(e) => return Err(format!("obstacle_store: {e}")),
            Ok(Some(dir)) => dir,
            Ok(None) => {
                if manifest.is_some_and(|m| m.covers_cell(cell)) {
                    continue; // proven ingested-empty — nothing to draw here
                }
                return Err(format!(
                    "obstacle_store: cell {cell} has no obstacle shard and the ingest \
                     manifest does not prove it empty"
                ));
            }
        };
        let buildings_arrow = h3r4_dir.map(|h| h.join(cell.to_string()).join("buildings.arrow"));
        // A cell whose cap cannot be read must not contribute footprints at
        // their uncapped height: a wrong number here is worse than an error.
        let low_profile = load_low_profile(buildings_arrow.as_deref())
            .map_err(|e| format!("obstacle_store: low-profile cap for {cell}: {e}"))?;
        let shards = shard_paths(&dir).map_err(|e| format!("obstacle_store: {cell}: {e}"))?;
        for path in shards {
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("obstacle_store: {}: {e}", path.display()))?;
            let reader = FileReader::try_new(Cursor::new(bytes), None)
                .map_err(|e| format!("obstacle_store: {}: {e}", path.display()))?;
            for batch in reader {
                let batch =
                    batch.map_err(|e| format!("obstacle_store: {}: {e}", path.display()))?;
                let (Some(wkb), Some(heights), Some(clats), Some(clons)) = (
                    batch
                        .column_by_name("polygon_wkb")
                        .and_then(|c| c.as_any().downcast_ref::<BinaryArray>()),
                    batch
                        .column_by_name("height_m")
                        .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
                    batch
                        .column_by_name("centroid_lat")
                        .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
                    batch
                        .column_by_name("centroid_lon")
                        .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
                ) else {
                    continue;
                };
                let tiers = batch
                    .column_by_name("height_tier")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
                for i in 0..batch.num_rows() {
                    if wkb.is_null(i) || heights.is_null(i) || clats.is_null(i) || clons.is_null(i)
                    {
                        continue;
                    }
                    let (clat, clon) = (clats.value(i), clons.value(i));
                    if clat < south - pad
                        || clat > north + pad
                        || clon < west - pad
                        || clon > east + pad
                    {
                        continue;
                    }
                    let raw = heights.value(i);
                    let tier = tiers.map(|t| t.value(i)).unwrap_or(0);
                    let height = low_profile.capped_height(
                        raw,
                        tier,
                        clat,
                        clon,
                        noise_compute::wkb::outer_ring_area_m2(wkb.value(i)),
                    );
                    for (outer, _holes) in
                        noise_compute::wkb::parse_wkb_polygons_bytes(wkb.value(i))
                    {
                        out.push(FootprintView {
                            outer: outer.clone(),
                            height_m: height,
                            tier,
                            capped: height < raw,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Environment variables are PROCESS-global and `cargo test` runs tests in
    /// parallel threads, so two tests setting `QM_OBSTACLES_DIR` read each
    /// other's value. Every test below that touches the environment takes this
    /// lock and restores what it found, and every one of them points the two
    /// caches at its OWN temp dirs — the suite's answer must not depend on its
    /// order (2026-08-05: it did, in both directions).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Holds [`ENV_LOCK`] and the previous values of the vars it pinned.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        /// Pin `vars` (`None` = unset) for the rest of the test body.
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut saved = Vec::new();
            for (k, v) in vars {
                saved.push((*k, std::env::var(k).ok()));
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
            EnvGuard { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    fn path_str(dir: &TempDir) -> String {
        dir.path().to_str().expect("utf-8 temp path").to_string()
    }

    /// Runs only where the Praha obstacle staging exists (dev boxes after the
    /// geodata-v2 night-1 ingest); hermetic CI skips silently. Asserts the
    /// real scale and that the second load is a cache hit, not a rebuild.
    #[test]
    fn loads_praha_set_and_caches_cells() {
        let data_dir = Path::new("../../data/prepared");
        let staged = data_dir
            .parent()
            .map(|d| d.join("enrichment/global/overture-obstacles/h3r4"))
            .is_some_and(|d| d.is_dir());
        if !staged {
            return;
        }
        // Its own index dir: this test must exercise the COLD build, and it
        // must not read (or evict) the box's production cache.
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[
            ("QM_OBSTACLES_DIR", None),
            ("QM_OBSTACLES_ALLOW_PARTIAL", Some("1")),
            ("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir))),
        ]);
        let t0 = std::time::Instant::now();
        let Ok(set) = load_obstacle_set(None, data_dir, 50.08, 14.43) else {
            return; // cell not ingested on this box — skip
        };
        let cold = t0.elapsed();
        assert!(
            set.edge_count() > 100_000,
            "central Praha must index >100k edges, got {}",
            set.edge_count()
        );
        let mut out = Vec::new();
        set.crossings(50.08, 14.43, 50.095, 14.45, &mut out);
        assert!(
            !out.is_empty(),
            "a 2 km central-Praha ray must cross footprints"
        );
        assert!(out.windows(2).all(|w| w[0].t <= w[1].t));

        let t1 = std::time::Instant::now();
        let set2 = load_obstacle_set(None, data_dir, 50.08, 14.43).expect("cached reload");
        let warm = t1.elapsed();
        assert_eq!(set2.edge_count(), set.edge_count());
        assert!(
            warm < cold / 5,
            "second load must be a cache hit: cold {cold:?}, warm {warm:?}"
        );
        std::env::remove_var("QM_OBSTACLES_ALLOW_PARTIAL");
    }

    /// Fix 4 popup half, on synthetic footprints so it runs everywhere: a
    /// 100 m block with a 30 m courtyard plus a low 3 m garage beside it.
    /// * block interior → inside, reporting the footprint's height;
    /// * COURTYARD → outdoors (a hole shares its outer ring's id, so the
    ///   crossing-parity test reads it outside — same rule as the heatmap
    ///   mask);
    /// * open ground → outdoors;
    /// * the 3 m garage → inside at 3 m (no 5 m enclosure gate here);
    /// * where two footprints overlap the TALLEST one is reported.
    #[test]
    fn point_inside_obstacle_matrix() {
        use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
        use noise_compute::propagation::obstacle_index::ObstacleIndex;

        const OLAT: f64 = 50.08;
        const OLON: f64 = 14.43;
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(OLAT.to_radians());
        let square = |north_m: f64, east_m: f64, half: f64| {
            vec![
                (OLAT + d_lat(north_m - half), OLON + d_lon(east_m - half)),
                (OLAT + d_lat(north_m - half), OLON + d_lon(east_m + half)),
                (OLAT + d_lat(north_m + half), OLON + d_lon(east_m + half)),
                (OLAT + d_lat(north_m + half), OLON + d_lon(east_m - half)),
            ]
        };
        let at = |north_m: f64, east_m: f64| (OLAT + d_lat(north_m), OLON + d_lon(east_m));

        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 50.0), 18.5, ObstacleKind::Building, 0);
        b.add_ring(&square(0.0, 0.0, 15.0), 18.5, ObstacleKind::Building, 0); // courtyard
        b.add_ring(&square(0.0, 200.0, 10.0), 3.0, ObstacleKind::Building, 1); // garage
        b.add_ring(&square(0.0, 205.0, 10.0), 40.0, ObstacleKind::Building, 2); // tower over it
        let set = ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        };

        let h = point_inside_obstacle(&set, at(30.0, 0.0).0, at(30.0, 0.0).1)
            .expect("block interior is inside");
        assert!((h - 18.5).abs() < 0.1, "block height {h} ≠ 18.5");
        assert!(
            point_inside_obstacle(&set, at(0.0, 0.0).0, at(0.0, 0.0).1).is_none(),
            "courtyard is open ground"
        );
        assert!(
            point_inside_obstacle(&set, at(0.0, 120.0).0, at(0.0, 120.0).1).is_none(),
            "open ground between footprints"
        );
        let g = point_inside_obstacle(&set, at(0.0, 193.0).0, at(0.0, 193.0).1)
            .expect("a 3 m garage interior is still an interior");
        assert!((g - 3.0).abs() < 0.1, "garage height {g} ≠ 3.0");
        let t = point_inside_obstacle(&set, at(0.0, 203.0).0, at(0.0, 203.0).1)
            .expect("garage ∩ tower");
        assert!(
            (t - 40.0).abs() < 0.1,
            "overlap must report the tallest, got {t}"
        );

        let block = point_inside_enclosed(&set, at(30.0, 0.0).0, at(30.0, 0.0).1)
            .expect("the tall unclassified block is an enclosed winner");
        assert_eq!(block.stored_class, EnvelopeClass::Default);
        assert_eq!(block.effective_class, EnvelopeClass::Default);
        assert_eq!(block.effective_class.delta_db(), Some(25.0));

        let garage = point_inside_enclosed(&set, at(0.0, 193.0).0, at(0.0, 193.0).1)
            .expect("the short unclassified garage is an enclosed winner");
        assert_eq!(garage.stored_class, EnvelopeClass::Default);
        assert_eq!(garage.effective_class, EnvelopeClass::Industrial);
        assert_eq!(garage.effective_class.delta_db(), Some(20.0));

        // Equal-height cross-index winners must use the lower index ordinal,
        // just like the painter. The first index is DEFAULT at the 6 m
        // boundary (effective 20 dB); the later index is residential (30 dB).
        let tie_wkb = {
            let ring = square(0.0, -300.0, 20.0);
            let mut wkb = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
            for &(lat, lon) in ring.iter().chain(std::iter::once(&ring[0])) {
                wkb.extend_from_slice(&lon.to_le_bytes());
                wkb.extend_from_slice(&lat.to_le_bytes());
            }
            wkb
        };
        let mut first = ObstacleIndex::builder(OLAT, OLON);
        first.add_polygon_wkb(
            &tie_wkb,
            6.0,
            ObstacleKind::Building,
            0,
            EnvelopeClass::Default,
        );
        let mut second = ObstacleIndex::builder(OLAT, OLON);
        second.add_polygon_wkb(
            &tie_wkb,
            6.0,
            ObstacleKind::Building,
            0,
            EnvelopeClass::Residential,
        );
        let tie_set = ObstacleSet {
            indexes: vec![Arc::new(first.build()), Arc::new(second.build())],
        };
        let tie = point_inside_enclosed(&tie_set, at(0.0, -300.0).0, at(0.0, -300.0).1)
            .expect("equal-height cross-index footprints overlap");
        assert_eq!(tie.stored_class, EnvelopeClass::Default);
        assert_eq!(tie.effective_class, EnvelopeClass::Industrial);
        assert_eq!(tie.effective_class.delta_db(), Some(20.0));
    }

    #[test]
    fn point_inside_footprint_keeps_outdoor_hover_structure() {
        use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
        use noise_compute::propagation::obstacle_index::ObstacleIndex;

        const OLAT: f64 = 50.08;
        const OLON: f64 = 14.43;
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(OLAT.to_radians());
        let mut wkb = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
        for (lon, lat) in [
            (OLON - d_lon(10.0), OLAT - d_lat(10.0)),
            (OLON + d_lon(10.0), OLAT - d_lat(10.0)),
            (OLON + d_lon(10.0), OLAT + d_lat(10.0)),
            (OLON - d_lon(10.0), OLAT + d_lat(10.0)),
            (OLON - d_lon(10.0), OLAT - d_lat(10.0)),
        ] {
            wkb.extend_from_slice(&lon.to_le_bytes());
            wkb.extend_from_slice(&lat.to_le_bytes());
        }

        let mut builder = ObstacleIndex::builder(OLAT, OLON);
        builder.add_polygon_wkb(&wkb, 3.0, ObstacleKind::Building, 0, EnvelopeClass::Outdoor);
        let set = ObstacleSet {
            indexes: vec![Arc::new(builder.build())],
        };

        assert_eq!(
            point_inside_footprint(&set, OLAT, OLON),
            Some((EnvelopeClass::Outdoor, 3.0))
        );
        assert!(point_inside_enclosed(&set, OLAT, OLON).is_none());
    }

    /// One tiny valid shard: a single closed square footprint (~20 m) whose
    /// south-west corner sits at `(lat, lon)`, WKB little-endian Polygon with
    /// 1 ring × 5 points.
    fn write_test_shard(dir: &Path, name: &str, lat: f64, lon: f64) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let schema = arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("polygon_wkb", arrow::datatypes::DataType::Binary, false),
            arrow::datatypes::Field::new("height_m", arrow::datatypes::DataType::Float32, false),
        ]);
        let mut wkb: Vec<u8> = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
        for (dlon, dlat) in [
            (0.0, 0.0),
            (0.0003, 0.0),
            (0.0003, 0.0002),
            (0.0, 0.0002),
            (0.0, 0.0),
        ] {
            wkb.extend_from_slice(&f64::to_le_bytes(lon + dlon));
            wkb.extend_from_slice(&f64::to_le_bytes(lat + dlat));
        }
        let batch = arrow::record_batch::RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![
                Arc::new(BinaryArray::from_vec(vec![&wkb])),
                Arc::new(Float32Array::from(vec![9.0_f32])),
            ],
        )
        .unwrap();
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut w = arrow::ipc::writer::FileWriter::try_new(file, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
        path
    }

    /// The disk cache must give back exactly the index that was built, and must
    /// refuse it the moment its inputs move. Exercised on the real functions
    /// the query path calls — the process LRU would hide the disk hop if this
    /// went through `load_obstacle_set`.
    #[test]
    fn disk_cache_round_trips_and_follows_its_inputs() {
        let tmp = TempDir::new().expect("temp dir");
        let tmp = tmp.path();
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let dir = tmp.join(cell.to_string());
        let shard = write_test_shard(&dir, "obstacles-TEST.arrow", 50.08, 14.43);
        let root = tmp.join("index-cache");

        let shards = shard_paths(&dir).unwrap();
        let data_ver = cell_data_ver(cell, &shards, None).expect("input fingerprint");
        let built = build_cell_index(cell, &shards, None).unwrap();
        store_cached_index(&root, cell, &built, data_ver);

        let path = cache_file_path(&root, cell);
        assert!(path.is_file(), "the cache file must exist at {path:?}");
        let mapped = load_cached_index(&path, data_ver).expect("maps back");
        assert_eq!(mapped.edge_count(), built.edge_count());
        let ray = |idx: &ObstacleIndex| {
            let mut out = Vec::new();
            idx.crossings(50.0801, 14.4295, 50.0801, 14.4310, &mut out);
            out
        };
        let (a, b) = (ray(&built), ray(&mapped));
        assert_eq!(a.len(), 2, "the probe ray must enter and leave the square");
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.t.to_bits(), y.t.to_bits());
            assert_eq!(x.height_m.to_bits(), y.height_m.to_bits());
            assert_eq!(x.id, y.id);
        }

        // A shard that moved under us must not be served from cache — and the
        // refused file is dropped so it cannot sit in the budget forever.
        let touched = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&shard)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        let moved = cell_data_ver(cell, &shards, None).expect("input fingerprint");
        assert_ne!(
            moved, data_ver,
            "an input mtime must rotate the fingerprint"
        );
        assert!(load_cached_index(&path, moved).is_none(), "stale file used");
        assert!(!path.exists(), "a refused cache file must be removed");

        // A second shard is a different input SET, hence a different index.
        write_test_shard(&dir, "obstacles-TEST2.arrow", 50.081, 14.431);
        let two = shard_paths(&dir).unwrap();
        assert_eq!(two.len(), 2);
        assert_ne!(cell_data_ver(cell, &two, None).unwrap(), moved);
    }

    /// EVERY input that shapes a cell's index must move its identity, and a
    /// file written under the old one must then be refused.
    ///
    /// This is the regression guard for 2026-08-05, when a cache served the
    /// answer to a different question. It is written as an ENUMERATION rather
    /// than one case on purpose: the defect class is "the key forgot
    /// something", so the test walks the closed list [`cell_data_ver`] actually
    /// folds — cell, shard ROOT, shard set, per-shard LENGTH and mtime, the
    /// `buildings.arrow` the low-profile cap reads, and [`CACHE_CODE_VER`].
    /// Adding an input to `build_cell_index` without a case here leaves the same
    /// hole, so the list is the review surface.
    ///
    /// NOT in the list, deliberately: shard CONTENT. Rewriting a shard to the
    /// same length while forcing the same mtime keeps the identity — that is the
    /// `world-stamps.py` staleness contract (mtime is the change signal), not an
    /// oversight, and hashing hundreds of MB per cell per query to close it would
    /// cost more than the rebuild the cache exists to avoid.
    #[test]
    fn cache_identity_moves_with_everything_that_shapes_the_index() {
        let tmp = TempDir::new().expect("temp dir");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let other_cell = LatLng::new(-23.5505, -46.6333)
            .unwrap()
            .to_cell(Resolution::Four);
        let dir = tmp.path().join(cell.to_string());
        let shard = write_test_shard(&dir, "obstacles-TEST.arrow", 50.08, 14.43);
        let root = tmp.path().join("index-cache");
        let shards = shard_paths(&dir).unwrap();
        let base = cell_data_ver(cell, &shards, None).expect("input fingerprint");

        // Positive control FIRST: without it every `is_none()` below would
        // also pass on a key that is simply always wrong.
        let built = build_cell_index(cell, &shards, None).unwrap();
        let path = cache_file_path(&root, cell);
        store_cached_index(&root, cell, &built, base);
        assert!(
            load_cached_index(&path, base).is_some(),
            "the unchanged identity must still map its own file"
        );

        // Each mutation must rotate the identity AND make the stored file
        // unusable. `load_cached_index` deletes what it refuses, so the file is
        // re-written before every case.
        let refuses = |ver: u64, what: &str| {
            assert_ne!(ver, base, "{what} must rotate the index identity");
            store_cached_index(&root, cell, &built, base);
            assert!(
                load_cached_index(&path, ver).is_none(),
                "{what}: a file written under the old identity was served"
            );
        };

        // 1. The cell — its centre is the index's metric origin, so the same
        //    shards under another cell are a different index.
        refuses(
            cell_data_ver(other_cell, &shards, None).unwrap(),
            "a different cell",
        );

        // 2. The shard ROOT: identical bytes at another path are another
        //    staging tree (`QM_OBSTACLES_DIR`, an A/B mount) and may hold
        //    entirely different obstacles for this cell tomorrow.
        let root_b = TempDir::new().expect("temp dir");
        let dir_b = root_b.path().join(cell.to_string());
        write_test_shard(&dir_b, "obstacles-TEST.arrow", 50.08, 14.43);
        refuses(
            cell_data_ver(cell, &shard_paths(&dir_b).unwrap(), None).unwrap(),
            "another shard root",
        );

        // 3. A shard's mtime (the world-stamps.py staleness contract).
        let touched = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&shard)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        refuses(
            cell_data_ver(cell, &shards, None).unwrap(),
            "a shard's mtime",
        );

        // 4. `buildings.arrow` appearing — the low-profile cap reads it, so
        //    the very same shards yield different HEIGHTS with it present.
        let b_arrow = dir.join("buildings.arrow");
        let absent = cell_data_ver(cell, &shards, Some(&b_arrow)).unwrap();
        assert_ne!(
            absent, base,
            "asking about a buildings.arrow at all is a different question \
             from not consulting one"
        );
        std::fs::write(&b_arrow, b"not-arrow-but-present").unwrap();
        refuses(
            cell_data_ver(cell, &shards, Some(&b_arrow)).unwrap(),
            "a buildings.arrow appearing",
        );

        // 5. A shard's LENGTH — the one content signal the key carries, folded
        //    per shard next to the mtime. Pinned at a FIXED mtime so it is the
        //    length alone doing the work, not case 3 again.
        let before_len = cell_data_ver(cell, &shards, None).unwrap();
        std::fs::write(&shard, b"a-shard-of-a-very-different-length").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&shard)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        let after_len = cell_data_ver(cell, &shards, None).unwrap();
        assert_ne!(
            after_len, before_len,
            "a shard's length must rotate the identity at an unchanged mtime"
        );
        refuses(after_len, "a shard's length");

        // 6. A shard SET that grew.
        write_test_shard(&dir, "obstacles-TEST2.arrow", 50.081, 14.431);
        let two = shard_paths(&dir).unwrap();
        assert_eq!(two.len(), 2);
        refuses(cell_data_ver(cell, &two, None).unwrap(), "an extra shard");

        // 7. This file's own decisions (id ordering, shard order, the capping
        //    call) are in the version, on top of the builder's chain — which is
        //    what carries the low-profile rule now that it lives in
        //    `noise_compute::low_profile`.
        assert_eq!(
            CACHE_CODE_VER,
            fnv1a64(BUILDER_CODE_VER, include_bytes!("obstacle_store.rs"))
        );
        assert_ne!(
            CACHE_CODE_VER, BUILDER_CODE_VER,
            "the loader's own source must be in the version, not just the builder's"
        );
    }

    /// The process memo in front of the disk cache must key on the same
    /// identity the file does — 2026-08-05's live defect, where it keyed on the
    /// CELL alone and handed the second query the first query's index no matter
    /// which shard root it asked about.
    ///
    /// Runs entirely through `cell_index`, the function the query path calls,
    /// because that is the layer that was wrong: the disk file always carried
    /// its fingerprint and would have refused these.
    #[test]
    fn process_memo_never_answers_for_another_shard_root() {
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let one = TempDir::new().expect("temp dir");
        let two = TempDir::new().expect("temp dir");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);

        // Same cell, two staging trees: one square in A, two in B.
        let dir_a = one.path().join(cell.to_string());
        let dir_b = two.path().join(cell.to_string());
        write_test_shard(&dir_a, "obstacles-TEST.arrow", 50.08, 14.43);
        write_test_shard(&dir_b, "obstacles-TEST.arrow", 50.08, 14.43);
        write_test_shard(&dir_b, "obstacles-TEST2.arrow", 50.081, 14.431);

        let data_dir = one.path().join("prepared");
        let edges = |dir: &Path| {
            cell_index(cell, dir, None, &data_dir)
                .expect("test shards build")
                .edge_count()
        };
        assert_eq!(edges(&dir_a), 4, "one square is four edges");
        assert_eq!(
            edges(&dir_b),
            8,
            "the memo served A's index for B's shards — it is keyed on the \
             cell, not on the inputs"
        );
        assert_eq!(edges(&dir_a), 4, "and back, without either poisoning");

        // A shard changing under a LIVE process is the same hole with one
        // root: the memo must notice, not hold yesterday's obstacles.
        write_test_shard(&dir_a, "obstacles-TEST2.arrow", 50.081, 14.431);
        assert_eq!(edges(&dir_a), 8, "a shard added under us must be picked up");
    }

    /// Strict default: a missing ring cell aborts to the raster path; the dev
    /// override admits the partial disk.
    ///
    /// Runs strict → partial → strict on ONE process and one disk, which is
    /// the question "does `QM_OBSTACLES_ALLOW_PARTIAL` belong in the cache
    /// key?" asked as a test. It does not: the flag is re-read per query in
    /// `load_obstacle_set` and only ever decides which CELLS are assembled
    /// into the set; a cache entry is one cell's index, built from that cell's
    /// shards alone, so no dev A/B run can leave a file that answers a strict
    /// query differently. The final strict pass is what would catch it if that
    /// ever stopped being true.
    #[test]
    fn missing_ring_cell_fails_unless_partial_allowed() {
        let tmp = TempDir::new().expect("temp dir");
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[
            ("QM_OBSTACLES_DIR", Some(&path_str(&tmp))),
            ("QM_OBSTACLES_ALLOW_PARTIAL", None),
            ("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir))),
        ]);
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let dir = tmp.path().join(cell.to_string());
        write_test_shard(&dir, "obstacles-TEST.arrow", 50.08, 14.43);
        let data_dir = tmp.path().join("prepared");

        assert!(
            load_obstacle_set(None, &data_dir, 50.08, 14.43).is_err(),
            "missing ring cells must fail the query: buildings are vector-only"
        );

        std::env::set_var("QM_OBSTACLES_ALLOW_PARTIAL", "1");
        let partial = load_obstacle_set(None, &data_dir, 50.08, 14.43)
            .expect("dev override must admit the partial disk");
        assert_eq!(partial.edge_count(), 4);

        // The A/B run just cached this cell, in memory and on disk. The strict
        // query must still refuse the incomplete ring.
        std::env::remove_var("QM_OBSTACLES_ALLOW_PARTIAL");
        assert!(
            load_obstacle_set(None, &data_dir, 50.08, 14.43).is_err(),
            "a partial-mode run must not leave anything that answers a strict query"
        );
    }
}
