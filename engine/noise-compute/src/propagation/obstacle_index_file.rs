//! On-disk (mmap-able) form of [`ObstacleIndex`] — build the grid once, map it
//! on every later cold start instead of re-deriving it from the Arrow shards.
//!
//! A São Paulo popup indexes ~40 M edges: ~6 s and ~1.1 GB of CSR arrays that
//! the process then throws away. Those arrays are already flat and immutable,
//! so the file IS the in-memory layout — a load is `mmap` plus a header check,
//! and the kernel faults in only the cells a ray actually walks. Nothing is
//! copied into the heap, which is the whole point: a 1.1 GB deserialize would
//! move the cost, not remove it.
//!
//! **The crate stays file-free** (see `Cargo.toml`): this module defines the
//! BYTES and validates them. Opening, mapping and writing files is the caller's
//! (`source-reader`'s) job, handed in through [`IndexBlob`].
//!
//! Staleness is decided by two u64s in the header, never by a comment:
//! * [`BUILDER_CODE_VER`] — a content hash of every source file that decides
//!   the bytes (the Rust twin of `scripts/layer-codever.py`'s per-layer content
//!   set-hash: over-invalidate rather than risk a stale artifact);
//! * `data_ver` — the caller's fingerprint of the INPUT files (the twin of
//!   `world-stamps.py`'s `_data_ver` mtime set-hash).
//!
//! Both must match exactly or [`ObstacleIndex::from_blob`] refuses the file and
//! the caller rebuilds. A wasted rebuild costs a minute; a stale index is a
//! silent hole in the map.

use std::sync::Arc;

use super::obstacle_index::{ObstacleEdge, ObstacleIndex};

/// Stable, immutable byte backing for a mapped index (an `Arc<Mmap>` in
/// production, an `Arc<Vec<u8>>` in tests).
///
/// # Safety
/// `as_bytes` must return the SAME address and length on every call for the
/// whole life of the value, and those bytes must never be mutated while the
/// value lives — [`IndexArray`] hands out slices into them for as long as the
/// index exists. `Mmap` and `Vec<u8>` both satisfy this; a type that
/// reallocates or re-reads on access does not.
pub unsafe trait IndexBlob: Send + Sync {
    fn as_bytes(&self) -> &[u8];
}

// SAFETY: `Vec<u8>`'s buffer address and length are fixed while the `Vec` is
// not mutated, and an `Arc<Vec<u8>>` hands out no `&mut`.
unsafe impl IndexBlob for Vec<u8> {
    fn as_bytes(&self) -> &[u8] {
        self
    }
}

/// One CSR array of an [`ObstacleIndex`]: heap-owned when the index was just
/// built, a window into a mapped file when it was loaded.
///
/// Derefs to `&[T]` so every query site reads `self.edges[i]` /
/// `&self.edge_refs[lo..hi]` exactly as it did when these were `Vec`s. The
/// pointer is resolved ONCE at construction rather than per access: a `Vec`'s
/// heap buffer and a mapping's base address are both stable across moves of the
/// owner, so the deref is as cheap as a `Vec`'s and the hot walks keep their
/// codegen.
pub struct IndexArray<T: Copy + 'static> {
    /// Keeps the bytes alive. Never read after construction — `ptr` is the
    /// resolved view of exactly these bytes.
    _backing: Backing<T>,
    ptr: *const T,
    len: usize,
}

/// Whatever owns the bytes `IndexArray::ptr` points into. Held for its
/// LIFETIME alone — dropping it would dangle the pointer — and deliberately
/// never read, which is what the `dead_code` allow records.
#[allow(dead_code)]
enum Backing<T> {
    Owned(Vec<T>),
    Mapped(Arc<dyn IndexBlob>),
}

