//! `structures.qoix`: one square's prebuilt obstacle index, written by the
//! pipeline beside `structures.arrow` and mapped by the popup and the painter.
//!
//! The pipeline step (`structures-finalize`, right after `structures`; see
//! `structures_finalize`) calls [`write_square_obstacle_index`] per square with
//! the final table bytes; readers call [`load_square_obstacle_index`]. A square
//! without `structures.arrow` has no index and no obstacles. For the painter and
//! the pipeline a `structures.arrow` without a valid, current `structures.qoix`
//! is an error naming the step to rerun. The popup
//! ([`load_square_obstacle_index_or_build_it_in_process`]) builds the index from
//! the table instead (0.4–18 s on the first click) and warns once per square: a
//! stale stamp took every popup of the world down on 2026-09-18.
//!
//! Provenance lives in the file header (`obstacle_index_file`): `code_ver` must
//! equal [`CACHE_CODE_VER`] (the index format changed ⇒ rebuild); `data_ver` and
//! `data_len` are [`structures_fingerprint`] and the byte length of the Arrow
//! file the index was built from. Every reader compares `data_len` with the
//! table on disk (one `metadata()` call); the painter, which holds the
//! manifest-verified Arrow bytes anyway, passes the fingerprint and gets the
//! full pairing checked. The release-level proof that no `structures.arrow`
//! changed after the step is `build-world.py`'s zero-write rerun of the step.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use grid::Square;
use noise_compute::propagation::obstacle_index::ObstacleIndex;
use noise_compute::propagation::obstacle_index_file::{
    fnv1a64, index_file_provenance, IndexBlob, IndexFileProvenance, FNV1A64_SEED,
    HEADER_BYTES,
};

/// The format version of `structures.qoix`. Change it by hand in the same commit that changes
/// the index BYTES (builder, grid pitch, id ordering, height cap, header); every older file is
/// then refused until `structures-finalize` reruns. It used to be a content hash of ten source
/// files, so a comment edit in `constants.rs` on 2026-09-18 declared every index in the world
/// stale and every popup returned 500. The value is that last hash, so the files of release
/// r260910 stay valid.
pub const CACHE_CODE_VER: u64 = 0x513e_d5eb_7ee8_1826;

pub const STRUCTURES_ARROW: &str = "structures.arrow";
pub const STRUCTURES_QOIX: &str = "structures.qoix";

/// The operator instruction every "no usable index" error carries.
const REBUILD_HINT: &str =
    "rerun the pipeline step structures-finalize (engine/target/release/structures-finalize <prepared_year_dir>)";

/// Process memo capacity of MAPPED indexes. A dense metro square's index runs to low
/// hundreds of MB; popups cluster spatially, so a small LRU keeps the active area's
/// mappings (and their faulted pages) alive while bounding worst-case RSS. Indexes built
/// in process are heap, not mappings, and are bounded by [`HEAP_BUILT_INDEX_CAP`].
const MEMO_CAP: usize = 8;

/// Fingerprint of a `structures.arrow` as the index's `data_ver`.
pub fn structures_fingerprint(structures_arrow_bytes: &[u8]) -> u64 {
    fnv1a64(FNV1A64_SEED, structures_arrow_bytes)
}

/// Mapped index file. The mapping's address and contents are fixed for its
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

/// Memo of mapped indexes keyed on the FILE ([`FileIdentity`]): a release is
/// immutable and a new release is a new inode, so the key names exactly one
/// set of bytes. Keying on the square alone once served one prepared tree's
/// index to a query about another (2026-08-05).
struct Memo {
    map: HashMap<FileIdentity, (Arc<ObstacleIndex>, IndexFileProvenance, u64)>,
    stamp: u64,
}

/// (device, inode, length, mtime): the inode alone can be reused by the next
/// file created after an unlink, so the length and mtime pin the bytes too.
type FileIdentity = (u64, u64, u64, i64, i64);

static MEMO: OnceLock<Mutex<Memo>> = OnceLock::new();

fn memo() -> &'static Mutex<Memo> {
    MEMO.get_or_init(|| {
        Mutex::new(Memo {
            map: HashMap::new(),
            stamp: 0,
        })
    })
}

fn file_identity(meta: &std::fs::Metadata) -> FileIdentity {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    )
}

fn remembered(key: &FileIdentity) -> Option<(Arc<ObstacleIndex>, IndexFileProvenance)> {
    let mut memo = memo().lock().unwrap_or_else(|e| e.into_inner());
    memo.stamp += 1;
    let stamp = memo.stamp;
    let (index, built_from, touched) = memo.map.get_mut(key)?;
    *touched = stamp;
    Some((Arc::clone(index), *built_from))
}

