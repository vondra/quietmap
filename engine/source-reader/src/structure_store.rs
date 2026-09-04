//! Vector structure loading for the popup.
//!
//! Each query assembles an
//! [`ObstacleSet`] from PER-CELL [`ObstacleIndex`]es covering the query
//! cell's `grid_disk(1)` — the halo the ingest contract requires
//! (centroid-assigned footprints; `scripts/structures/build-structures.py`).
//! One `structures.arrow` per cell carries BOTH screening stocks — buildings
//! (kind 0, polygons) and noise walls (kind 1, polyline microsegments, indexed
//! as [`ObstacleKind::Barrier`] edges) — and, in its OSM-attributed rows, the
//! input of the low-profile height cap's lookup. One table, one read.
//!
//! Two hard rules:
//! - **Bounded cost.** Per-cell indexes are built ONCE per process and
//!   LRU-cached (`CELL_CACHE_CAP`); a query only Arc-clones ≤7 of them.
//!   The naive per-query rebuild measured 448 MB RSS / 0.47 s per popup.
//! - **All-or-error.** Any read/parse error, and any ring-1 cell of the
//!   prepared world whose `structures.arrow` is missing, aborts the whole load.
//!   A partial index would silently under-screen the path. Emptiness is not a
//!   gap: a 0-row table is the answer "nothing stands here"
//!   (`noise_compute::propagation::structure_cell_file` carries the rule and the
//!   third case, a cell outside the prepared world).
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
//! cell alone and so answered a query about one obstacle tree with an index
//! built from another (2026-08-05); the popup is the project's acoustic
//! reference, and a cache that returns the answer to a different question is
//! worse than no cache. The ring is deliberately NOT in that key: it decides
//! which cells are assembled into a set, per query, in [`load_obstacle_set`],
//! while a cache entry is one cell's index built from that cell's own file.

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use arrow::array::{
    Array, BinaryArray, Float32Array, Float64Array, Int64Array, UInt32Array, UInt8Array,
};
use arrow::ipc::reader::FileReader;
use h3o::{CellIndex, LatLng, Resolution};
use noise_compute::envelope::{effective_envelope_class, EnvelopeClass};
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::propagation::obstacle_index_file::{fnv1a64, IndexBlob, BUILDER_CODE_VER};
use noise_compute::propagation::structure_cell_file::locate_cell_structures;

use crate::hex_store::{STRUCTURE_KIND_BARRIER, STRUCTURE_KIND_BUILDING};

/// Per-cell index cache capacity. A dense metro cell's index runs to low
/// hundreds of MB; popups cluster spatially, so a small LRU covers the
/// active area while bounding worst-case RSS.
const CELL_CACHE_CAP: usize = 8;