// SAFETY: an `IndexArray` is immutable after construction and its bytes are
// either an owned `Vec<T>` or an `IndexBlob` (itself `Send + Sync` and
// immutable by contract), so sharing `&IndexArray` across threads shares only
// read-only memory.
unsafe impl<T: Copy + Send + Sync + 'static> Send for IndexArray<T> {}
// SAFETY: as above.
unsafe impl<T: Copy + Send + Sync + 'static> Sync for IndexArray<T> {}

impl<T: Copy + 'static> IndexArray<T> {
    /// Adopt a freshly built array.
    pub fn from_vec(v: Vec<T>) -> Self {
        let (ptr, len) = (v.as_ptr(), v.len());
        IndexArray {
            _backing: Backing::Owned(v),
            ptr,
            len,
        }
    }

    /// View `len` elements at byte `offset` of `blob`.
    ///
    /// Returns `None` when the window would leave the blob or would be
    /// misaligned for `T` — the two conditions that make the raw view unsound,
    /// checked here so no caller can skip them.
    fn from_blob(blob: &Arc<dyn IndexBlob>, offset: usize, len: usize) -> Option<Self> {
        let bytes = blob.as_bytes();
        let want = len.checked_mul(std::mem::size_of::<T>())?;
        if offset.checked_add(want)? > bytes.len() {
            return None;
        }
        // SAFETY: `offset <= bytes.len()`, so the one-past-the-end result is
        // still inside the same allocation.
        let ptr = unsafe { bytes.as_ptr().add(offset) };
        if !(ptr as usize).is_multiple_of(std::mem::align_of::<T>()) {
            return None;
        }
        Some(IndexArray {
            _backing: Backing::Mapped(Arc::clone(blob)),
            ptr: ptr.cast::<T>(),
            len,
        })
    }
}

impl<T: Copy + 'static> std::ops::Deref for IndexArray<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        // SAFETY: `ptr`/`len` were validated at construction against a backing
        // whose address, length and contents are fixed for its whole life (the
        // `IndexBlob` contract, or an untouched owned `Vec`), and that backing
        // is kept alive by `_backing` in this very struct — so the slice cannot
        // outlive its memory.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T: Copy + 'static> From<Vec<T>> for IndexArray<T> {
    fn from(v: Vec<T>) -> Self {
        IndexArray::from_vec(v)
    }
}

impl<T: Copy + std::fmt::Debug + 'static> std::fmt::Debug for IndexArray<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// FNV-1a over `bytes`, seeded with `seed` — `const` so a source-content hash
/// can be computed at COMPILE time from `include_bytes!`. Not a cryptographic
/// hash and does not need to be: it fingerprints our own build inputs, and the
/// only adversary is a forgotten rebuild.
pub const fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// FNV-1a offset basis — the seed for a fresh chain.
pub const FNV1A64_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Content hash of every source file that decides an index's BYTES: the
/// builder and its grid pitch, this file's layout, the WKB ring parser, the
/// low-profile height cap and the metric-frame constants. Editing any of them
/// rotates the version, so every cached file written by the old code is refused
/// on the next start — the same safe-over-invalidation rule
/// `scripts/layer-codever.py` applies to tiles, enforced by the compiler instead
/// of by remembering to bump a number.
///
/// Callers that add decisions of their OWN on top (id ordering, shard order)
/// must fold their source in too — see `source-reader`'s
/// `obstacle_store::CACHE_CODE_VER`.
pub const BUILDER_CODE_VER: u64 = {
    let h = fnv1a64(FNV1A64_SEED, include_bytes!("obstacle_index.rs"));
    let h = fnv1a64(h, include_bytes!("obstacle_index_file.rs"));
    let h = fnv1a64(h, include_bytes!("../wkb.rs"));
    let h = fnv1a64(h, include_bytes!("../low_profile.rs"));
    fnv1a64(h, include_bytes!("../constants.rs"))
};

