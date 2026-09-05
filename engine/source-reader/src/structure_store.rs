//! Vector structure loading for the popup.
//!
//! Each query assembles an
//! [`ObstacleSet`] from per-square [`ObstacleIndex`]es covering the query
//! source envelope, with centroid-assigned footprints from the structure builder.
//! One `structures.arrow` per square carries BOTH screening stocks — buildings
//! (kind 0, polygons) and noise walls (kind 1, polyline microsegments, indexed
//! as [`ObstacleKind::Barrier`] edges) — and, in its OSM-attributed rows, the
//! input of the low-profile height cap's lookup. One table, one read.
//!
//! Two hard rules:
//! - **Bounded cost.** Per-square indexes are built ONCE per process and
//!   LRU-cached (`SQUARE_CACHE_CAP`); each query Arc-clones the selected indexes.
//!   The naive per-query rebuild measured 448 MB RSS / 0.47 s per popup.
//! - **All-or-error.** Any read/parse error, and any selected square of the
//!   prepared world whose `structures.arrow` is missing, aborts the whole load.
//!   A partial index would silently under-screen the path. Emptiness is not a
//!   gap: a 0-row table is the answer "nothing stands here".
//!
//! Built indexes are also kept ON DISK (`noise_compute::propagation::obstacle_index_file`)
//! and mapped back on the next cold start — a São Paulo popup indexes 40 M
//! edges from ~1 GB of Arrow, which cost ~6 s of the FIRST click and was then
//! thrown away with the process. The cached file is the in-memory layout, so a
//! reload is an `mmap` plus a header check and the kernel faults in only the
//! grid cells the rays walk.
//!
//! **Both caches key on [`square_data_ver`], the full identity of the index**
//! — never on the square. The memo in front of the disk cache once keyed on the
//! cell alone and so answered a query about one obstacle tree with an index
//! built from another (2026-08-05); the popup is the project's acoustic
//! reference, and a cache that returns the answer to a different question is
//! worse than no cache. The ring is deliberately NOT in that key: it decides
//! which squares are assembled into a set, per query, in [`load_obstacle_set`],
//! while a cache entry is one square's index built from that square's own file.

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use arrow::array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, UInt32Array, UInt8Array,
};
use arrow::ipc::reader::FileReader;
use grid::Square;
use noise_compute::envelope::{effective_envelope_class, EnvelopeClass};
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::propagation::obstacle_index_file::{fnv1a64, IndexBlob, BUILDER_CODE_VER};

use crate::query::squares_within_reach;
use square_store::grid_cols::{decode_geom, ring_lonlat};
use square_store::store::{STRUCTURE_KIND_BARRIER, STRUCTURE_KIND_BUILDING};
use square_store::structure_contract;

/// Per-square index cache capacity. A dense metro square's index runs to low
/// hundreds of MB; popups cluster spatially, so a small LRU covers the
/// active area while bounding worst-case RSS.
const SQUARE_CACHE_CAP: usize = 8;

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

/// Disk budget for the cached indexes. One dense metro square is a few hundred
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

/// Short stable file tag for a square: `z9_276_173`.
fn square_tag(square: Square) -> String {
    format!("z9_{}_{}", square.x, square.y)
}