fn remember(key: FileIdentity, index: &Arc<ObstacleIndex>, built_from: IndexFileProvenance) {
    let mut memo = memo().lock().unwrap_or_else(|e| e.into_inner());
    memo.stamp += 1;
    let stamp = memo.stamp;
    if memo.map.len() >= MEMO_CAP {
        if let Some((&evict, _)) = memo.map.iter().min_by_key(|(_, (_, _, t))| *t) {
            memo.map.remove(&evict);
        }
    }
    memo.map.insert(key, (Arc::clone(index), built_from, stamp));
}

/// Indexes built in process own anonymous heap the kernel cannot drop, unlike the mapped files
/// of [`MEMO_CAP`], so they have their own budget: one query's surface squares (at most four),
/// and at most 1 GiB of source tables, of which an index is a fraction. The newest index always
/// stays; it is alive for its query anyway.
const HEAP_BUILT_INDEX_CAP: usize = 4;
const HEAP_BUILT_TABLE_BYTES_BUDGET: u64 = 1 << 30;

/// Oldest first: (table identity, index, table bytes).
static HEAP_BUILT_INDEXES: Mutex<Vec<(FileIdentity, Arc<ObstacleIndex>, u64)>> =
    Mutex::new(Vec::new());
/// One in-process build at a time in the whole process: concurrent clicks and the rayon workers
/// of one click wait for the first build of a table instead of each reading it (a dense metro
/// table is ~1 GB), and two different tables are never held in memory together.
static ONE_BUILD_AT_A_TIME: Mutex<()> = Mutex::new(());

fn heap_built(key: &FileIdentity) -> Option<Arc<ObstacleIndex>> {
    let mut indexes = HEAP_BUILT_INDEXES.lock().unwrap_or_else(|e| e.into_inner());
    let at = indexes.iter().position(|(identity, _, _)| identity == key)?;
    let entry = indexes.remove(at);
    let index = Arc::clone(&entry.1);
    indexes.push(entry);
    Some(index)
}

fn keep_heap_built(key: FileIdentity, index: &Arc<ObstacleIndex>, table_bytes: u64) {
    let mut indexes = HEAP_BUILT_INDEXES.lock().unwrap_or_else(|e| e.into_inner());
    indexes.push((key, Arc::clone(index), table_bytes));
    while indexes.len() > 1
        && (indexes.len() > HEAP_BUILT_INDEX_CAP
            || indexes.iter().map(|(_, _, bytes)| bytes).sum::<u64>()
                > HEAP_BUILT_TABLE_BYTES_BUDGET)
    {
        indexes.remove(0);
    }
}

/// The popup's loader: a missing, stale or mispaired `structures.qoix` costs this square a slow
/// first click, never the answer: the index is derived, so the build gives the same edges.
/// Only a table that cannot become an index is an error. The release stays immutable: the
/// built index is never written back.
pub fn load_square_obstacle_index_or_build_it_in_process(
    square_dir: &Path,
    square: Square,
) -> Result<Option<Arc<ObstacleIndex>>, String> {
    let index_file_fault = match load_square_obstacle_index(square_dir, None) {
        Ok(index) => return Ok(index),
        Err(fault) => fault,
    };
    let arrow_path = square_dir.join(STRUCTURES_ARROW);
    let build = || -> Result<Arc<ObstacleIndex>, String> {
        let meta =
            std::fs::metadata(&arrow_path).map_err(|e| format!("{}: {e}", arrow_path.display()))?;
        let key = file_identity(&meta);
        if let Some(index) = heap_built(&key) {
            return Ok(index);
        }
        let _one_build = ONE_BUILD_AT_A_TIME
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(index) = heap_built(&key) {
            return Ok(index);
        }
        let bytes =
            std::fs::read(&arrow_path).map_err(|e| format!("{}: {e}", arrow_path.display()))?;
        let index = Arc::new(
            crate::structure_store::build_obstacle_index_from_arrow_bytes(
                square,
                &bytes,
                &arrow_path,
            )?,
        );
        keep_heap_built(key, &index, bytes.len() as u64);
        Ok(index)
    };
    match build() {
        Ok(index) => {
            square_store::warn_once::warn_once(
                &format!(
                    "{index_file_fault}; serving an index built in process from {}",
                    arrow_path.display()
                ),
                "",
            );
            Ok(Some(index))
        }
        Err(build_fault) => Err(format!(
            "{index_file_fault}; building it in process failed: {build_fault}"
        )),
    }
}