/// "Quiet Obstacle IndeX" — a stray file identifies itself, like the tile
/// store's `QTSI`/`QTSD`.
const MAGIC: &[u8; 4] = b"QOIX";
/// Bumped only for a layout change the content hash cannot see (it can see
/// every one of ours, so this exists for forensics, not for gating).
const VERSION: u8 = 1;
const HEADER_BYTES: usize = 128;
/// Every section starts on this boundary, so the mapping's page alignment
/// carries through to each typed view (`u32`/`f32`/[`ObstacleEdge`] all want 4).
const SECTION_ALIGN: usize = 64;

/// Byte offset and length of each section, derived from the header counts —
/// the writer and the reader compute them with this one function, so they
/// cannot disagree.
struct Layout {
    cell_starts: (usize, usize),
    edge_refs: (usize, usize),
    edges: (usize, usize),
    cell_max_h: (usize, usize),
    footprint_xmin: (usize, usize),
    total: usize,
}

const fn align_up(n: usize) -> usize {
    n.div_ceil(SECTION_ALIGN) * SECTION_ALIGN
}

impl Layout {
    fn new(cells: usize, n_edge_refs: usize, n_edges: usize, n_fp: usize) -> Option<Self> {
        let mut at = HEADER_BYTES;
        let mut section = |elems: usize, elem_bytes: usize| -> Option<(usize, usize)> {
            let bytes = elems.checked_mul(elem_bytes)?;
            let start = at;
            at = align_up(start.checked_add(bytes)?);
            Some((start, bytes))
        };
        let cell_starts = section(cells.checked_add(1)?, 4)?;
        let edge_refs = section(n_edge_refs, 4)?;
        let edges = section(n_edges, std::mem::size_of::<ObstacleEdge>())?;
        let cell_max_h = section(cells, 4)?;
        let footprint_xmin = section(n_fp, 4)?;
        Some(Layout {
            cell_starts,
            edge_refs,
            edges,
            cell_max_h,
            footprint_xmin,
            total: at,
        })
    }
}

/// The index as bytes to write, in order: [`FileParts::header`] first, then
/// every slice of [`FileParts::sections`] (data and inter-section padding
/// already interleaved). Concatenating them IS the file — no intermediate
/// buffer, so writing a 1.1 GB index costs no extra RAM.
pub struct FileParts<'a> {
    pub header: [u8; HEADER_BYTES],
    pub sections: Vec<&'a [u8]>,
}

impl FileParts<'_> {
    /// Total bytes the writer will emit.
    pub fn total_len(&self) -> usize {
        HEADER_BYTES + self.sections.iter().map(|s| s.len()).sum::<usize>()
    }
}

/// Reinterpret a slice of POD values as its raw bytes.
fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: `T` is one of `u32`/`f32`/`ObstacleEdge` — all `#[repr(C)]` POD
    // with no padding and no pointers — and the result borrows the same memory
    // for the same lifetime, read-only.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

static ZERO_PAD: [u8; SECTION_ALIGN] = [0; SECTION_ALIGN];

impl ObstacleIndex {
    /// Serialize for [`ObstacleIndex::from_blob`]. `data_ver` is the caller's
    /// fingerprint of the input files this index was built from; it is stored
    /// verbatim and compared on load.
    pub fn file_parts(&self, code_ver: u64, data_ver: u64) -> FileParts<'_> {
        let cells = self.cols * self.rows;
        let layout = Layout::new(
            cells,
            self.edge_refs.len(),
            self.edges.len(),
            self.footprint_xmin.len(),
        )
        .expect("obstacle index layout overflows usize");

        let mut header = [0u8; HEADER_BYTES];
        header[0..4].copy_from_slice(MAGIC);
        header[4] = VERSION;
        let mut put = |at: usize, v: u64| header[at..at + 8].copy_from_slice(&v.to_le_bytes());
        put(8, code_ver);
        put(16, data_ver);
        put(24, self.origin_lat.to_bits());
        put(32, self.origin_lon.to_bits());
        put(40, self.m_per_deg_lon.to_bits());
        put(48, self.cell_m.to_bits());
        put(56, self.min_x.to_bits());
        put(64, self.min_y.to_bits());
        put(72, self.max_footprint_w.to_bits());
        put(80, self.cols as u64);
        put(88, self.rows as u64);
        put(96, self.edge_refs.len() as u64);
        put(104, self.edges.len() as u64);
        put(112, self.footprint_xmin.len() as u64);
        put(120, layout.total as u64);

