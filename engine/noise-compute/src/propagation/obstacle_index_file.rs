//! On-disk (mmap-able) edge table of one cell's [`ObstacleIndex`] — the kernel-frame
//! edges parsed once from the cell's `structures.arrow`, mapped on every later load.
//!
//! Parsing a metro cell's WKB rings is the cost worth storing: 5.0 s for Paris's
//! 10.7 M edges, 9.5 s for São Paulo's 18.4 M (one thread, 2026-09-07). The CSR grid
//! over those edges is not: deriving it from the mapped file costs 96 ms for Prague's
//! 3.2 M edges, 476 ms for Paris, 522 ms for São Paulo, while storing every cell's grid
//! took 310 GB of the world's 687 GB, 45 % of it empty grid cells. So the file holds
//! the edges and the per-footprint classes, nothing else: [`ObstacleIndex::from_blob`]
//! maps them, reads every edge once to derive the grid in memory (~4 B per edge
//! reference), and the kernel then reads the edges straight out of the mapping.
//!
//! **The crate stays file-free** (see `Cargo.toml`): this module defines the
//! BYTES and validates them. Opening, mapping and writing files is the caller's
//! (`source-reader`'s) job, handed in through [`IndexBlob`].
//!
//! Staleness is decided by two u64s in the header, never by a comment:
//! * [`BUILDER_CODE_VER`] — a content hash of every source file that decides
//!   the bytes (a compile-time per-input content
//!   set-hash: over-invalidate rather than risk a stale artifact);
//! * `data_ver` — the caller's fingerprint of the INPUT files (the twin of
//!   `world-stamps.py`'s `_data_ver` mtime set-hash).
//!
//! Both must match exactly or [`ObstacleIndex::from_blob`] refuses the file and
//! the caller rebuilds. A wasted rebuild costs seconds; a stale edge table is a
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

/// The edge table or the class table of an [`ObstacleIndex`]: heap-owned when
/// just parsed from Arrow, a window into the mapped file when loaded.
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

/// Content hash of every source file that decides an edge table's BYTES: the
/// builder (its grid pitch too — same file, so a pitch change over-invalidates),
/// this file's layout, the WKB ring parser, the low-profile height cap and the
/// metric-frame constants. Editing any of them
/// rotates the version, so every cached file written by the old code is refused
/// on the next start — the same safe-over-invalidation rule
/// a cached artifact needs, enforced by the compiler instead
/// of by remembering to bump a number.
///
/// Callers that add decisions of their OWN on top (id ordering, shard order)
/// must fold their source in too — see `source-reader`'s
/// `structure_store::EDGE_FILE_CODE_VER`.
pub const BUILDER_CODE_VER: u64 = {
    let h = fnv1a64(FNV1A64_SEED, include_bytes!("obstacle_index.rs"));
    let h = fnv1a64(h, include_bytes!("obstacle_index_file.rs"));
    let h = fnv1a64(h, include_bytes!("../wkb.rs"));
    let h = fnv1a64(h, include_bytes!("../low_profile.rs"));
    fnv1a64(h, include_bytes!("../constants.rs"))
};

/// "Quiet EDGes" — a stray file identifies itself, like the tile store's
/// `QTSI`/`QTSD`. A layout change is an edit to this file, which
/// [`BUILDER_CODE_VER`] hashes, so no format version is needed beside it.
const MAGIC: &[u8; 4] = b"QEDG";
/// The header: magic, then u64 fields at the offsets below; the edge table
/// follows at [`HEADER_BYTES`] (64-aligned, so the mapping's page alignment
/// carries through to the 4-aligned edges) and the class table right after it.
pub const HEADER_BYTES: usize = 128;
const AT_CODE_VER: usize = 8;
const AT_DATA_VER: usize = 16;
const AT_ORIGIN_LAT: usize = 24;
const AT_ORIGIN_LON: usize = 32;
const AT_M_PER_DEG_LON: usize = 40;
const AT_N_EDGES: usize = 48;
const AT_N_FP: usize = 56;
const AT_TOTAL: usize = 64;
const EDGE_BYTES: usize = std::mem::size_of::<ObstacleEdge>();

/// Where the two tables sit and how long the file is, from the counts alone —
/// the writer and the reader compute them with this one function.
fn layout(n_edges: usize, n_fp: usize) -> Option<(usize, usize, usize)> {
    let classes_at = HEADER_BYTES.checked_add(n_edges.checked_mul(EDGE_BYTES)?)?;
    let total = classes_at.checked_add(n_fp)?;
    Some((HEADER_BYTES, classes_at, total))
}