/// One square's index, or `None` when there is no square directory (outside
/// the prepared world). A square directory without `structures.arrow` is a
/// broken tree — the structures step writes a 0-row table for every square any
/// source created — so it is an error, never "no obstacles".
/// `expected_fingerprint` is `Some` for a caller holding the Arrow bytes.
pub fn load_square_obstacle_index(
    square_dir: &Path,
    expected_fingerprint: Option<u64>,
) -> Result<Option<Arc<ObstacleIndex>>, String> {
    if !square_dir.is_dir() {
        return Ok(None);
    }
    let arrow_len = match std::fs::metadata(square_dir.join(STRUCTURES_ARROW)) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => {
            return Err(format!(
                "{} has no {STRUCTURES_ARROW}: the structures step did not finish this square",
                square_dir.display()
            ))
        }
    };
    let path = square_dir.join(STRUCTURES_QOIX);
    let file = std::fs::File::open(&path).map_err(|e| {
        format!(
            "{} beside {STRUCTURES_ARROW}: {e}; {REBUILD_HINT}",
            path.display()
        )
    })?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let key = file_identity(&meta);
    let check_pairing = |built_from: IndexFileProvenance| -> Result<(), String> {
        let other_bytes = match expected_fingerprint {
            Some(want) if want != built_from.data_ver => Some(format!(
                "data_ver {:016x} ≠ {want:016x}",
                built_from.data_ver
            )),
            _ if built_from.data_len != arrow_len => Some(format!(
                "{} bytes ≠ {arrow_len} on disk",
                built_from.data_len
            )),
            _ => None,
        };
        match other_bytes {
            Some(why) => Err(format!(
                "{} was built from other {STRUCTURES_ARROW} bytes ({why}); {REBUILD_HINT}",
                path.display()
            )),
            None => Ok(()),
        }
    };
    if let Some((index, built_from)) = remembered(&key) {
        check_pairing(built_from)?;
        return Ok(Some(index));
    }
    // SAFETY: the file is written once (tmp + rename) and a release is never
    // edited in place, so no writer can change these bytes under the mapping.
    let mmap =
        unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;
    let built_from = index_file_provenance(&mmap)
        .map_err(|e| format!("{}: {e}; {REBUILD_HINT}", path.display()))?;
    if built_from.code_ver != CACHE_CODE_VER {
        return Err(format!(
            "{} is stale (code_ver {:016x}, engine {CACHE_CODE_VER:016x}); {REBUILD_HINT}",
            path.display(),
            built_from.code_ver
        ));
    }
    check_pairing(built_from)?;
    let blob: Arc<dyn IndexBlob> = Arc::new(MappedIndexFile(mmap));
    let index = Arc::new(
        ObstacleIndex::from_blob(blob, CACHE_CODE_VER)
            .map_err(|e| format!("{}: {e}; {REBUILD_HINT}", path.display()))?,
    );
    remember(key, &index, built_from);
    Ok(Some(index))
}

/// What the pipeline step did for one square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObstacleIndexReceipt {
    pub edge_count: usize,
    /// `false` when the existing file already matched the engine and the table.
    pub written: bool,
}