/// The FULL identity of one square's index — everything that decides its bytes,
/// in one u64. Both caches (the process memo and the file on disk) key on it,
/// and nothing may be served under a key that does not carry all of:
///
/// * [`CACHE_CODE_VER`] — the builder, the grid, this loader's own rules;
/// * the SQUARE, whose centre is the index's metric origin (and which is the
///   only thing the file name would otherwise bind);
/// * the square's `structures.arrow` as (path, length, mtime) — the path because
///   two prepared TREES (a moved mount, a second checkout's data node) hold
///   different structures for the same square. The low-profile cap reads the
///   SAME file, so it needs no fold of its own.
///
/// That list is closed by construction: `build_square_index` reads its square and
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
fn square_data_ver(square: Square, structures_arrow: &Path) -> Option<u64> {
    let mut h = fnv1a64(CACHE_CODE_VER, b"structure-index-inputs-v1");
    h = fnv1a64(h, &square.x.to_le_bytes());
    h = fnv1a64(h, &square.y.to_le_bytes());
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

/// `<square>.<code_ver>.qoix`. The builder version is in the NAME, not just the
/// header, because prod and dev checkouts share one `prepared/` node: two checkouts on
/// different engine versions would otherwise fight over one path, each deleting
/// and rebuilding the other's file forever. Superseded versions are ordinary
/// cache files and age out through the LRU budget.
fn cache_file_path(root: &Path, square: Square) -> PathBuf {
    root.join(format!(
        "{}.{CACHE_CODE_VER:016x}.{CACHE_FILE_EXT}",
        square_tag(square)
    ))
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
fn store_cached_index(root: &Path, square: Square, index: &ObstacleIndex, data_ver: u64) {
    let parts = index.file_parts(CACHE_CODE_VER, data_ver);
    let total = parts.total_len() as u64;
    if let Err(e) = std::fs::create_dir_all(root) {
        eprintln!("structure_store: no index cache at {}: {e}", root.display());
        return;
    }
    evict_to_budget(root, total);
    let final_path = cache_file_path(root, square);
    // Same-directory tmp + rename: a reader either maps the whole previous
    // file or the whole new one, never a half-written index. Two NAPI worker
    // threads can miss the process LRU on the SAME square at the same time, so
    // the temp name carries a per-write sequence — sharing one `<pid>.tmp`
    // would let them interleave into a file that then passes the header check.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = root.join(format!(
        "{}.{}.{seq}.tmp",
        square_tag(square),
        std::process::id()
    ));
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
/// file carries ([`square_data_ver`]) — not on the square.
///
/// Keying it on the square alone was a live defect (2026-08-05): the second
/// query for a square got the first query's index no matter which obstacle tree
/// it asked about, so a run against a second prepared root screened the popup
/// against obstacles that are not there. The disk cache never had this hole —
/// its header carries the fingerprint — which is exactly why the memo in front
/// of it had to grow one.
struct SquareCache {
    /// (square, identity) → (index, LRU stamp). Failed builds are NOT cached —
    /// transient IO must stay retryable; missing squares stay a per-query
    /// decision.
    map: HashMap<(Square, u64), (Arc<ObstacleIndex>, u64)>,
    stamp: u64,
}

static SQUARE_CACHE: OnceLock<Mutex<SquareCache>> = OnceLock::new();

fn memo() -> &'static Mutex<SquareCache> {
    SQUARE_CACHE.get_or_init(|| {
        Mutex::new(SquareCache {
            map: HashMap::new(),
            stamp: 0,
        })
    })
}

fn memo_get(square: Square, ver: u64) -> Option<Arc<ObstacleIndex>> {
    let mut c = memo().lock().unwrap_or_else(|e| e.into_inner());
    c.stamp += 1;
    let stamp = c.stamp;
    let (idx, touched) = c.map.get_mut(&(square, ver))?;
    *touched = stamp;
    Some(Arc::clone(idx))
}

fn memo_put(square: Square, ver: u64, idx: &Arc<ObstacleIndex>) {
    let mut c = memo().lock().unwrap_or_else(|e| e.into_inner());
    c.stamp += 1;
    let stamp = c.stamp;
    if c.map.len() >= SQUARE_CACHE_CAP {
        if let Some((&evict, _)) = c.map.iter().min_by_key(|(_, (_, t))| *t) {
            c.map.remove(&evict);
        }
    }
    c.map.insert((square, ver), (Arc::clone(idx), stamp));
}

/// Locate one square's structure table. `None` = the square directory does not
/// exist, i.e. outside the prepared world: it holds no structures for the same
/// reason it holds no roads.
fn locate_square_structures(prepared_year_dir: &Path, square: Square) -> Option<PathBuf> {
    let dir = prepared_year_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string());
    if !dir.exists() {
        return None;
    }
    Some(dir.join("structures.arrow"))
}