/// The index as bytes to write, in order: [`FileParts::header`] first, then
/// every slice of [`FileParts::sections`]. Concatenating them IS the file — no
/// intermediate buffer, so writing a 500 MB edge table costs no extra RAM.
pub struct FileParts<'a> {
    pub header: [u8; HEADER_BYTES],
    pub sections: Vec<&'a [u8]>,
}

/// Reinterpret a slice of POD values as its raw bytes.
fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: `T` is `u8` or `ObstacleEdge` — `#[repr(C)]` POD with no padding
    // and no pointers — and the result borrows the same memory for the same
    // lifetime, read-only.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

impl ObstacleIndex {
    /// Serialize for [`ObstacleIndex::from_blob`]. `data_ver` is the caller's
    /// fingerprint of the input files this index was built from; it is stored
    /// verbatim and compared on load.
    pub fn file_parts(&self, code_ver: u64, data_ver: u64) -> FileParts<'_> {
        let (_, _, total) = layout(self.edges.len(), self.footprint_class.len())
            .expect("obstacle edge table overflows usize");
        let mut header = [0u8; HEADER_BYTES];
        header[0..4].copy_from_slice(MAGIC);
        let mut put = |at: usize, v: u64| header[at..at + 8].copy_from_slice(&v.to_le_bytes());
        put(AT_CODE_VER, code_ver);
        put(AT_DATA_VER, data_ver);
        put(AT_ORIGIN_LAT, self.origin_lat.to_bits());
        put(AT_ORIGIN_LON, self.origin_lon.to_bits());
        put(AT_M_PER_DEG_LON, self.m_per_deg_lon.to_bits());
        put(AT_N_EDGES, self.edges.len() as u64);
        put(AT_N_FP, self.footprint_class.len() as u64);
        put(AT_TOTAL, total as u64);
        FileParts {
            header,
            sections: vec![as_bytes(&self.edges), as_bytes(&self.footprint_class)],
        }
    }

    /// Whether a file with this header (its first [`HEADER_BYTES`]) and length is
    /// the current edge table for `expect_code_ver` / `expect_data_ver` — the same
    /// judgement [`Self::from_blob`] makes before mapping, without the map and the
    /// grid, so a world sweep can skip a current cell on one small read.
    pub fn file_is_current(
        header: &[u8],
        file_len: usize,
        expect_code_ver: u64,
        expect_data_ver: u64,
    ) -> bool {
        validate(header, file_len, expect_code_ver, expect_data_ver).is_ok()
    }

    /// Map a cell's edge table and derive its grid, or explain why the file
    /// cannot be used.
    ///
    /// Refuses anything whose `code_ver` or `data_ver` differs from the
    /// caller's — a mismatch means the builder or its inputs moved, and the
    /// only safe answer is to rebuild. The edges are not copied: the returned
    /// index reads them straight out of `blob`; only the grid is built, in
    /// memory.
    pub fn from_blob(
        blob: Arc<dyn IndexBlob>,
        expect_code_ver: u64,
        expect_data_ver: u64,
    ) -> Result<ObstacleIndex, String> {
        let bytes = blob.as_bytes();
        let header = validate(bytes, bytes.len(), expect_code_ver, expect_data_ver)?;
        let (edges_at, classes_at, _) = layout(header.n_edges, header.n_fp)
            .ok_or_else(|| "edge table overflows usize".to_string())?;
        let edges = IndexArray::from_blob(&blob, edges_at, header.n_edges)
            .ok_or_else(|| "edges window invalid".to_string())?;
        let footprint_class = IndexArray::from_blob(&blob, classes_at, header.n_fp)
            .ok_or_else(|| "footprint_class window invalid".to_string())?;
        ObstacleIndex::from_edges(
            header.origin_lat,
            header.origin_lon,
            header.m_per_deg_lon,
            edges,
            footprint_class,
        )
    }
}

/// The header fields a reader needs after the checks.
struct Header {
    origin_lat: f64,
    origin_lon: f64,
    m_per_deg_lon: f64,
    n_edges: usize,
    n_fp: usize,
}