/// Everything that decides a cached index's BYTES: the engine's builder and
/// grid (`BUILDER_CODE_VER`) folded with THIS file, which owns the loader's own
/// decisions — the obstacle id ordering (dense, by file order), and which rows
/// are offered to the height cap. Editing either side rotates the version and every file written by the
/// old code is refused, exactly as `scripts/layer-codever.py` re-stales tiles on
/// a source change. Over-invalidating costs a rebuild; under-invalidating puts a
/// silently wrong screen in the map.
///
/// The low-profile cap needs no fold of its own: the rule lives in
/// `noise_compute::low_profile`, which [`BUILDER_CODE_VER`] hashes — so a change
/// to its class list, its match geometry or its cap rotates this version without
/// anyone naming the constants here.
const CACHE_CODE_VER: u64 = fnv1a64(BUILDER_CODE_VER, include_bytes!("structure_store.rs"));

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
/// prepared artifacts (`prepared/dem`, `prepared/rasters`).
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
/// * the cell's `structures.arrow` as (path, length, mtime) — the path because
///   two prepared TREES (a moved mount, a second checkout's data node) hold
///   different structures for the same cell. The low-profile cap reads the
///   SAME file, so it needs no fold of its own.
///
/// That list is closed by construction: `build_cell_index` reads its cell and
/// that one table, and nothing else — no env, no clock, no map iteration order
/// (`ObstacleIndex::build` is a Vec walk). Whatever a future edit adds to it
/// lands in THIS file, and this file's content is already in
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
fn cell_data_ver(cell: CellIndex, structures_arrow: &Path) -> Option<u64> {
    let mut h = fnv1a64(CACHE_CODE_VER, b"structure-index-inputs-v1");
    h = fnv1a64(h, &u64::from(cell).to_le_bytes());
    h = fnv1a64(h, structures_arrow.as_os_str().as_encoded_bytes());
    h = fnv1a64(h, &[1]); // present: the locator handed us an existing file
    let meta = std::fs::metadata(structures_arrow).ok()?;
    h = fnv1a64(h, &meta.len().to_le_bytes());
    let mtime = meta.modified().ok()?;
    let since_epoch = mtime
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?;
    h = fnv1a64(h, &since_epoch.as_nanos().to_le_bytes());
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
            eprintln!("structure_store: ignoring cached {}: {e}", path.display());
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
        eprintln!("structure_store: no index cache at {}: {e}", root.display());
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
            "structure_store: could not cache index {}: {e}",
            final_path.display()
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Process-local memo of built indexes, keyed on the SAME identity the disk
/// file carries ([`cell_data_ver`]) — not on the cell.
///
/// Keying it on the cell alone was a live defect (2026-08-05): the second
/// query for a cell got the first query's index no matter which obstacle tree
/// it asked about, so a run against a second prepared root screened the popup
/// against obstacles that are not there. The disk cache never had this hole —
/// its header carries the fingerprint — which is exactly why the memo in front
/// of it had to grow one.
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

/// Assemble the query's [`ObstacleSet`], or fail when vector coverage cannot
/// be proved complete.
pub fn load_obstacle_set(
    h3r4_dir: &Path,
    data_dir: &Path,
    lat: f64,
    lon: f64,
) -> Result<ObstacleSet, String> {
    let cell = LatLng::new(lat, lon)
        .map_err(|e| format!("structure_store: {lat},{lon} is not a point on earth: {e}"))?
        .to_cell(Resolution::Four);
    let mut indexes = Vec::new();
    for c in cell.grid_disk::<Vec<_>>(1) {
        let located = locate_cell_structures(h3r4_dir, c).map_err(|e| {
            format!(
                "structure_store: {e} — buildings are vector-only, so this query cannot be answered"
            )
        })?;
        let Some(structures_arrow) = located else {
            // Outside the prepared world: no cell directory at all, so it holds
            // no structures for the same reason it holds no roads.
            continue;
        };
        match cell_index(c, &structures_arrow, data_dir) {
            Ok(idx) => indexes.push(idx),
            Err(e) => return Err(format!("structure_store: {e}")),
        }
    }
    // Zero edges is a legitimate answer: a 0-row table is the finished sweep
    // saying there is nothing here. A file that exists HAS been asked and HAS
    // answered; treating its emptiness as a fault would take whole countries
    // silent the moment an Overture release rejects their heights (raised in
    // review, 2026-08-30).
    Ok(ObstacleSet { indexes })
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
/// memo, then the on-disk index cache, then a rebuild from the cell's Arrow
/// table (which is also written back). Build errors are not cached; successful
/// builds are immutable and shared.
///
/// The inputs are fingerprinted BEFORE either cache is consulted: both are
/// keyed on that fingerprint, so a hit is only ever the index this process
/// would have built from these very files. Two `stat`s per cell per query —
/// three orders below the rebuild they guard, and the price of a cache that
/// answers the question it was asked.
fn cell_index(
    cell: CellIndex,
    structures_arrow: &Path,
    data_dir: &Path,
) -> Result<Arc<ObstacleIndex>, String> {
    let ver = cell_data_ver(cell, structures_arrow);
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

    let built = Arc::new(build_cell_index(cell, structures_arrow)?);
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

/// The low-profile cap's lookup, read from the SAME structures.arrow the index
/// is built from: kind=0 rows with a valid `osm_id` are the OSM building stock
/// (the merge's emission rows — the old buildings.arrow subsequence), matched
/// at their emission centroid where the merge kept one, else the screening
/// centroid. The rule itself lives in [`noise_compute::low_profile`] (shared
/// with the tile painter's loader, so popup and tiles cap the same footprints).
///
/// A table without the OSM attribute columns is a pre-merge file: nothing to
/// cap against, so an empty lookup is the right answer — a correction layer
/// that cannot be applied is not an error. A parse failure of the in-memory
/// bytes IS an error: swallowing it would cap NOTHING and [`cell_index`] would
/// write that uncapped index to disk AND to the memo under the NORMAL
/// fingerprint, so every later query reports garages at 8 m instead of 3 m
/// until the file's mtime happens to move (2026-08-08 review; the tile
/// painter's twin has always failed loud here, and popup ≠ tiles at every capped
/// footprint is exactly what this rule exists to prevent).
fn low_profile_from_structures(bytes: &[u8], label: &Path) -> Result<LowProfileLookup, String> {
    let empty = || Ok(LowProfileLookup::default());
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", label.display()))?;
    let mut lookup = LowProfileLookup::default();
    for batch in reader {
        let batch = batch.map_err(|e| format!("arrow batch {}: {e}", label.display()))?;
        let (Some(kinds), Some(osm_ids), Some(lats), Some(lons), Some(types), Some(areas)) = (
            batch
                .column_by_name("kind")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("osm_id")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>()),
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
            return empty(); // pre-merge schema → no capping, never an error
        };
        // The merge moves an OSM-matched row's screening centroid to the
        // Overture footprint's; the OSM one survives as emission_centroid_*.
        let elats = batch
            .column_by_name("emission_centroid_lat")
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
        let elons = batch
            .column_by_name("emission_centroid_lon")
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
        for i in 0..batch.num_rows() {
            if kinds.value(i) != STRUCTURE_KIND_BUILDING
                || osm_ids.is_null(i)
                || types.is_null(i)
                || areas.is_null(i)
            {
                continue;
            }
            let (lat, lon) = match (elats, elons) {
                (Some(elats), Some(elons)) if !elats.is_null(i) && !elons.is_null(i) => {
                    (elats.value(i), elons.value(i))
                }
                _ if !lats.is_null(i) && !lons.is_null(i) => (lats.value(i), lons.value(i)),
                _ => continue,
            };
            lookup.insert_if_low(types.value(i), lat, lon, areas.value(i));
        }
    }
    Ok(lookup)
}

/// Build one cell's index from its structure table. The index origin is the
/// CELL CENTRE (not the query point) so the cache entry is query-independent;
/// crossings project the ray per call, so mixed origins across a set are fine.
///
/// `structures_arrow` is the path the caller fingerprinted, not a fresh lookup:
/// the same file must decide the cache identity AND the obstacle ordinals.
/// Ids are dense in file order, one per geometry-carrying row, buildings and
/// walls sharing the one counter.
fn build_cell_index(cell: CellIndex, structures_arrow: &Path) -> Result<ObstacleIndex, String> {
    let centre = LatLng::from(cell);
    let mut builder = ObstacleIndex::builder(centre.lat(), centre.lng());
    let bytes = std::fs::read(structures_arrow)
        .map_err(|e| format!("read {}: {e}", structures_arrow.display()))?;
    // The cap lookup must be complete before ANY row is capped: the match is
    // spatial, so a first-rows-only lookup would miss neighbours further down
    // the file. Two streaming passes over the one in-memory read keep the old
    // loader's memory shape (a dense metro cell's table runs to ~1 GB).
    let low_profile = low_profile_from_structures(&bytes, structures_arrow)?;
    let reader = FileReader::try_new(Cursor::new(&bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", structures_arrow.display()))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches
            .push(batch.map_err(|e| format!("arrow batch {}: {e}", structures_arrow.display()))?);
    }
    for (idx, batch) in batches.iter().enumerate() {
        let c = batch
            .schema_ref()
            .metadata()
            .get("structures_contract")
            .map(String::as_str);
        if c != Some(crate::hex_store::STRUCTURES_CONTRACT_V1) {
            return Err(format!(
                "{}[batch {idx}]: structures_contract mismatch (expected {}, got {c:?}) — \
                 rebuild the cell with scripts/structures/build-structures.py",
                structures_arrow.display(),
                crate::hex_store::STRUCTURES_CONTRACT_V1,
            ));
        }
    }
    // The index inserts rows in `screening_ordinal` order (its dense ids follow
    // the sort): the engine's exact-δ tie resolution is scan-order sensitive,
    // and the migration's ordinals reproduce the legacy obstacles.arrow order.
    let mut index_rows: Vec<(u32, usize, usize)> = Vec::new();
    for (batch_idx, batch) in batches.iter().enumerate() {
        let wkb = batch
            .column_by_name("geometry_wkb")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geometry_wkb", structures_arrow.display()))?;
        let ordinals = batch
            .column_by_name("screening_ordinal")
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| format!("{}: missing screening_ordinal", structures_arrow.display()))?;
        for i in 0..batch.num_rows() {
            if wkb.is_null(i) {
                continue; // a geometry-less row screens nothing (the schema allows null)
            }
            if ordinals.is_null(i) {
                return Err(format!(
                    "{}: row {i} has geometry but no screening_ordinal",
                    structures_arrow.display()
                ));
            }
            index_rows.push((ordinals.value(i), batch_idx, i));
        }
    }
    index_rows.sort_unstable_by_key(|&(ordinal, _, _)| ordinal);
    let mut next_id: u32 = 0;
    for &(_, batch_idx, i) in &index_rows {
        let batch = &batches[batch_idx];
        let kinds = batch
            .column_by_name("kind")
            .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
            .ok_or_else(|| format!("{}: missing kind", structures_arrow.display()))?;
        let wkb = batch
            .column_by_name("geometry_wkb")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geometry_wkb", structures_arrow.display()))?;
        let heights = batch
            .column_by_name("height_m")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| format!("{}: missing height_m", structures_arrow.display()))?;
        let tiers = batch
            .column_by_name("height_tier")
            .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
        let clats = batch
            .column_by_name("centroid_lat")
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
        let clons = batch
            .column_by_name("centroid_lon")
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
        if heights.is_null(i) {
            return Err(format!(
                "{}: null height_m at row {i}",
                structures_arrow.display()
            ));
        }
        let id = next_id;
        match kinds.value(i) {
            STRUCTURE_KIND_BUILDING => {
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
                builder.add_polygon_wkb(wkb.value(i), height, ObstacleKind::Building, id, class);
            }
            // Walls keep their mapped height: the cap is a building-only
            // correction (noise_compute::low_profile caps tiers 2/4), and
            // add_polyline never clamps to the building height ceiling.
            STRUCTURE_KIND_BARRIER => builder.add_polyline(
                &noise_compute::wkb::parse_wkb_linestring_bytes(wkb.value(i)),
                heights.value(i),
                ObstacleKind::Barrier,
                id,
            ),
            other => {
                return Err(format!(
                    "{}: unknown structure kind {other} at row {i}",
                    structures_arrow.display()
                ));
            }
        }
        next_id = next_id.wrapping_add(1);
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
    h3r4_dir: &Path,
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
    for cell in cells {
        let Some(path) =
            locate_cell_structures(h3r4_dir, cell).map_err(|e| format!("structure_store: {e}"))?
        else {
            continue; // outside the prepared world — nothing to draw here
        };
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        // A cell whose cap cannot be read must not contribute footprints at
        // their uncapped height: a wrong number here is worse than an error.
        let low_profile = low_profile_from_structures(&bytes, &path)
            .map_err(|e| format!("structure_store: low-profile cap for {cell}: {e}"))?;
        let reader = FileReader::try_new(Cursor::new(&bytes), None)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        for batch in reader {
            let batch = batch.map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
            let (Some(kinds), Some(wkb), Some(heights), Some(clats), Some(clons)) = (
                batch
                    .column_by_name("kind")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
                batch
                    .column_by_name("geometry_wkb")
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
                // Walls are not footprints; the overlay draws buildings only.
                if kinds.value(i) != STRUCTURE_KIND_BUILDING {
                    continue;
                }
                if wkb.is_null(i) || heights.is_null(i) || clats.is_null(i) || clons.is_null(i) {
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
                for (outer, _holes) in noise_compute::wkb::parse_wkb_polygons_bytes(wkb.value(i)) {
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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_test_fixture::{square_polygon_wkb, wall_linestring_wkb, StructureRow};
    use tempfile::TempDir;

    /// Environment variables are PROCESS-global and `cargo test` runs tests in
    /// parallel threads, so two tests setting `QM_OBSTACLE_INDEX_DIR` read each
    /// other's value. Every test below that touches the environment takes this
    /// lock and restores what it found, and every one of them points the index
    /// cache at its OWN temp dir — the suite's answer must not depend on its
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

    /// The newest `…/data/prepared/<year>/h3r4` on this box, or `None` on a
    /// hermetic checkout. The year is the product's own pin
    /// (`scripts/dataset-year.json`), so it is read off the disk rather than
    /// written here twice.
    fn live_h3r4_dir() -> Option<PathBuf> {
        let prepared = Path::new("../../data/prepared");
        std::fs::read_dir(prepared)
            .ok()?
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| {
                name.as_encoded_bytes().len() == 4
                    && name.as_encoded_bytes().iter().all(u8::is_ascii_digit)
            })
            .max()
            .map(|year| prepared.join(year).join("h3r4"))
            .filter(|dir| dir.is_dir())
    }

    /// Runs only where the world obstacle store exists (dev boxes with the
    /// prepared tree); hermetic CI skips silently. Asserts the real scale and
    /// that the second load is a cache hit, not a rebuild.
    #[test]
    fn loads_praha_set_and_caches_cells() {
        let Some(h3r4) = live_h3r4_dir() else {
            return;
        };
        let data_dir = Path::new("../../data/prepared");
        // Its own index dir: this test must exercise the COLD build, and it
        // must not read (or evict) the box's production cache.
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let t0 = std::time::Instant::now();
        let Ok(set) = load_obstacle_set(&h3r4, data_dir, 50.08, 14.43) else {
            return; // this box's ring is not materialized yet — skip
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
        let set2 = load_obstacle_set(&h3r4, data_dir, 50.08, 14.43).expect("cached reload");
        let warm = t1.elapsed();
        assert_eq!(set2.edge_count(), set.edge_count());
        assert!(
            warm < cold / 5,
            "second load must be a cache hit: cold {cold:?}, warm {warm:?}"
        );
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

    /// One cell's structure table: a closed ~20 m square building footprint
    /// per entry, each with its south-west corner at the given `(lat, lon)`.
    /// An empty `squares` writes the 0-row table a swept-and-empty cell carries.
    fn write_structure_table(h3r4_dir: &Path, cell: CellIndex, squares: &[(f64, f64)]) -> PathBuf {
        let rows: Vec<StructureRow> = squares
            .iter()
            .map(|&(lat, lon)| StructureRow {
                kind: STRUCTURE_KIND_BUILDING,
                geometry_wkb: Some(square_polygon_wkb(lat, lon)),
                height_m: 9.0,
                centroid_lat: lat + 0.0001,
                centroid_lon: lon + 0.00015,
                ..Default::default()
            })
            .collect();
        crate::structure_test_fixture::write_structure_table(h3r4_dir, cell, &rows)
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
        let table = write_structure_table(tmp, cell, &[(50.08, 14.43)]);
        let root = tmp.join("index-cache");

        let data_ver = cell_data_ver(cell, &table).expect("input fingerprint");
        let built = build_cell_index(cell, &table).unwrap();
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

        // A table that moved under us must not be served from cache — and the
        // refused file is dropped so it cannot sit in the budget forever.
        let touched = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&table)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        let moved = cell_data_ver(cell, &table).expect("input fingerprint");
        assert_ne!(
            moved, data_ver,
            "an input mtime must rotate the fingerprint"
        );
        assert!(load_cached_index(&path, moved).is_none(), "stale file used");
        assert!(!path.exists(), "a refused cache file must be removed");

        // A second footprint is a different table, hence a different index.
        write_structure_table(tmp, cell, &[(50.08, 14.43), (50.081, 14.431)]);
        assert_ne!(cell_data_ver(cell, &table).unwrap(), moved);
    }

    /// EVERY input that shapes a cell's index must move its identity, and a
    /// file written under the old one must then be refused.
    ///
    /// This is the regression guard for 2026-08-05, when a cache served the
    /// answer to a different question. It is written as an ENUMERATION rather
    /// than one case on purpose: the defect class is "the key forgot
    /// something", so the test walks the closed list [`cell_data_ver`] actually
    /// folds — cell, structure TREE, the table's LENGTH and mtime, and
    /// [`CACHE_CODE_VER`]. The low-profile cap reads the same `structures.arrow`,
    /// so it needs no fold of its own.
    /// Adding an input to `build_cell_index` without a case here leaves the same
    /// hole, so the list is the review surface.
    ///
    /// NOT in the list, deliberately: table CONTENT. Rewriting the table to the
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
        let table = write_structure_table(tmp.path(), cell, &[(50.08, 14.43)]);
        let root = tmp.path().join("index-cache");
        let base = cell_data_ver(cell, &table).expect("input fingerprint");

        // Positive control FIRST: without it every `is_none()` below would
        // also pass on a key that is simply always wrong.
        let built = build_cell_index(cell, &table).unwrap();
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
        //    table under another cell is a different index.
        refuses(
            cell_data_ver(other_cell, &table).unwrap(),
            "a different cell",
        );

        // 2. The structure TREE: identical bytes at another path are another
        //    prepared root (a moved mount, a second data node) and may hold
        //    entirely different structures for this cell tomorrow.
        let root_b = TempDir::new().expect("temp dir");
        let table_b = write_structure_table(root_b.path(), cell, &[(50.08, 14.43)]);
        refuses(
            cell_data_ver(cell, &table_b).unwrap(),
            "another structure tree",
        );

        // 3. The table's mtime (the world-stamps.py staleness contract).
        let touched = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&table)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        refuses(cell_data_ver(cell, &table).unwrap(), "the table's mtime");

        // 4. The table's LENGTH — the one content signal the key carries,
        //    folded next to the mtime. Pinned at a FIXED mtime so it is the
        //    length alone doing the work, not case 3 again.
        let before_len = cell_data_ver(cell, &table).unwrap();
        std::fs::write(&table, b"a-table-of-a-very-different-length").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&table)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(touched))
            .unwrap();
        let after_len = cell_data_ver(cell, &table).unwrap();
        assert_ne!(
            after_len, before_len,
            "the table's length must rotate the identity at an unchanged mtime"
        );
        refuses(after_len, "the table's length");

        // 5. This file's own decisions (id ordering, the kind routing, the
        //    capping call) are in the version, on top of the builder's chain —
        //    which is what carries the low-profile rule now that it lives in
        //    `noise_compute::low_profile`.
        assert_eq!(
            CACHE_CODE_VER,
            fnv1a64(BUILDER_CODE_VER, include_bytes!("structure_store.rs"))
        );
        assert_ne!(
            CACHE_CODE_VER, BUILDER_CODE_VER,
            "the loader's own source must be in the version, not just the builder's"
        );
    }

    /// The process memo in front of the disk cache must key on the same
    /// identity the file does — 2026-08-05's live defect, where it keyed on the
    /// CELL alone and handed the second query the first query's index no matter
    /// which obstacle tree it asked about.
    ///
    /// Runs entirely through `cell_index`, the function the query path calls,
    /// because that is the layer that was wrong: the disk file always carried
    /// its fingerprint and would have refused these.
    #[test]
    fn process_memo_never_answers_for_another_obstacle_tree() {
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let one = TempDir::new().expect("temp dir");
        let two = TempDir::new().expect("temp dir");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);

        // Same cell, two prepared trees: one square in A, two in B.
        let table_a = write_structure_table(one.path(), cell, &[(50.08, 14.43)]);
        let table_b = write_structure_table(two.path(), cell, &[(50.08, 14.43), (50.081, 14.431)]);

        let data_dir = one.path().join("prepared");
        let edges = |table: &Path| {
            cell_index(cell, table, &data_dir)
                .expect("test table builds")
                .edge_count()
        };
        assert_eq!(edges(&table_a), 4, "one square is four edges");
        assert_eq!(
            edges(&table_b),
            8,
            "the memo served A's index for B's table — it is keyed on the \
             cell, not on the inputs"
        );
        assert_eq!(edges(&table_a), 4, "and back, without either poisoning");

        // A table changing under a LIVE process is the same hole with one
        // tree: the memo must notice, not hold yesterday's obstacles.
        write_structure_table(one.path(), cell, &[(50.08, 14.43), (50.081, 14.431)]);
        assert_eq!(edges(&table_a), 8, "a rewritten table must be picked up");
    }

    /// The three answers a ring cell can give, on the loader the popup calls.
    /// Conflating any two of them is the bug class this whole design exists to
    /// remove: an empty table is "nothing stands here", a missing table is
    /// undelivered data, and a cell the extract never produced is outside the
    /// world.
    #[test]
    fn empty_table_answers_nothing_stands_here_and_a_missing_one_is_an_error() {
        let tmp = TempDir::new().expect("temp dir");
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let h3r4 = tmp.path().join("h3r4");
        let data_dir = tmp.path().join("prepared");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let ring: Vec<CellIndex> = cell.grid_disk(1);

        write_structure_table(&h3r4, cell, &[(50.08, 14.43)]);
        for &neighbour in ring.iter().filter(|&&c| c != cell) {
            write_structure_table(&h3r4, neighbour, &[]);
        }
        let set = load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43)
            .expect("empty neighbours are an answer, not a gap");
        assert_eq!(set.indexes.len(), ring.len());
        assert_eq!(set.edge_count(), 4, "only the query cell holds a footprint");

        // A prepared cell whose table was not delivered must fail the query.
        let victim = *ring.iter().find(|&&c| c != cell).unwrap();
        std::fs::remove_file(h3r4.join(victim.to_string()).join("structures.arrow")).unwrap();
        assert!(
            load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43).is_err(),
            "a ring cell without its structure table is missing data, not empty"
        );

        // A cell the extract never produced has no directory: outside the
        // world, contributing nothing, exactly as it contributes no roads.
        std::fs::remove_dir_all(h3r4.join(victim.to_string())).unwrap();
        let partial = load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43)
            .expect("a cell outside the prepared world is not an error");
        assert_eq!(partial.indexes.len(), ring.len() - 1);
        assert_eq!(partial.edge_count(), 4);

        // The query cell itself is always prepared: without its table there is
        // no answer to give.
        std::fs::remove_file(h3r4.join(cell.to_string()).join("structures.arrow")).unwrap();
        assert!(
            load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43).is_err(),
            "the query cell's own table cannot be optional"
        );
    }

    /// The low-profile cap reads the SAME structures.arrow the index is built
    /// from: an OSM-attributed low-class row (building_type 7 = garage) caps a
    /// tier-2 defaulted 8 m footprint at its spot to 3 m, while a tier-0
    /// (mapped) 8 m footprint is per-building knowledge and never caps.
    #[test]
    fn build_applies_low_profile_cap_from_the_same_table() {
        let tmp = TempDir::new().expect("temp dir");
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let h3r4 = tmp.path().join("h3r4");
        let data_dir = tmp.path().join("prepared");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let square_area = noise_compute::wkb::outer_ring_area_m2(&square_polygon_wkb(50.08, 14.43));

        let garage = StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            geometry_wkb: Some(square_polygon_wkb(50.08, 14.43)),
            height_m: 3.0,
            height_tier: 0,
            centroid_lat: 50.0801,
            centroid_lon: 14.43015,
            osm_id: Some(101),
            building_type: Some(7),
            area_m2: Some(square_area),
            ..Default::default()
        };
        // The Overture-only twin at the same spot: tier 2 default, no osm_id.
        let defaulted = StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            geometry_wkb: Some(square_polygon_wkb(50.08, 14.43)),
            height_m: 8.0,
            height_tier: 2,
            centroid_lat: 50.0801,
            centroid_lon: 14.43015,
            ..Default::default()
        };
        // A mapped height (tier 0) is per-building knowledge: never capped,
        // even with a low-profile neighbour 400 m away being irrelevant here.
        let mapped = StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            geometry_wkb: Some(square_polygon_wkb(50.08, 14.4356)),
            height_m: 8.0,
            height_tier: 0,
            centroid_lat: 50.0801,
            centroid_lon: 14.43575,
            ..Default::default()
        };
        crate::structure_test_fixture::write_structure_table(
            &h3r4,
            cell,
            &[garage, defaulted, mapped],
        );
        let set = load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43).expect("set loads");

        let capped = point_inside_obstacle(&set, 50.0801, 14.43015).expect("inside the twin pair");
        assert!(
            (capped - 3.0).abs() < 0.1,
            "tier-2 default next to a garage must cap to 3 m, got {capped}"
        );
        let kept = point_inside_obstacle(&set, 50.0801, 14.43575).expect("inside the mapped row");
        assert!(
            (kept - 8.0).abs() < 0.1,
            "a tier-0 height never caps, got {kept}"
        );
    }

    /// Wall rows (kind=1) index as Barrier polylines from the SAME table, at
    /// their stored height — the low-profile cap is building-only, so a tier-2
    /// wall beside a garage keeps its 8 m.
    #[test]
    fn walls_index_as_uncapped_barrier_edges() {
        let tmp = TempDir::new().expect("temp dir");
        let index_dir = TempDir::new().expect("temp index dir");
        let _env = EnvGuard::set(&[("QM_OBSTACLE_INDEX_DIR", Some(&path_str(&index_dir)))]);
        let h3r4 = tmp.path().join("h3r4");
        let data_dir = tmp.path().join("prepared");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);

        let garage = StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            geometry_wkb: Some(square_polygon_wkb(50.08, 14.43)),
            height_m: 3.0,
            centroid_lat: 50.0801,
            centroid_lon: 14.43015,
            osm_id: Some(101),
            building_type: Some(7),
            area_m2: Some(100.0),
            ..Default::default()
        };
        // A north-south wall at lon 14.4301, tier 2 and 8 m: kind-1 rows route
        // to add_polyline at their stored height, never through the
        // building-only low-profile cap.
        let wall = StructureRow {
            kind: STRUCTURE_KIND_BARRIER,
            geometry_wkb: Some(wall_linestring_wkb((50.0798, 14.4301), (50.0802, 14.4301))),
            height_m: 8.0,
            height_tier: 2,
            centroid_lat: 50.08,
            centroid_lon: 14.4301,
            osm_id: Some(555),
            segment_idx: Some(0),
            ..Default::default()
        };
        crate::structure_test_fixture::write_structure_table(&h3r4, cell, &[garage, wall]);
        let set = load_obstacle_set(&h3r4, &data_dir, 50.08, 14.43).expect("set loads");

        let mut out = Vec::new();
        set.crossings(50.0801, 14.4295, 50.0801, 14.4310, &mut out);
        let wall_hit = out
            .iter()
            .find(|c| c.kind == ObstacleKind::Barrier)
            .expect("the wall must screen as a barrier edge");
        assert_eq!(wall_hit.height_m, 8.0, "walls are never low-profile capped");
        assert!(
            out.iter().any(|c| c.kind == ObstacleKind::Building),
            "the building crosses the same ray as a building"
        );
    }
}