/// Assemble the query's [`ObstacleSet`], or fail when vector coverage cannot
/// be proved complete.
pub fn load_obstacle_set(
    prepared_year_dir: &Path,
    data_dir: &Path,
    lat: f64,
    lon: f64,
) -> Result<ObstacleSet, String> {
    let mut indexes = Vec::new();
    for square in squares_within_reach(lat, lon)? {
        let Some(structures_arrow) = locate_square_structures(prepared_year_dir, square) else {
            // Outside the prepared world: no square directory at all, so it holds
            // no structures for the same reason it holds no roads.
            continue;
        };
        match square_index(square, &structures_arrow, data_dir) {
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

/// Preserve clicked enclosure metadata while selecting the point used by every source gate.
pub fn locate_facade_receiver(
    obstacle_set: &ObstacleSet,
    lat: f64,
    lng: f64,
) -> (f64, f64, Option<EnclosedEnvelopeWinner>) {
    let inside_envelope = point_inside_enclosed(obstacle_set, lat, lng);
    let (facade_lat, facade_lng) = if inside_envelope.is_some() {
        let step_lat = 1.0 / grid::geo::M_PER_DEG_LAT;
        let step_lon = 1.0 / grid::geo::m_per_deg_lon(lat.to_radians());
        let mut outside = None;
        for distance in 1..=100 {
            for (dy, dx) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                let candidate = (
                    lat + dy * distance as f64 * step_lat,
                    lng + dx * distance as f64 * step_lon,
                );
                if point_inside_enclosed(obstacle_set, candidate.0, candidate.1).is_none() {
                    outside = Some(candidate);
                    break;
                }
            }
            if outside.is_some() {
                break;
            }
        }
        outside.unwrap_or((lat, lng))
    } else {
        (lat, lng)
    };
    (facade_lat, facade_lng, inside_envelope)
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

/// One square's index, from the nearest source that still holds it: the process
/// memo, then the on-disk index cache, then a rebuild from the square's Arrow
/// table (which is also written back). Build errors are not cached; successful
/// builds are immutable and shared.
///
/// The inputs are fingerprinted BEFORE either cache is consulted: both are
/// keyed on that fingerprint, so a hit is only ever the index this process
/// would have built from these very files. Two `stat`s per square per query —
/// three orders below the rebuild they guard, and the price of a cache that
/// answers the question it was asked.
fn square_index(
    square: Square,
    structures_arrow: &Path,
    data_dir: &Path,
) -> Result<Arc<ObstacleIndex>, String> {
    let ver = square_data_ver(square, structures_arrow);
    let t0 = std::time::Instant::now();
    if let Some(ver) = ver {
        if let Some(idx) = memo_get(square, ver) {
            return Ok(idx);
        }
        if let Some(root) = index_cache_root(data_dir) {
            if let Some(idx) = load_cached_index(&cache_file_path(&root, square), ver) {
                let idx = Arc::new(idx);
                log_square_load(square, "mapped", idx.edge_count(), t0);
                memo_put(square, ver, &idx);
                return Ok(idx);
            }
        }
    }

    let built = Arc::new(build_square_index(square, structures_arrow)?);
    // No fingerprint ⇒ no memo and no file. An index whose inputs could not be
    // pinned is used for THIS query and forgotten.
    if let Some(ver) = ver {
        if let Some(root) = index_cache_root(data_dir) {
            store_cached_index(&root, square, &built, ver);
        }
        memo_put(square, ver, &built);
    }
    log_square_load(square, "built", built.edge_count(), t0);
    Ok(built)
}

/// Per-square provenance under `POPUP_TIMING=1` — the same lever
/// `query_noise_impl` uses for its stage timings. `mapped` vs `built` is the
/// entire difference this cache makes, so it belongs in one log line instead of
/// being inferred from a wall clock that also carries the Arrow square load.
fn log_square_load(square: Square, how: &str, edges: usize, t0: std::time::Instant) {
    if std::env::var("POPUP_TIMING").as_deref() == Ok("1") {
        eprintln!(
            "obstacle-index {how} square={} edges={edges} in {:.0} ms",
            square_tag(square),
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
fn low_profile_from_structures(bytes: &[u8], label: &Path) -> Result<LowProfileLookup, String> {
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", label.display()))?;
    structure_contract::validate_schema(reader.schema().as_ref())?;
    let mut lookup = LowProfileLookup::default();
    for batch in reader {
        let batch = batch.map_err(|e| format!("arrow batch {}: {e}", label.display()))?;
        let (Some(kinds), Some(osm_ids), Some(cgxs), Some(cgys), Some(types), Some(areas)) = (
            batch
                .column_by_name("kind")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("osm_id")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>()),
            batch
                .column_by_name("centroid_gx")
                .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            batch
                .column_by_name("centroid_gy")
                .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            batch
                .column_by_name("building_type")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
        ) else {
            return Err(format!(
                "{}: missing current structure emission columns",
                label.display()
            ));
        };
        // The merge moves an OSM-matched row's screening centroid to the
        // Overture footprint's; the OSM one survives as emission_centroid_*.
        let egxs = batch
            .column_by_name("emission_centroid_gx")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        let egys = batch
            .column_by_name("emission_centroid_gy")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        for i in 0..batch.num_rows() {
            if kinds.value(i) != STRUCTURE_KIND_BUILDING
                || osm_ids.is_null(i)
                || types.is_null(i)
                || areas.is_null(i)
            {
                continue;
            }
            let (gx, gy) = match (egxs, egys) {
                (Some(egxs), Some(egys)) if !egxs.is_null(i) && !egys.is_null(i) => {
                    (egxs.value(i), egys.value(i))
                }
                _ if !cgxs.is_null(i) && !cgys.is_null(i) => (cgxs.value(i), cgys.value(i)),
                _ => continue,
            };
            let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
            lookup.insert_if_low(types.value(i), lat, lon, areas.value(i));
        }
    }
    Ok(lookup)
}

/// Metric origin of one square's index: the square centre, so the cache entry
/// is query-independent; crossings project the ray per call, so mixed origins
/// across a set are fine.
fn square_center_latlon(square: Square) -> (f64, f64) {
    use grid::{EARTH_CIRCUMFERENCE_M, WEB_MERCATOR_RADIUS_M, Z9_TILES_PER_AXIS};
    use std::f64::consts::PI;
    let axis = f64::from(Z9_TILES_PER_AXIS);
    let lon = (f64::from(square.x) + 0.5) / axis * 360.0 - 180.0;
    let half = EARTH_CIRCUMFERENCE_M / 2.0;
    let y_m = half - (f64::from(square.y) + 0.5) / axis * EARTH_CIRCUMFERENCE_M;
    let lat = (2.0 * (y_m / WEB_MERCATOR_RADIUS_M).exp().atan() - PI / 2.0).to_degrees();
    (lat, lon)
}

/// Build one square's index from its structure table.
///
/// `structures_arrow` is the path the caller fingerprinted, not a fresh lookup:
/// the same file must decide the cache identity AND the obstacle ordinals.
/// Ids are dense in file order, one per geometry-carrying row, buildings and
/// walls sharing the one counter.
fn build_square_index(square: Square, structures_arrow: &Path) -> Result<ObstacleIndex, String> {
    let (origin_lat, origin_lon) = square_center_latlon(square);
    let mut builder = ObstacleIndex::builder(origin_lat, origin_lon);
    let bytes = std::fs::read(structures_arrow)
        .map_err(|e| format!("read {}: {e}", structures_arrow.display()))?;
    // The cap lookup must be complete before ANY row is capped: the match is
    // spatial, so a first-rows-only lookup would miss neighbours further down
    // the file. Two streaming passes over the one in-memory read keep the old
    // loader's memory shape (a dense metro square's table runs to ~1 GB).
    let low_profile = low_profile_from_structures(&bytes, structures_arrow)?;
    let reader = FileReader::try_new(Cursor::new(&bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", structures_arrow.display()))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches
            .push(batch.map_err(|e| format!("arrow batch {}: {e}", structures_arrow.display()))?);
    }
    let batch_heights: Vec<_> = batches
        .iter()
        .map(structure_contract::heights)
        .collect::<Result<_, _>>()?;
    // The index inserts rows in `screening_ordinal` order (its dense ids follow
    // the sort): the engine's exact-δ tie resolution is scan-order sensitive,
    // and the migration's ordinals reproduce the legacy obstacles.arrow order.
    let mut index_rows: Vec<(u32, usize, usize)> = Vec::new();
    for (batch_idx, batch) in batches.iter().enumerate() {
        let geom = batch
            .column_by_name("geom")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geom", structures_arrow.display()))?;
        let ordinals = batch
            .column_by_name("screening_ordinal")
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| format!("{}: missing screening_ordinal", structures_arrow.display()))?;
        for i in 0..batch.num_rows() {
            if geom.is_null(i) {
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
        let geom = batch
            .column_by_name("geom")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geom", structures_arrow.display()))?;
        let heights = batch_heights[batch_idx];
        let tiers = batch
            .column_by_name("height_tier")
            .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
        let cgxs = batch
            .column_by_name("centroid_gx")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        let cgys = batch
            .column_by_name("centroid_gy")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        let area_col = batch
            .column_by_name("area_m2")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
        let ring = decode_geom(Some(geom.value(i))).ok_or_else(|| {
            format!(
                "{}: row {i} geom is not a grid ring",
                structures_arrow.display()
            )
        })?;
        let id = next_id;
        match kinds.value(i) {
            STRUCTURE_KIND_BUILDING => {
                let mut height = f32::from(heights.value(i));
                if let (Some(tiers), Some(cgxs), Some(cgys)) = (tiers, cgxs, cgys) {
                    if !tiers.is_null(i) && !cgxs.is_null(i) && !cgys.is_null(i) {
                        let (clon, clat) =
                            square_store::grid_cols::grid_cell_lonlat(cgxs.value(i), cgys.value(i));
                        let area = area_col
                            .filter(|a| !a.is_null(i))
                            .map(|a| a.value(i))
                            .or_else(|| grid::poly::ring_area_m2(&ring).map(|a| a as f32))
                            .unwrap_or(0.0);
                        height =
                            low_profile.capped_height(height, tiers.value(i), clat, clon, area);
                    }
                }
                let class = batch
                    .column_by_name("envelope_class")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
                    .filter(|a| !a.is_null(i))
                    .map(|a| EnvelopeClass::from_u8(a.value(i)))
                    .unwrap_or(EnvelopeClass::Default);
                // Grid rings reach the index through the same WKB ingestion
                // every other loader uses, so the envelope class travels with
                // the footprint exactly as before.
                let wkb = crate::query::grid_ring_to_wkb_polygon_pub(&ring_lonlat(&ring));
                builder.add_polygon_wkb(&wkb, height, ObstacleKind::Building, id, class);
            }
            // Walls keep their mapped height: the cap is a building-only
            // correction (noise_compute::low_profile caps tiers 2/4), and
            // add_polyline never clamps to the building height ceiling.
            STRUCTURE_KIND_BARRIER => {
                if ring.len() < 2 {
                    return Err(format!(
                        "{}: row {i} geom is not a wall microsegment",
                        structures_arrow.display()
                    ));
                }
                let pts: Vec<(f64, f64)> = ring_lonlat(&ring)
                    .into_iter()
                    .map(|(lon, lat)| (lat, lon))
                    .collect();
                builder.add_polyline(&pts, f32::from(heights.value(i)), ObstacleKind::Barrier, id);
            }
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

/// One footprint as the MODEL uses it (display twin of `build_square_index`):
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

/// Footprints within the existing padded-centroid display gate, at as-used heights.
/// Enumerate every owner in that gate, including the viewport interior.
///
/// This overlay draws what the engine screens with, so it follows the physics
/// loader's rule rather than a softer one: a square that is provably empty
/// contributes nothing, and anything ELSE that stops us reading it is an error.
/// Returning an empty list on a broken shard would paint a transparent tile,
/// and on a noise map an absent building is indistinguishable from a quiet
/// place (raised in review, 2026-08-30).
pub fn footprints_in_bbox(
    prepared_year_dir: &Path,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> Result<Vec<FootprintView>, String> {
    let longitude_span = if west > east {
        east + 360.0 - west
    } else {
        east - west
    };
    let center_lon = west + longitude_span / 2.0;
    let pad = 0.01;
    let squares = grid::bounds::BoundedSquares::from_degrees(
        south - pad,
        west - pad,
        north + pad,
        west + longitude_span + pad,
    )
    .ok_or_else(|| "invalid footprint query bounds".to_string())?;
    let mut out = Vec::new();
    for square in squares.iter() {
        let Some(path) = locate_square_structures(prepared_year_dir, square) else {
            continue; // outside the prepared world — nothing to draw here
        };
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        // A square whose cap cannot be read must not contribute footprints at
        // their uncapped height: a wrong number here is worse than an error.
        let low_profile = low_profile_from_structures(&bytes, &path).map_err(|e| {
            format!(
                "structure_store: low-profile cap for {}: {e}",
                square_tag(square)
            )
        })?;
        let reader = FileReader::try_new(Cursor::new(&bytes), None)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        for batch in reader {
            let batch = batch.map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
            let heights = structure_contract::heights(&batch)?;
            let (Some(kinds), Some(geom), Some(cgxs), Some(cgys)) = (
                batch
                    .column_by_name("kind")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
                batch
                    .column_by_name("geom")
                    .and_then(|c| c.as_any().downcast_ref::<BinaryArray>()),
                batch
                    .column_by_name("centroid_gx")
                    .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
                batch
                    .column_by_name("centroid_gy")
                    .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            ) else {
                continue;
            };
            let tiers = batch
                .column_by_name("height_tier")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
            let area_col = batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
            for i in 0..batch.num_rows() {
                // Walls are not footprints; the overlay draws buildings only.
                if kinds.value(i) != STRUCTURE_KIND_BUILDING {
                    continue;
                }
                if geom.is_null(i) || heights.is_null(i) || cgxs.is_null(i) || cgys.is_null(i) {
                    continue;
                }
                let (clon, clat) =
                    square_store::grid_cols::grid_cell_lonlat(cgxs.value(i), cgys.value(i));
                if clat < south - pad
                    || clat > north + pad
                    || grid::geo::wrapped_longitude_delta(center_lon, clon).abs()
                        > longitude_span / 2.0 + pad
                {
                    continue;
                }
                let Some(ring) = decode_geom(Some(geom.value(i))) else {
                    continue;
                };
                let raw = f32::from(heights.value(i));
                let tier = tiers.map(|t| t.value(i)).unwrap_or(0);
                let area = area_col
                    .filter(|a| !a.is_null(i))
                    .map(|a| a.value(i))
                    .or_else(|| grid::poly::ring_area_m2(&ring).map(|a| a as f32))
                    .unwrap_or(0.0);
                let height = low_profile.capped_height(raw, tier, clat, clon, area);
                out.push(FootprintView {
                    outer: ring_lonlat(&ring)
                        .into_iter()
                        .map(|(lon, lat)| (lat, lon))
                        .collect(),
                    height_m: height,
                    tier,
                    capped: height < raw,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "structure_producer_tests.rs"]
mod producer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_test_fixture as fx;
    use tempfile::TempDir;

    /// Environment variables are PROCESS-global and `cargo test` runs tests in
    /// parallel threads, so every test below that touches the environment takes
    /// this lock and restores what it found, and every one of them points the
    /// index cache at its OWN temp dir — the suite's answer must not depend on
    /// its order.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    fn prague() -> Square {
        grid::square_of(LAT, LON)
    }

    fn house_row() -> fx::StructureRow {
        fx::StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            height_m: 12,
            height_tier: 0,
            envelope_class: 1, // Residential
            centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
            osm_id: Some(7),
            building_type: Some(1),
            area_m2: Some(450.0),
            ..Default::default()
        }
    }

    /// Query against a fixture tree without touching the shared disk cache.
    fn obstacle_set(year: &Path) -> ObstacleSet {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("QM_OBSTACLE_INDEX_CACHE", "0");
        let set = load_obstacle_set(year, year, LAT, LON).unwrap();
        std::env::remove_var("QM_OBSTACLE_INDEX_CACHE");
        set
    }

    #[test]
    fn enclosed_winner_reports_stored_class_and_height() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(tmp.path(), prague(), &[house_row()]);
        let set = obstacle_set(tmp.path());
        assert!(!set.indexes.is_empty());
        let winner = point_inside_enclosed(&set, LAT + 0.0001, LON + 0.0001)
            .expect("click inside the footprint must be enclosed");
        assert_eq!(winner.stored_class, EnvelopeClass::Residential);
        assert!(
            (winner.height_m - 12.0).abs() < 0.5,
            "h={}",
            winner.height_m
        );
        assert!(point_inside_enclosed(&set, LAT + 0.5, LON + 0.5).is_none());
    }

    #[test]
    fn hover_winner_names_outdoor_footprints_that_indoor_ignores() {
        let tmp = TempDir::new().unwrap();
        let mut row = house_row();
        row.envelope_class = 0; // Outdoor carport
        fx::write_square_structures(tmp.path(), prague(), &[row]);
        let set = obstacle_set(tmp.path());
        let (class, _) = point_inside_footprint(&set, LAT + 0.0001, LON + 0.0001)
            .expect("hover must see the carport");
        assert_eq!(class, EnvelopeClass::Outdoor);
        assert!(point_inside_enclosed(&set, LAT + 0.0001, LON + 0.0001).is_none());
    }

    #[test]
    fn wall_rows_index_without_becoming_footprints() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(
            tmp.path(),
            prague(),
            &[fx::StructureRow {
                kind: STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(LON, LAT), (LON + 0.001, LAT + 0.001)]),
                height_m: 3,
                height_tier: 0,
                envelope_class: 0,
                centroid_lonlat: Some((LON + 0.0005, LAT + 0.0005)),
                osm_id: Some(11),
                segment_idx: Some(2),
                ..Default::default()
            }],
        );
        let set = obstacle_set(tmp.path());
        assert!(!set.indexes.is_empty());
        // A wall is not a footprint: neither probe fires on its midpoint.
        assert!(point_inside_footprint(&set, LAT + 0.0005, LON + 0.0005).is_none());
        let fps =
            footprints_in_bbox(tmp.path(), LAT - 0.01, LON - 0.01, LAT + 0.01, LON + 0.01).unwrap();
        assert!(fps.is_empty());
    }

    #[test]
    fn dateline_wall_stays_local_after_structures_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(0.0, -180.0);
        fx::write_square_structures(
            tmp.path(),
            square,
            &[fx::StructureRow {
                kind: STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(179.999, 0.0), (-179.999, 0.0)]),
                height_m: 3,
                height_tier: 0,
                envelope_class: 0,
                centroid_lonlat: Some((-180.0, 0.0)),
                osm_id: Some(11),
                segment_idx: Some(0),
                ..Default::default()
            }],
        );

        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let set = load_obstacle_set(tmp.path(), tmp.path(), 0.0, 179.9).unwrap();
        assert_eq!(set.edge_count(), 1);
        let view = set
            .indexes
            .iter()
            .map(|index| index.gpu_view())
            .find(|view| !view.edges_xyxyh.is_empty())
            .expect("wall index");
        let wall_length_m = f64::from((view.edges_xyxyh[2] - view.edges_xyxyh[0]).abs());
        assert!(
            (wall_length_m - 222.64).abs() < 0.5,
            "wall length {wall_length_m} m"
        );
        assert!(
            view.cols <= 4,
            "short wall grew to {} grid columns",
            view.cols
        );

        for lon in [179.8, -179.8] {
            let mut hits = Vec::new();
            set.crossings(-0.01, lon, 0.01, lon, &mut hits);
            assert!(
                hits.is_empty(),
                "phantom world-spanning wall at {lon}: {hits:?}"
            );
        }
        for lon in [179.9995, -179.9995] {
            let mut hits = Vec::new();
            set.crossings(-0.01, lon, 0.01, lon, &mut hits);
            assert_eq!(hits.len(), 1, "seam wall missing at {lon}: {hits:?}");
            assert_eq!(hits[0].kind, ObstacleKind::Barrier);
            assert_eq!(hits[0].height_m, 3.0);
            assert!((hits[0].t - 0.5).abs() < 0.001, "t={}", hits[0].t);
        }
    }

    #[test]
    fn footprints_carry_as_used_height_and_ring() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(tmp.path(), prague(), &[house_row()]);
        let fps =
            footprints_in_bbox(tmp.path(), LAT - 0.01, LON - 0.01, LAT + 0.01, LON + 0.01).unwrap();
        assert_eq!(fps.len(), 1);
        assert!(
            (fps[0].height_m - 12.0).abs() < 0.5,
            "h={}",
            fps[0].height_m
        );
        assert_eq!(fps[0].tier, 0);
        assert!(!fps[0].capped);
        assert_eq!(fps[0].outer.len(), 5);
        assert!((fps[0].outer[0].0 - LAT).abs() < 0.0001);
        assert!((fps[0].outer[0].1 - LON).abs() < 0.0001);
    }

    #[test]
    fn viewport_interior_footprints_are_not_limited_to_corner_and_center_owners() {
        let tmp = TempDir::new().unwrap();
        let mut row = house_row();
        row.ring_lonlat = Some(fx::square_ring_lonlat(2.0, 5.0));
        row.centroid_lonlat = Some((5.0001, 2.0001));
        fx::write_square_structures(tmp.path(), grid::square_of(2.0001, 5.0001), &[row]);
        let footprints = footprints_in_bbox(tmp.path(), 0.0, 0.0, 10.0, 10.0).unwrap();
        assert_eq!(footprints.len(), 1);
        assert_eq!(footprints[0].height_m, 12.0);
        assert!(footprints_in_bbox(tmp.path(), 0.0, 0.0, 1.0, 1.0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dateline_footprint_survives_both_overlay_tile_queries() {
        for centroid_lon in [-179.9998, 179.9998] {
            let tmp = TempDir::new().unwrap();
            let mut row = house_row();
            row.centroid_lonlat = Some((centroid_lon, 10.0005));
            row.ring_lonlat = Some(vec![
                (179.999, 10.0),
                (-179.999, 10.0),
                (-179.999, 10.001),
                (179.999, 10.001),
                (179.999, 10.0),
            ]);
            fx::write_square_structures(tmp.path(), grid::square_of(10.0005, centroid_lon), &[row]);
            for (west, east) in [(179.99, 180.0), (-180.0, -179.99), (179.99, -179.99)] {
                let footprints = footprints_in_bbox(tmp.path(), 9.99, west, 10.01, east).unwrap();
                assert_eq!(
                    footprints.len(),
                    1,
                    "centroid {centroid_lon}, bbox {west}..{east}"
                );
                assert_eq!(footprints[0].height_m, 12.0);
                assert_eq!(footprints[0].outer.len(), 5);
            }
            assert!(
                footprints_in_bbox(tmp.path(), 9.99, 179.96, 10.01, 179.97)
                    .unwrap()
                    .is_empty(),
                "longitude padding must remain bounded"
            );
        }
    }

    #[test]
    fn missing_square_dir_is_empty_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("QM_OBSTACLE_INDEX_CACHE", "0");
        let set = load_obstacle_set(tmp.path(), tmp.path(), LAT, LON).unwrap();
        std::env::remove_var("QM_OBSTACLE_INDEX_CACHE");
        assert!(set.indexes.is_empty());
    }

    #[test]
    fn unstamped_table_fails_the_query() {
        let tmp = TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_structure_file(&dir.join("structures.arrow"), &[house_row()], false);
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("QM_OBSTACLE_INDEX_CACHE", "0");
        let err = match load_obstacle_set(tmp.path(), tmp.path(), LAT, LON) {
            Ok(_) => panic!("unstamped table must fail"),
            Err(e) => e,
        };
        std::env::remove_var("QM_OBSTACLE_INDEX_CACHE");
        assert!(err.contains("structures_contract mismatch"), "got: {err}");
    }

    #[test]
    fn square_center_is_query_independent_and_sane() {
        let (lat, lon) = square_center_latlon(prague());
        assert!((lon - 14.24).abs() < 0.4, "lon={lon}");
        assert!((lat - 50.1).abs() < 0.4, "lat={lat}");
    }
}