        let mut sections = Vec::with_capacity(10);
        let mut at = HEADER_BYTES;
        for bytes in [
            as_bytes(&self.cell_starts),
            as_bytes(&self.edge_refs),
            as_bytes(&self.edges),
            as_bytes(&self.cell_max_h),
            as_bytes(&self.footprint_xmin),
        ] {
            sections.push(bytes);
            at += bytes.len();
            let pad = align_up(at) - at;
            if pad > 0 {
                sections.push(&ZERO_PAD[..pad]);
                at += pad;
            }
        }
        debug_assert_eq!(at, layout.total);
        FileParts { header, sections }
    }

    /// Map a previously written index, or explain why the file cannot be used.
    ///
    /// Refuses anything whose `code_ver` or `data_ver` differs from the
    /// caller's — a mismatch means the builder or its inputs moved, and the
    /// only safe answer is to rebuild. Nothing is copied: the returned index
    /// reads straight out of `blob`, so a 1.1 GB file costs one `mmap` and the
    /// pages a query actually touches.
    pub fn from_blob(
        blob: Arc<dyn IndexBlob>,
        expect_code_ver: u64,
        expect_data_ver: u64,
    ) -> Result<ObstacleIndex, String> {
        let bytes = blob.as_bytes();
        if bytes.len() < HEADER_BYTES {
            return Err(format!("truncated header ({} bytes)", bytes.len()));
        }
        if &bytes[0..4] != MAGIC {
            return Err(format!("bad magic {:?} (want {MAGIC:?})", &bytes[0..4]));
        }
        if bytes[4] != VERSION {
            return Err(format!("format version {} ≠ {VERSION}", bytes[4]));
        }
        let get = |at: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(b)
        };
        let (code_ver, data_ver) = (get(8), get(16));
        if code_ver != expect_code_ver {
            return Err(format!("code_ver {code_ver:016x} ≠ {expect_code_ver:016x}"));
        }
        if data_ver != expect_data_ver {
            return Err(format!("data_ver {data_ver:016x} ≠ {expect_data_ver:016x}"));
        }
        let usz = |v: u64| usize::try_from(v).map_err(|_| format!("count {v} exceeds usize"));
        let cols = usz(get(80))?;
        let rows = usz(get(88))?;
        let n_edge_refs = usz(get(96))?;
        let n_edges = usz(get(104))?;
        let n_fp = usz(get(112))?;
        let total = usz(get(120))?;
        if cols == 0 || rows == 0 {
            return Err(format!("empty grid {cols}×{rows}"));
        }
        let cells = cols
            .checked_mul(rows)
            .ok_or_else(|| format!("grid {cols}×{rows} overflows usize"))?;
        let layout = Layout::new(cells, n_edge_refs, n_edges, n_fp)
            .ok_or_else(|| "section layout overflows usize".to_string())?;
        if layout.total != total || bytes.len() < total {
            return Err(format!(
                "size mismatch: header says {total}, layout {}, file {}",
                layout.total,
                bytes.len()
            ));
        }

        fn map<T: Copy + 'static>(
            blob: &Arc<dyn IndexBlob>,
            (off, _len): (usize, usize),
            n: usize,
            what: &str,
        ) -> Result<IndexArray<T>, String> {
            IndexArray::from_blob(blob, off, n).ok_or_else(|| format!("{what} window invalid"))
        }
        let cell_starts: IndexArray<u32> =
            map(&blob, layout.cell_starts, cells + 1, "cell_starts")?;
        let edge_refs: IndexArray<u32> = map(&blob, layout.edge_refs, n_edge_refs, "edge_refs")?;
        let edges: IndexArray<ObstacleEdge> = map(&blob, layout.edges, n_edges, "edges")?;
        let cell_max_h: IndexArray<f32> = map(&blob, layout.cell_max_h, cells, "cell_max_h")?;
        let footprint_xmin: IndexArray<f32> =
            map(&blob, layout.footprint_xmin, n_fp, "footprint_xmin")?;

        // O(1) structural check: the CSR's own invariant. Catches a truncated
        // or half-written file without touching (and paging in) a single edge —
        // scanning 40 M edges here would undo the reason this file exists.
        if cell_starts[0] != 0 || cell_starts[cells] as usize != n_edge_refs {
            return Err(format!(
                "CSR bounds broken: starts[0]={}, starts[{cells}]={} ≠ {n_edge_refs}",
                cell_starts[0], cell_starts[cells]
            ));
        }

        Ok(ObstacleIndex {
            origin_lat: f64::from_bits(get(24)),
            origin_lon: f64::from_bits(get(32)),
            m_per_deg_lon: f64::from_bits(get(40)),
            cell_m: f64::from_bits(get(48)),
            min_x: f64::from_bits(get(56)),
            min_y: f64::from_bits(get(64)),
            cols,
            rows,
            cell_starts,
            edge_refs,
            edges,
            cell_max_h,
            footprint_xmin,
            max_footprint_w: f64::from_bits(get(72)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::obstacle_index::{CrossingCandidate, ObstacleKind};

    /// Flatten `file_parts` the way a writer would.
    fn to_file_bytes(idx: &ObstacleIndex, code_ver: u64, data_ver: u64) -> Vec<u8> {
        let parts = idx.file_parts(code_ver, data_ver);
        let mut out = Vec::with_capacity(parts.total_len());
        out.extend_from_slice(&parts.header);
        for s in &parts.sections {
            out.extend_from_slice(s);
        }
        assert_eq!(out.len(), parts.total_len());
        out
    }

    const OLAT: f64 = 50.08;
    const OLON: f64 = 14.43;

    /// A row of 12 blocks marching east from the origin, plus one long
    /// barrier polyline — enough edges to fill several grid cells, both
    /// `ObstacleKind`s, and a dense id space.
    fn sample_index() -> ObstacleIndex {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        for k in 0..12u32 {
            let e = f64::from(k) * 0.0015;
            b.add_ring(
                &[
                    (OLAT - 0.0003, OLON + e),
                    (OLAT - 0.0003, OLON + e + 0.0007),
                    (OLAT + 0.0003, OLON + e + 0.0007),
                    (OLAT + 0.0003, OLON + e),
                ],
                6.0 + k as f32,
                ObstacleKind::Building,
                k,
            );
        }
        b.add_polyline(
            &[(OLAT - 0.002, OLON + 0.009), (OLAT + 0.002, OLON + 0.009)],
            4.0,
            ObstacleKind::Barrier,
            12,
        );
        b.build()
    }

    /// A slightly tilted west→east ray through the whole row.
    fn crossings(idx: &ObstacleIndex) -> Vec<CrossingCandidate> {
        let mut out = Vec::new();
        idx.crossings(
            OLAT - 0.0001,
            OLON - 0.001,
            OLAT + 0.0001,
            OLON + 0.02,
            &mut out,
        );
        out
    }

    /// A round-tripped index must answer bit-identically — the property the
    /// whole cache rests on.
    #[test]
    fn round_trip_is_bit_identical() {
        let built = sample_index();
        let bytes = to_file_bytes(&built, 0xabc, 0xdef);
        let mapped = ObstacleIndex::from_blob(Arc::new(bytes), 0xabc, 0xdef).expect("loads");

        assert_eq!(mapped.edge_count(), built.edge_count());
        let (a, b) = (crossings(&built), crossings(&mapped));
        assert!(!a.is_empty(), "the probe ray must hit something");
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.t.to_bits(), y.t.to_bits(), "chainage must be bit-equal");
            assert_eq!(x.height_m.to_bits(), y.height_m.to_bits());
            assert_eq!(x.id, y.id);
            assert_eq!(x.kind, y.kind);
        }
        let mut seen_a = Vec::new();
        let mut seen_b = Vec::new();
        let mut inside = 0;
        for (lat, lon) in [
            (OLAT, OLON + 0.0003),          // inside block 0
            (OLAT, OLON + 0.0075 + 0.0003), // inside block 5
            (OLAT, OLON + 0.0011),          // the gap between blocks
            (OLAT + 0.01, OLON),            // well north of the row
        ] {
            let a = built.contains_built(lat, lon, 0.0, &mut seen_a);
            assert_eq!(a, mapped.contains_built(lat, lon, 0.0, &mut seen_b));
            inside += usize::from(a);
        }
        assert_eq!(inside, 2, "the containment probe must discriminate");
        let (ga, gb) = (built.gpu_view(), mapped.gpu_view());
        assert_eq!(ga.edges_xyxyh, gb.edges_xyxyh);
        assert_eq!(ga.edge_ids, gb.edge_ids);
        assert_eq!(ga.cell_starts, gb.cell_starts);
        assert_eq!(ga.edge_refs, gb.edge_refs);
        assert_eq!(ga.cell_max_h, gb.cell_max_h);
    }

    /// The rural fast path (no edges at all) must survive the round trip too —
    /// its 1×1 grid is the degenerate case every offset computation trips on.
    #[test]
    fn empty_index_round_trips() {
        let empty = ObstacleIndex::builder(50.0, 14.0).build();
        let bytes = to_file_bytes(&empty, 1, 2);
        let mapped = ObstacleIndex::from_blob(Arc::new(bytes), 1, 2).expect("loads");
        assert_eq!(mapped.edge_count(), 0);
        assert!(crossings(&mapped).is_empty());
    }

    /// Every rejection path: wrong builder version, wrong input fingerprint,
    /// foreign file, truncation. A cached index must never be used on a maybe.
    #[test]
    fn stale_or_damaged_files_are_refused() {
        // `ObstacleIndex` has no `Debug` (it would print 40 M edges), so
        // rejections are read back as the message they must carry.
        fn refuses(bytes: Vec<u8>, cv: u64, dv: u64, want: &str) {
            match ObstacleIndex::from_blob(Arc::new(bytes), cv, dv) {
                Ok(_) => panic!("must refuse ({want})"),
                Err(e) => assert!(e.contains(want), "{e} does not mention {want}"),
            }
        }
        let bytes = to_file_bytes(&sample_index(), 0xabc, 0xdef);

        refuses(bytes.clone(), 0xabd, 0xdef, "code_ver");
        refuses(bytes.clone(), 0xabc, 0xde0, "data_ver");

        let mut foreign = bytes.clone();
        foreign[0] = b'X';
        refuses(foreign, 0xabc, 0xdef, "magic");

        let mut old = bytes.clone();
        old[4] = VERSION + 1;
        refuses(old, 0xabc, 0xdef, "version");

        refuses(
            bytes[..bytes.len() - SECTION_ALIGN].to_vec(),
            0xabc,
            0xdef,
            "size mismatch",
        );
        refuses(vec![0u8; 8], 0xabc, 0xdef, "truncated");
    }

    /// The content hash must actually cover the builder's sources — a constant
    /// that never moves is worse than no versioning at all, because it looks
    /// like versioning.
    #[test]
    fn builder_code_ver_hashes_real_sources() {
        assert_ne!(BUILDER_CODE_VER, 0);
        assert_ne!(BUILDER_CODE_VER, FNV1A64_SEED);
        assert_ne!(
            BUILDER_CODE_VER,
            fnv1a64(FNV1A64_SEED, include_bytes!("obstacle_index.rs")),
            "the chain must fold in more than the first file"
        );
    }
}