/// Write `structures.qoix` from the square's final `structures.arrow` bytes,
/// atomically (same-directory tmp + rename + directory fsync) and
/// idempotently: an existing file whose header names this engine and these
/// Arrow bytes is kept after reading only its header. The written file is
/// reopened through [`load_square_obstacle_index`], so a receipt proves the
/// file is usable.
pub fn write_square_obstacle_index(
    square_dir: &Path,
    square: Square,
    bytes: &[u8],
) -> Result<ObstacleIndexReceipt, String> {
    let arrow_path = square_dir.join(STRUCTURES_ARROW);
    let provenance = IndexFileProvenance {
        code_ver: CACHE_CODE_VER,
        data_ver: structures_fingerprint(bytes),
        data_len: bytes.len() as u64,
    };
    let path = square_dir.join(STRUCTURES_QOIX);
    let header = std::fs::File::open(&path).ok().and_then(|mut f| {
        let mut header = [0u8; HEADER_BYTES];
        f.read_exact(&mut header).ok().map(|()| header)
    });
    let current = header.is_some_and(|h| index_file_provenance(&h) == Ok(provenance));
    let written = !current;
    if written {
        let index = crate::structure_store::build_obstacle_index_from_arrow_bytes(
            square,
            bytes,
            &arrow_path,
        )?;
        let parts = index.file_parts(provenance);
        let sections: Vec<&[u8]> = std::iter::once(parts.header.as_slice())
            .chain(parts.sections.iter().copied())
            .collect();
        crate::structures_finalize::write_file_atomically(square_dir, STRUCTURES_QOIX, &sections)?;
    }
    let index = load_square_obstacle_index(square_dir, Some(provenance.data_ver))?
        .ok_or_else(|| format!("{} vanished during the build", arrow_path.display()))?;
    Ok(ObstacleIndexReceipt {
        edge_count: index.edge_count(),
        written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_test_fixture as fx;
    use crate::structures_finalize::finalize_square_structures;
    use square_store::store::STRUCTURE_KIND_BUILDING;
    use tempfile::TempDir;

    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    fn house(height_m: i16) -> fx::StructureRow {
        fx::StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            height_m,
            envelope_class: 1,
            centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
            osm_id: Some(7),
            building_type: Some(1),
            area_m2: Some(450.0),
            ..Default::default()
        }
    }

    /// The index pairs with the table and the engine: a changed table — even
    /// at the same byte length — or an index from another engine version
    /// makes the step rewrite, and the reopened file answers with the
    /// builder's bytes. (The rerun no-op and the crash-leftover sweep are
    /// `structures_finalize`'s test.)
    #[test]
    fn pipeline_step_rewrites_the_index_when_the_table_or_engine_changes() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        let arrow = dir.join(STRUCTURES_ARROW);
        fx::write_structure_file(&arrow, &[house(12)], true);
        let first = finalize_square_structures(&dir, square).unwrap().unwrap().index;
        assert!(first.written && first.edge_count == 4, "{first:?}");
        assert!(dir.join(STRUCTURES_QOIX).is_file());

        let built = crate::structure_store::build_obstacle_index_from_arrow_bytes(
            square,
            &std::fs::read(&arrow).unwrap(),
            &arrow,
        )
        .unwrap();
        let mapped = load_square_obstacle_index(&dir, None).unwrap().unwrap();
        assert_eq!(mapped.gpu_view().edges_xyxyh, built.gpu_view().edges_xyxyh);
        assert_eq!(mapped.gpu_view().edge_ids, built.gpu_view().edge_ids);

        fx::write_structure_file(&arrow, &[house(12), house(20)], true);
        let rebuilt = finalize_square_structures(&dir, square).unwrap().unwrap().index;
        assert!(rebuilt.written && rebuilt.edge_count == 8, "{rebuilt:?}");
        // A same-length rewrite (one height changed) still rotates the
        // fingerprint: the step rewrites, so the zero-write gate catches it.
        let length = std::fs::metadata(&arrow).unwrap().len();
        fx::write_structure_file(&arrow, &[house(12), house(21)], true);
        let same_length = finalize_square_structures(&dir, square).unwrap().unwrap().index;
        assert_eq!(std::fs::metadata(&arrow).unwrap().len(), length);
        assert!(same_length.written, "{same_length:?}");

        // Another engine version (the grid pitch or the walk moved): the
        // header word is rewritten in a copy the memo has not seen, and the
        // step rewrites the index instead of refusing the square.
        let mut bytes = std::fs::read(dir.join(STRUCTURES_QOIX)).unwrap();
        bytes[8..16].copy_from_slice(&(CACHE_CODE_VER ^ 1).to_le_bytes());
        std::fs::remove_file(dir.join(STRUCTURES_QOIX)).unwrap();
        std::fs::write(dir.join(STRUCTURES_QOIX), bytes).unwrap();
        let other_engine = finalize_square_structures(&dir, square).unwrap().unwrap().index;
        assert!(other_engine.written, "{other_engine:?}");
        let rerun = finalize_square_structures(&dir, square).unwrap().unwrap().index;
        assert!(!rerun.written, "{rerun:?}");
    }

    /// A table without its index, or with an index from another engine or
    /// another table — including a table rewritten after the step, seen by a
    /// popup that holds no Arrow bytes — is a pipeline fault the loader must
    /// name, never a silent unscreened path.
    #[test]
    fn missing_stale_or_mismatched_index_is_an_error_naming_the_step() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        let arrow = dir.join(STRUCTURES_ARROW);
        fx::write_structure_file(&arrow, &[house(12)], true);
        let refuses =
            |expected: Option<u64>, want: &str| match load_square_obstacle_index(&dir, expected) {
                Ok(_) => panic!("must refuse ({want})"),
                Err(err) => {
                    assert!(
                        err.contains(want) && err.contains("structures-finalize"),
                        "{err}"
                    )
                }
            };
        refuses(None, "beside structures.arrow");

        finalize_square_structures(&dir, square).unwrap();
        let fingerprint = structures_fingerprint(&std::fs::read(&arrow).unwrap());
        assert!(load_square_obstacle_index(&dir, Some(fingerprint)).is_ok());
        refuses(Some(fingerprint ^ 1), "other structures.arrow bytes");
        // The popup does not hold the table; it checks the table's length.
        assert!(load_square_obstacle_index(&dir, None).is_ok());
        fx::write_structure_file(&arrow, &[house(12), house(20)], true);
        refuses(None, "bytes ≠");
        finalize_square_structures(&dir, square).unwrap();
        assert!(load_square_obstacle_index(&dir, None).is_ok());

        // Another engine version: rewrite the header word only, in a copy the
        // memo has not seen (a new inode).
        let mut bytes = std::fs::read(dir.join(STRUCTURES_QOIX)).unwrap();
        bytes[8..16].copy_from_slice(&(CACHE_CODE_VER ^ 1).to_le_bytes());
        std::fs::remove_file(dir.join(STRUCTURES_QOIX)).unwrap();
        std::fs::write(dir.join(STRUCTURES_QOIX), bytes).unwrap();
        refuses(None, "stale");
    }

    /// The tests that fill the process-wide heap-built budget take turns, so neither evicts
    /// the other's index between two of its own loads.
    static HEAP_BUILT_BUDGET_TESTS_TAKE_TURNS: Mutex<()> = Mutex::new(());

    /// The visitor's loader answers all three index faults with the builder's own edges.
    #[test]
    fn popup_builds_a_missing_stale_or_mispaired_index_in_process() {
        let _turn = HEAP_BUILT_BUDGET_TESTS_TAKE_TURNS.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        let arrow = dir.join(STRUCTURES_ARROW);
        let serves_edges = |edges: usize| {
            assert!(load_square_obstacle_index(&dir, None).is_err());
            let index = load_square_obstacle_index_or_build_it_in_process(&dir, square)
                .unwrap()
                .unwrap();
            assert_eq!(index.edge_count(), edges);
        };
        fx::write_structure_file(&arrow, &[house(12)], true);
        serves_edges(4);
        finalize_square_structures(&dir, square).unwrap();
        fx::write_structure_file(&arrow, &[house(12), house(20)], true);
        serves_edges(8);
        finalize_square_structures(&dir, square).unwrap();
        let mut bytes = std::fs::read(dir.join(STRUCTURES_QOIX)).unwrap();
        bytes[8..16].copy_from_slice(&(CACHE_CODE_VER ^ 1).to_le_bytes());
        std::fs::remove_file(dir.join(STRUCTURES_QOIX)).unwrap();
        std::fs::write(dir.join(STRUCTURES_QOIX), bytes).unwrap();
        serves_edges(8);
        std::fs::write(&arrow, b"not Arrow").unwrap();
        let fault = load_square_obstacle_index_or_build_it_in_process(&dir, square)
            .err()
            .unwrap();
        assert!(fault.contains("building it in process failed"), "{fault}");
    }

    /// Concurrent clicks and rayon workers on one stale square share ONE build: every caller
    /// gets the same index, so the table was read once.
    #[test]
    fn concurrent_popups_of_one_stale_square_share_one_in_process_build() {
        let _turn = HEAP_BUILT_BUDGET_TESTS_TAKE_TURNS.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_structure_file(&dir.join(STRUCTURES_ARROW), &[house(12)], true);
        let built: Vec<_> = std::thread::scope(|scope| {
            let clicks: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        load_square_obstacle_index_or_build_it_in_process(&dir, square)
                            .unwrap()
                            .unwrap()
                    })
                })
                .collect();
            clicks.into_iter().map(|click| click.join().unwrap()).collect()
        });
        assert!(built.iter().all(|index| Arc::ptr_eq(index, &built[0])));
    }

    /// No square directory is the answer "no obstacles" (outside the prepared
    /// world); a square directory without its table is a broken tree.
    #[test]
    fn absent_square_is_no_obstacles_but_a_square_without_its_table_is_a_fault() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        assert!(load_square_obstacle_index(&dir, None).unwrap().is_none());
        std::fs::create_dir_all(&dir).unwrap();
        match load_square_obstacle_index(&dir, None) {
            Ok(_) => panic!("a square without its table must be a fault"),
            Err(err) => assert!(err.contains("structures step"), "{err}"),
        }
    }
}