/// Every rejection in one place: foreign or truncated header, another builder
/// (`code_ver`), other inputs (`data_ver`), counts the layout cannot hold, and a
/// file shorter than the layout its header announces.
fn validate(
    header: &[u8],
    file_len: usize,
    expect_code_ver: u64,
    expect_data_ver: u64,
) -> Result<Header, String> {
    if header.len() < HEADER_BYTES {
        return Err(format!("truncated header ({} bytes)", header.len()));
    }
    if &header[0..4] != MAGIC {
        return Err(format!("bad magic {:?} (want {MAGIC:?})", &header[0..4]));
    }
    let get = |at: usize| -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&header[at..at + 8]);
        u64::from_le_bytes(b)
    };
    let (code_ver, data_ver) = (get(AT_CODE_VER), get(AT_DATA_VER));
    if code_ver != expect_code_ver {
        return Err(format!("code_ver {code_ver:016x} ≠ {expect_code_ver:016x}"));
    }
    if data_ver != expect_data_ver {
        return Err(format!("data_ver {data_ver:016x} ≠ {expect_data_ver:016x}"));
    }
    let usz = |v: u64| usize::try_from(v).map_err(|_| format!("count {v} exceeds usize"));
    let n_edges = usz(get(AT_N_EDGES))?;
    let n_fp = usz(get(AT_N_FP))?;
    let total = usz(get(AT_TOTAL))?;
    let (_, _, layout_total) =
        layout(n_edges, n_fp).ok_or_else(|| "edge table overflows usize".to_string())?;
    if layout_total != total || file_len < total {
        return Err(format!(
            "size mismatch: header says {total}, layout {layout_total}, file {file_len}"
        ));
    }
    Ok(Header {
        origin_lat: f64::from_bits(get(AT_ORIGIN_LAT)),
        origin_lon: f64::from_bits(get(AT_ORIGIN_LON)),
        m_per_deg_lon: f64::from_bits(get(AT_M_PER_DEG_LON)),
        n_edges,
        n_fp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::obstacle_index::SeenEdges;
    use crate::propagation::obstacle_index::{CrossingCandidate, ObstacleKind};

    /// Flatten `file_parts` the way a writer would.
    fn to_file_bytes(idx: &ObstacleIndex, code_ver: u64, data_ver: u64) -> Vec<u8> {
        let parts = idx.file_parts(code_ver, data_ver);
        let mut out = Vec::new();
        out.extend_from_slice(&parts.header);
        for s in &parts.sections {
            out.extend_from_slice(s);
        }
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
        b.footprint_class = (0..13).map(|i| if i == 5 { 2 } else { 5 }).collect();
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
    fn obstacle_source_ids_survive_built_and_mmap_views() {
        let built = sample_index();
        let bytes = to_file_bytes(&built, 0xabc, 0xdef);
        let mapped = ObstacleIndex::from_blob(Arc::new(bytes), 0xabc, 0xdef).expect("loads");

        assert_eq!(mapped.edge_count(), built.edge_count());
        assert_eq!(mapped.footprint_class[5], 2, "envelope class survives mmap");
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

        let mut skyline_a = Vec::new();
        let mut skyline_b = Vec::new();
        let mut seen_a = SeenEdges::default();
        let mut seen_b = SeenEdges::default();
        built.skyline_arcs_within(
            0,
            OLAT,
            OLON,
            0.0,
            2_000.0,
            0.0,
            0.0,
            None,
            Some(&mut seen_a),
            &mut |arc| skyline_a.push((arc.source_id.bits(), arc.lo.to_bits(), arc.hi.to_bits())),
        );
        mapped.skyline_arcs_within(
            0,
            OLAT,
            OLON,
            0.0,
            2_000.0,
            0.0,
            0.0,
            None,
            Some(&mut seen_b),
            &mut |arc| skyline_b.push((arc.source_id.bits(), arc.lo.to_bits(), arc.hi.to_bits())),
        );
        assert_eq!(skyline_a, skyline_b, "derived source IDs survive mmap");
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

        refuses(
            bytes[..bytes.len() - 1].to_vec(),
            0xabc,
            0xdef,
            "size mismatch",
        );
        refuses(vec![0u8; 8], 0xabc, 0xdef, "truncated");

        // The sweep's skip judgement is the loader's, minus the map.
        let header = &bytes[..HEADER_BYTES];
        assert!(ObstacleIndex::file_is_current(
            header,
            bytes.len(),
            0xabc,
            0xdef
        ));
        assert!(!ObstacleIndex::file_is_current(
            header,
            bytes.len() - 1,
            0xabc,
            0xdef
        ));
        assert!(!ObstacleIndex::file_is_current(
            header,
            bytes.len(),
            0xabc,
            0xde0
        ));
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
