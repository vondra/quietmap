//! The read/write tile store over one `(layer, zoom)` index + data file pair.
//!
//! Write path: reserve space in the data log with an atomic tail bump, `pwrite`
//! the blob, then `pwrite` the 16-byte index entry — the entry IS the commit
//! (a crash between the two leaves invisible garbage at the tail, reclaimed at
//! the next pack/compaction). Parallel `put_*` calls from one process are safe:
//! reservations are disjoint, entries are disjoint.
//!
//! Cross-process write exclusivity is the STORE's own job, not the caller's:
//! every writable `open`/`create` takes an exclusive OS `flock` on a
//! per-(layer,zoom) `z{N}.lock` file BEFORE it recovers the tail or reserves any
//! offset, and holds it until the `TileStore` drops (see [`acquire_write_lock`]).
//! Without it two writer processes can recover the same process-local tail and
//! reserve identical offsets, overwriting each other's blobs. Read-only opens
//! take no lock: they reserve nothing, so they neither block writers nor are
//! blocked by them.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};

use super::format::{Entry, Header, TileCodec, ENTRY_BYTES, HEADER_BYTES, MAGIC_DATA, MAGIC_INDEX};
use super::locks::zoom_store_lock_path;
use crate::wire_hm3;

/// Data-log preallocation step. Appends are sequential, but batching extent
/// allocation keeps concurrent tail writes off the ext4 allocation lock.
const FALLOC_CHUNK: u64 = 256 * 1024 * 1024;
/// zstd level for central working tiles — measured 2026-07-08: 132 µs/tile
/// round-trip at 15.6 % of raw; zstd-3 saved a further 6 % bytes for 29 %
/// slower encode (not worth it), lz4 +46 % bytes (no case).
const ZSTD_LEVEL: i32 = 1;

pub struct TileStore {
    index: File,
    data: File,
    header: Header,
    /// Next append offset in the data log.
    tail: AtomicU64,
    /// Extent-allocation watermark for the data log (see [`FALLOC_CHUNK`]).
    allocated: AtomicU64,
    /// Serialises the rare fallocate grow; the fast path is a lone atomic load.
    grow: Mutex<()>,
    writable: bool,
    /// Exclusive per-(layer,zoom) write flock, held for a writable store's whole
    /// lifetime and released on `Drop`; `None` for a read-only open. An RAII
    /// guard — never read, only dropped — hence the leading underscore. Declared
    /// LAST so it releases only after the index/data fds close: the next writer's
    /// flock is thus granted only once this writer's committed blobs are visible
    /// in the page cache for its tail recovery. (Visibility, not durability —
    /// there is no fsync here; see the crate crash contract in `mod.rs`.)
    _write_lock: Option<File>,
}

fn index_path(dir: &Path, zoom: u8) -> PathBuf {
    dir.join(format!("z{zoom}.qtsi"))
}
fn data_path(dir: &Path, zoom: u8) -> PathBuf {
    dir.join(format!("z{zoom}.qtsd"))
}

/// Acquire the exclusive per-store write flock, BLOCKING until any other writer
/// of this exact `(layer, zoom)` drops its store. Returns the held `File`;
/// dropping it (with the owning `TileStore`) releases the lock.
///
/// The lock file is created if absent and DELIBERATELY never truncated
/// (`truncate(false)`) or unlinked: an unlink-on-drop would let a waiting writer
/// open a fresh inode and lock THAT, so two writers could both "hold" the lock
/// on different inodes — the classic flock-with-unlink race. A stable,
/// never-removed file keeps every writer flock-ing the one inode.
///
/// LOCK ORDERING (deadlock avoidance): this is the INNERMOST of the build's
/// three cross-process write locks. The outer two — the combine/pyramid master
/// (`scripts/world/store-lock.sh`, `.master._combine.lock`) and the hub's
/// per-ingest `.ingest.lock` (`tile_store_ingest.rs`) — are taken before any
/// store opens. A path needing both uses [`super::locks::StoreMasterIngestLocks`]
/// and therefore always takes master→ingest; transcode holds both for its whole
/// replacement, while publish retains both through snapshot validation, archive
/// staging, and manifest durability. This per-store lock is only taken AFTER the outer locks and released (on `Drop`)
/// BEFORE them: no path holds it while acquiring an outer one. Every writer
/// holds exactly ONE per-store write lock at a time: ingest and transcode process one store per loop
/// iteration; a pyramid level opens its read source read-only (unlocked) and one
/// write dst, dropped before the next level; combine reads the layer stores
/// read-only and drops `total` after `sync_all` before rolling the pyramid
/// (`build_heatmap_combine.rs`). With no thread holding two of these locks, there
/// is no lock-ordering cycle to form; a blocking wait here can only be on a peer
/// writer that will finish (a hung live writer would block this innermost flock;
/// the shared outer-pair acquisition is separately bounded).
///
/// CALLER INVARIANT: the flock is per open-file-description, not reentrant per
/// process — a single thread that opens the SAME `(dir, zoom)` writable twice
/// while the first handle is still alive would BLOCK on itself forever. No caller
/// does this (see above); a new one must not either.
fn acquire_write_lock(dir: &Path, zoom: u8) -> Result<File> {
    let path = zoom_store_lock_path(dir, zoom);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open write lock {}", path.display()))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("flock {}", path.display()));
    }
    Ok(lock)
}

impl TileStore {
    /// Create (or truncate) the store for one `(layer dir, zoom)`. The index is
    /// a sparse file sized for every possible tile up front; absent tiles never
    /// touch disk.
    pub fn create(dir: &Path, zoom: u8, source_id: u8, tile_px: u16) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
        // Exclusive write lock BEFORE truncating the store files, so a
        // concurrent writer can never observe or interleave with a
        // half-initialised store (module doc).
        let write_lock = acquire_write_lock(dir, zoom)?;
        let header = Header {
            zoom,
            source_id,
            tile_px,
        };
        let n = u64::from(1u32 << zoom);

        let index = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(index_path(dir, zoom))?;
        index.write_all_at(&header.encode(MAGIC_INDEX), 0)?;
        index.set_len(HEADER_BYTES + n * n * ENTRY_BYTES)?;

        let data = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(data_path(dir, zoom))?;
        data.write_all_at(&header.encode(MAGIC_DATA), 0)?;

        Ok(TileStore {
            index,
            data,
            header,
            tail: AtomicU64::new(HEADER_BYTES),
            allocated: AtomicU64::new(HEADER_BYTES),
            grow: Mutex::new(()),
            writable: true,
            _write_lock: Some(write_lock),
        })
    }

    /// Open an existing store. `writable = false` turns every `put_*`/`delete`
    /// into an error (readers: pack, parity checks, the dev preview).
    pub fn open(dir: &Path, zoom: u8, writable: bool) -> Result<Self> {
        // Cross-process write exclusivity (module doc): a writable open takes the
        // per-store exclusive flock BEFORE opening the data log or recovering the
        // tail below — otherwise two writers seed the same tail and reserve
        // overlapping offsets. Read-only opens reserve nothing, so take no lock.
        let write_lock = if writable {
            Some(acquire_write_lock(dir, zoom)?)
        } else {
            None
        };
        let mut opts = OpenOptions::new();
        opts.read(true).write(writable);
        let index = opts
            .open(index_path(dir, zoom))
            .with_context(|| format!("open {}", index_path(dir, zoom).display()))?;
        let data = opts
            .open(data_path(dir, zoom))
            .with_context(|| format!("open {}", data_path(dir, zoom).display()))?;

        let mut hbuf = [0u8; HEADER_BYTES as usize];
        index.read_exact_at(&mut hbuf, 0)?;
        let header = Header::decode(&hbuf, MAGIC_INDEX)?;
        data.read_exact_at(&mut hbuf, 0)?;
        let dh = Header::decode(&hbuf, MAGIC_DATA)?;
        if dh != header {
            bail!("index/data header mismatch: {header:?} vs {dh:?}");
        }
        if header.zoom != zoom {
            bail!("store zoom {} ≠ requested {zoom}", header.zoom);
        }
        let n = u64::from(1u32 << zoom);
        let want = HEADER_BYTES + n * n * ENTRY_BYTES;
        let got = index.metadata()?.len();
        if got != want {
            bail!("index size {got} ≠ {want} (zoom {zoom})");
        }
        let store = TileStore {
            index,
            data,
            header,
            tail: AtomicU64::new(0),
            allocated: AtomicU64::new(0),
            grow: Mutex::new(()),
            writable,
            _write_lock: write_lock,
        };
        // Tail recovery. With FALLOC_FL_KEEP_SIZE the file length is normally
        // the logical data end — but after a HOST crash an index page holding
        // an entry can be durable while the data file's size update is not
        // (there is no fsync ordering anywhere in this store). A stale short
        // tail would hand out offsets that OVERWRITE committed blobs, so a
        // writable open also takes the max over every indexed blob end — one
        // sequential index scan, paid only when opening to write.
        let mut tail = store.data.metadata()?.len().max(HEADER_BYTES);
        if writable {
            let mut max_end = HEADER_BYTES;
            store.for_each_present(|_, _, e| {
                let end = e
                    .offset
                    .checked_add(u64::from(e.len))
                    .context("indexed data range overflows u64 while recovering write tail")?;
                max_end = max_end.max(end);
                Ok(())
            })?;
            tail = tail.max(max_end);
        }
        store.tail.store(tail, Ordering::Relaxed);
        store.allocated.store(tail, Ordering::Relaxed);
        Ok(store)
    }

    /// The layer discriminator baked into published HM3 headers — read from
    /// the store header (a reader that only `open`s cannot recover it otherwise).
    pub fn source_id(&self) -> u8 {
        self.header.source_id
    }

    /// Stored tile side in pixels. Size-agnostic readers derive cell counts
    /// from this header, never from a constant.
    pub fn tile_px(&self) -> u16 {
        self.header.tile_px
    }

    /// Tiles per side (2^zoom) — derived, never stored.
    fn n(&self) -> u32 {
        1 << self.header.zoom
    }

    fn entry_pos(&self, x: u32, y: u32) -> Result<u64> {
        let n = self.n();
        if x >= n || y >= n {
            bail!("tile {x}/{y} out of range for zoom {}", self.header.zoom);
        }
        Ok(HEADER_BYTES + (u64::from(x) * u64::from(n) + u64::from(y)) * ENTRY_BYTES)
    }

    fn read_entry(&self, x: u32, y: u32) -> Result<Option<Entry>> {
        let mut buf = [0u8; ENTRY_BYTES as usize];
        self.index.read_exact_at(&mut buf, self.entry_pos(x, y)?)?;
        Entry::decode(&buf)
    }

    pub fn present(&self, x: u32, y: u32) -> Result<bool> {
        Ok(self.read_entry(x, y)?.is_some())
    }

    /// Logical length of this handle's pinned data-file inode. Snapshot validators capture it
    /// under the matching writer locks, so a later append cannot make an originally
    /// out-of-bounds index entry appear valid.
    pub fn data_file_len(&self) -> Result<u64> {
        Ok(self.data.metadata()?.len())
    }

    /// The stored bytes exactly as written, with their codec tag.
    pub fn get_blob(&self, x: u32, y: u32) -> Result<Option<(TileCodec, Vec<u8>)>> {
        let Some(e) = self.read_entry(x, y)? else {
            return Ok(None);
        };
        Ok(Some((e.codec, self.get_blob_by_entry(&e)?)))
    }

    /// Blob bytes for an entry already in hand — the packer's scan path: skips
    /// the redundant index pread and enables offset-ordered data-log streaming
    /// (sort scanned entries by `offset`, then read sequentially).
    pub fn get_blob_by_entry(&self, e: &Entry) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; e.len as usize];
        self.data.read_exact_at(&mut buf, e.offset)?;
        Ok(buf)
    }

    /// Decoded dense cells (`tile_px²` bytes), whatever the stored codec.
    pub fn get_cells(&self, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        let Some((codec, blob)) = self.get_blob(x, y)? else {
            return Ok(None);
        };
        self.decode_cells(codec, &blob)
            .with_context(|| format!("tile {x}/{y}"))
            .map(Some)
    }

    /// Decode+validate an already-fetched blob (magic/version/length for
    /// `BrotliHm3`, zstd decode for the working codec, then cell-count check
    /// either way). Split out of [`Self::get_cells`] so a caller that needs to
    /// distinguish "couldn't even fetch the blob" (I/O error, corrupt index —
    /// an infrastructure problem, must stay fatal) from "fetched fine but the
    /// bytes don't decode" (a known, self-healing corrupt-blob class, safe to
    /// treat as absent — see `pyramid::get_cells_lenient`) can call
    /// [`Self::get_blob`] then this separately instead of only getting one
    /// combined `Result`. `pub`, not `pub(crate)`:
    /// `tile-store-fsck` (a separate bin crate) needs the same fetch/decode
    /// split to distinguish an I/O failure from a corrupt blob.
    pub fn decode_cells(&self, codec: TileCodec, blob: &[u8]) -> Result<Vec<u8>> {
        let n_cells = usize::from(self.header.tile_px) * usize::from(self.header.tile_px);
        let cells = match codec {
            TileCodec::BrotliHm3 => wire_hm3::read_tile_bytes(blob)?,
            TileCodec::ZstdCells => zstd::decode_all(std::io::Cursor::new(blob))?,
        };
        if cells.len() != n_cells {
            bail!("decoded {} cells ≠ {n_cells}", cells.len());
        }
        Ok(cells)
    }

    /// A complete HM3 file image (whole-file Brotli) whatever the stored
    /// codec — verbatim for [`TileCodec::BrotliHm3`], composed via
    /// [`wire_hm3::encode_tile_bytes`] for working-codec tiles. The ship-out
    /// path: the pmtiles packer stays codec-blind.
    pub fn get_hm3(&self, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        match self.read_entry(x, y)? {
            Some(e) => self.get_hm3_by_entry(&e).map(Some),
            None => Ok(None),
        }
    }

    /// [`Self::get_hm3`] for an entry already in hand (see
    /// [`Self::get_blob_by_entry`]).
    ///
    /// SHIP-OUT PATH: `BrotliHm3` bytes return VERBATIM, with no downstream
    /// decode or validation. Per-tile validation was deliberately removed after
    /// measuring ~20% of publish CPU; the mandatory pre-publish fsck validates
    /// once per captured layer instead. That gate retains the corruption defense
    /// introduced after a pre-flock-fix blob reached a published archive.
    /// `ZstdCells` remains a legacy path and is decoded into a fresh HM3.
    pub fn get_hm3_by_entry(&self, e: &Entry) -> Result<Vec<u8>> {
        let blob = self.get_blob_by_entry(e)?;
        match e.codec {
            TileCodec::BrotliHm3 => Ok(blob),
            TileCodec::ZstdCells => {
                // decode_cells' length-check bail, not encode_tile_bytes' assert —
                // keeps a corrupt store or codec-tag mismatch a Result, never a panic.
                let cells = self
                    .decode_cells(TileCodec::ZstdCells, &blob)
                    .with_context(|| format!("entry at offset {}", e.offset))?;
                Ok(wire_hm3::encode_tile_bytes(&cells, self.header.source_id)?)
            }
        }
    }

    /// Decode-validate an entry WITHOUT composing a ship-out image — the
    /// validating twin [`Self::get_hm3_by_entry`] folded into every read
    /// before 2026-07-16 (see that method's doc for why the two split).
    /// `tile-store-fsck` calls this for `BrotliHm3` entries so the MANDATORY
    /// pre-publish check (`tile-store-pack`) still catches a corrupt or
    /// aliased blob before it ships verbatim (the 2026-07-14 industrial/z12
    /// class). Same checks `get_hm3_by_entry` used to run: decode +
    /// magic/version/length, then the source_id must match this store's own
    /// layer. `ZstdCells` reduces to [`Self::decode_cells`]'s own length
    /// check — callers that already call `get_blob_by_entry` +
    /// `decode_cells` directly for that arm (fsck does) have no reason to
    /// route through here too.
    pub fn validate_hm3_by_entry(&self, e: &Entry) -> Result<()> {
        let blob = self.get_blob_by_entry(e)?;
        match e.codec {
            TileCodec::BrotliHm3 => {
                let sid = wire_hm3::read_tile_bytes_source_id(&blob).with_context(|| {
                    format!("entry at offset {}: corrupt BrotliHm3 blob", e.offset)
                })?;
                if sid != self.header.source_id {
                    bail!(
                        "entry at offset {}: source_id {sid} ≠ store {}",
                        e.offset,
                        self.header.source_id
                    );
                }
                Ok(())
            }
            TileCodec::ZstdCells => {
                self.decode_cells(TileCodec::ZstdCells, &blob)
                    .with_context(|| format!("entry at offset {}", e.offset))?;
                Ok(())
            }
        }
    }

    /// Store pre-encoded bytes verbatim (fleet Brotli blobs, transcode).
    ///
    /// The present ⟺ audible invariant is enforced by [`Self::put_cells`];
    /// `put_blob` trusts the caller not to hand it an all-silent blob
    /// (migration blobs are non-silent by construction — `write_tile` never
    /// wrote silent tiles).
    ///
    /// INVARIANT: a `BrotliHm3` blob ships VERBATIM from here into PMTiles;
    /// nothing downstream re-decodes it at ship-out time.
    ///
    /// A writer that stores an externally produced blob it did not just
    /// encode itself (tile-store-ingest, tile-store-transcode: fleet bytes,
    /// or bytes read back from a loose staging tree) MUST validate it (decode +
    /// magic/version/size) before calling `put_blob` — a new such writer must
    /// add the same gate. [`Self::put_cells_hm3`] is
    /// exempt: it encodes the blob itself, in this same call, from cells the
    /// caller already holds valid — there is no external byte stream to
    /// distrust, only a hypothetical bug in `encode_tile_bytes` itself, which
    /// a decode-right-back-again wouldn't catch any more reliably than the
    /// mandatory fsck pass does before publish.
    pub fn put_blob(&self, x: u32, y: u32, codec: TileCodec, blob: &[u8]) -> Result<()> {
        if !self.writable {
            bail!("store opened read-only");
        }
        if blob.is_empty() {
            bail!("empty blob for {x}/{y} — use delete()");
        }
        let pos = self.entry_pos(x, y)?; // validate coords BEFORE reserving log space
        let len = u32::try_from(blob.len())
            .with_context(|| format!("blob for {x}/{y}: {} B exceeds u32", blob.len()))?;
        let len_u64 = u64::from(len);
        // A wrapping fetch_add would corrupt the process-local tail even though the checked_add
        // below reports this one write. Reserve with one atomic checked update instead.
        let offset = self
            .tail
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |offset| {
                offset.checked_add(len_u64)
            })
            .map_err(|_| anyhow::anyhow!("data log offset overflow"))?;
        let end = offset
            .checked_add(len_u64)
            .context("data log offset overflow")?;
        self.ensure_allocated(end)?;
        self.data.write_all_at(blob, offset)?;
        self.index
            .write_all_at(&Entry { offset, len, codec }.encode(), pos)?;
        Ok(())
    }

    /// Encode cells with the working codec and store them. All-`NO_DATA` cells
    /// delete the tile instead (mirrors `write_tile`'s `skip_if_empty`).
    /// Returns stored bytes (0 = deleted-as-empty).
    pub fn put_cells(&self, x: u32, y: u32, cells: &[u8]) -> Result<usize> {
        let n_cells = usize::from(self.header.tile_px) * usize::from(self.header.tile_px);
        if cells.len() != n_cells {
            bail!("put_cells {x}/{y}: {} cells ≠ {n_cells}", cells.len());
        }
        if wire_hm3::is_silent(cells) {
            self.delete(x, y)?;
            return Ok(0);
        }
        let blob = zstd::encode_all(std::io::Cursor::new(cells), ZSTD_LEVEL)?;
        self.put_blob(x, y, TileCodec::ZstdCells, &blob)?;
        Ok(blob.len())
    }

    /// Encode cells with the SHIP-OUT codec ([`TileCodec::BrotliHm3`], the
    /// same quality-9 whole-file Brotli [`wire_hm3::encode_tile_bytes`]
    /// produces for a loose `.bin`) and store them — the central-writer path
    /// since 2026-07-16 (`build_heatmap_combine`'s `total` writes,
    /// `pyramid::build_one_level`'s pyramid levels), replacing the old
    /// `put_cells`/`ZstdCells` working-codec write for anything that will
    /// eventually ship to a publish. This moves the Brotli-q9 encode to
    /// write time — once per pyramid tile per drain batch, amortized over
    /// the days between publishes — instead of `tile-store-pack` redoing it
    /// for every tile on every publish (measured ~63 min for one 580k-tile
    /// layer before this change; see [`TileCodec::ZstdCells`]'s doc). All-
    /// `NO_DATA` cells delete the tile instead, mirroring `put_cells`.
    /// Returns stored bytes (0 = deleted-as-empty).
    pub fn put_cells_hm3(&self, x: u32, y: u32, cells: &[u8]) -> Result<usize> {
        let n_cells = usize::from(self.header.tile_px) * usize::from(self.header.tile_px);
        if cells.len() != n_cells {
            bail!("put_cells_hm3 {x}/{y}: {} cells ≠ {n_cells}", cells.len());
        }
        if wire_hm3::is_silent(cells) {
            self.delete(x, y)?;
            return Ok(0);
        }
        // HM3 v3 has one global 512-pixel tile size, while a store header can
        // expose a stale size. Reject that mismatch before encode_tile_bytes
        // asserts, so one bad store cannot panic a pyramid/combine run.
        if usize::from(self.header.tile_px) != crate::grid::TILE_PX {
            bail!(
                "put_cells_hm3 {x}/{y}: store tile_px {} ≠ current build TILE_PX {} — BrotliHm3 \
                 (HM3 v3) is locked to the current tile size; this store needs a full repaint at \
                 the new size before it can use the ship-out write path",
                self.header.tile_px,
                crate::grid::TILE_PX
            );
        }
        let blob = wire_hm3::encode_tile_bytes(cells, self.header.source_id)?;
        self.put_blob(x, y, TileCodec::BrotliHm3, &blob)?;
        Ok(blob.len())
    }

    /// fsync both files. Deletion-gating tools MUST call this before treating
    /// a green run as license to delete their source — the parity gates verify
    /// the page cache; only this makes the store power-loss durable.
    pub fn sync_all(&self) -> Result<()> {
        self.data.sync_all()?;
        self.index.sync_all()?;
        Ok(())
    }

    /// Tombstone: zero the entry. Blob bytes are reclaimed at pack time.
    pub fn delete(&self, x: u32, y: u32) -> Result<()> {
        if !self.writable {
            bail!("store opened read-only");
        }
        self.index
            .write_all_at(&[0u8; ENTRY_BYTES as usize], self.entry_pos(x, y)?)?;
        Ok(())
    }

    /// Sequential scan of the index; calls `f(x, y, entry)` for every present
    /// tile. Column-major (x outer), matching the legacy tree walk order.
    pub fn for_each_present(&self, f: impl FnMut(u32, u32, Entry) -> Result<()>) -> Result<()> {
        self.for_each_present_in_x_range(0, self.n() - 1, f)
    }

    /// [`Self::for_each_present`] restricted to columns `x0..=x1`. The index is
    /// addressed `[x·n + y]`, so an x-range is ONE contiguous slab — a bbox
    /// scan reads only its region's entries, never the whole index.
    pub fn for_each_present_in_x_range(
        &self,
        x0: u32,
        x1: u32,
        mut f: impl FnMut(u32, u32, Entry) -> Result<()>,
    ) -> Result<()> {
        const CHUNK_ENTRIES: usize = 64 * 1024;
        let n = self.n();
        if x0 >= n {
            return Ok(());
        }
        let x1 = x1.min(n - 1);
        let end = (u64::from(x1) + 1) * u64::from(n);
        let mut buf = vec![0u8; CHUNK_ENTRIES * ENTRY_BYTES as usize];
        let (mut done, mut x, mut y) = (u64::from(x0) * u64::from(n), x0, 0u32);
        while done < end {
            let batch = CHUNK_ENTRIES.min((end - done) as usize);
            let bytes = &mut buf[..batch * ENTRY_BYTES as usize];
            self.index
                .read_exact_at(bytes, HEADER_BYTES + done * ENTRY_BYTES)?;
            for raw in bytes.chunks_exact(ENTRY_BYTES as usize) {
                if let Some(e) = Entry::decode(raw)? {
                    f(x, y, e)?;
                }
                y += 1;
                if y == n {
                    y = 0;
                    x += 1;
                }
            }
            done += batch as u64;
        }
        Ok(())
    }

    /// Batch extent allocation for the data log (see [`FALLOC_CHUNK`]). The
    /// fast path is one atomic load — 39 M transcode puts must not queue on a
    /// mutex that only the ~200 actual grows need.
    fn ensure_allocated(&self, needed: u64) -> Result<()> {
        if needed <= self.allocated.load(Ordering::Acquire) {
            return Ok(());
        }
        let _grow = self.grow.lock().unwrap();
        let current = self.allocated.load(Ordering::Acquire);
        if needed <= current {
            return Ok(()); // another thread grew past us while we queued
        }
        let new_end = needed.div_ceil(FALLOC_CHUNK) * FALLOC_CHUNK;
        // FALLOC_FL_KEEP_SIZE: preallocate extents WITHOUT extending the file
        // size, so `metadata().len()` stays the logical data end and `open()`
        // resumes appending exactly at the tail (no dead gap after a restart).
        let rc = unsafe {
            libc::fallocate(
                self.data.as_raw_fd(),
                libc::FALLOC_FL_KEEP_SIZE,
                current as libc::off_t,
                (new_end - current) as libc::off_t,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("fallocate data log");
        }
        self.allocated.store(new_end, Ordering::Release);
        Ok(())
    }
}

/// Layer dirs = subdirs of the store root that contain at least one `z*.qtsi`.
/// Shared by `tile-store-pack` and `tile-store-fsck` — both need to discover
/// every store under a root before walking it.
pub fn detect_layers(store_root: &Path) -> Result<Vec<String>> {
    let mut layers = Vec::new();
    for e in std::fs::read_dir(store_root).with_context(|| store_root.display().to_string())? {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let dir = e.path();
        let has_store = std::fs::read_dir(&dir)?.any(|f| {
            f.ok()
                .and_then(|f| f.file_name().to_str().map(|n| n.ends_with(".qtsi")))
                .unwrap_or(false)
        });
        if has_store {
            layers.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    layers.sort();
    Ok(layers)
}

/// Zooms present in one layer store dir (from `z{N}.qtsi` filenames).
pub fn detect_zooms(layer_dir: &Path) -> Result<Vec<u8>> {
    let mut zooms: Vec<u8> = std::fs::read_dir(layer_dir)?
        .filter_map(|e| {
            let name = e.ok()?.file_name();
            let n = name.to_str()?;
            n.strip_prefix('z')?.strip_suffix(".qtsi")?.parse().ok()
        })
        .collect();
    zooms.sort_unstable();
    Ok(zooms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::TILE_PX;
    use crate::wire_hm3::{NO_DATA, SOURCE_ID_RAIL, SOURCE_ID_ROAD};

    fn cells_with(vals: &[(usize, u8)]) -> Vec<u8> {
        let mut c = vec![NO_DATA; TILE_PX * TILE_PX];
        for &(i, v) in vals {
            c[i] = v;
        }
        c
    }

    #[test]
    fn create_put_get_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let cells = cells_with(&[(0, 10), (777, 42), (65_535, 254)]);
        let stored = s.put_cells(3, 5, &cells).unwrap();
        assert!(stored > 0);
        assert!(s.present(3, 5).unwrap());
        assert_eq!(s.get_cells(3, 5).unwrap().unwrap(), cells);
        assert!(s.get_cells(3, 6).unwrap().is_none());
    }

    #[test]
    fn reopen_sees_data_and_appends_after_tail() {
        let dir = tempfile::tempdir().unwrap();
        let cells = cells_with(&[(1, 99)]);
        {
            let s = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
            s.put_cells(0, 0, &cells).unwrap();
        }
        let s = TileStore::open(dir.path(), 6, true).unwrap();
        assert_eq!(s.get_cells(0, 0).unwrap().unwrap(), cells);
        let more = cells_with(&[(2, 50)]);
        s.put_cells(1, 1, &more).unwrap();
        assert_eq!(s.get_cells(1, 1).unwrap().unwrap(), more);
        assert_eq!(
            s.get_cells(0, 0).unwrap().unwrap(),
            cells,
            "old tile intact"
        );
    }

    #[test]
    fn brotli_blob_round_trips_through_get_cells() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        // Build an HM3 blob via write_tile and store it verbatim.
        let cells = cells_with(&[(123, 77)]);
        let tmp = dir.path().join("legacy.bin");
        wire_hm3::write_tile(&tmp, &cells, SOURCE_ID_ROAD, false).unwrap();
        let blob = std::fs::read(&tmp).unwrap();
        s.put_blob(9, 9, TileCodec::BrotliHm3, &blob).unwrap();
        let (codec, back) = s.get_blob(9, 9).unwrap().unwrap();
        assert_eq!(codec, TileCodec::BrotliHm3);
        assert_eq!(back, blob, "blob stored verbatim");
        assert_eq!(s.get_cells(9, 9).unwrap().unwrap(), cells);
    }

    #[test]
    fn empty_cells_delete_and_overwrite_updates() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let cells = cells_with(&[(5, 60)]);
        s.put_cells(2, 2, &cells).unwrap();
        assert!(s.present(2, 2).unwrap());
        // Rebuild-to-silence: storing all-NO_DATA removes the tile.
        assert_eq!(
            s.put_cells(2, 2, &vec![NO_DATA; TILE_PX * TILE_PX])
                .unwrap(),
            0
        );
        assert!(!s.present(2, 2).unwrap());
        // Overwrite points the entry at the new blob.
        let v2 = cells_with(&[(5, 61)]);
        s.put_cells(2, 2, &v2).unwrap();
        assert_eq!(s.get_cells(2, 2).unwrap().unwrap(), v2);
    }

    #[test]
    fn for_each_present_scans_in_column_major_order() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 4, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let cells = cells_with(&[(0, 1)]);
        for (x, y) in [(2u32, 7u32), (0, 3), (15, 15), (2, 1)] {
            s.put_cells(x, y, &cells).unwrap();
        }
        let mut seen = Vec::new();
        s.for_each_present(|x, y, e| {
            assert!(e.len > 0);
            seen.push((x, y));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![(0, 3), (2, 1), (2, 7), (15, 15)]);

        // Range slab: only columns 1..=2 → exactly the x=2 tiles, in order.
        let mut ranged = Vec::new();
        s.for_each_present_in_x_range(1, 2, |x, y, _| {
            ranged.push((x, y));
            Ok(())
        })
        .unwrap();
        assert_eq!(ranged, vec![(2, 1), (2, 7)]);
    }

    #[test]
    fn writable_open_recovers_tail_from_index_after_stale_data_len() {
        let dir = tempfile::tempdir().unwrap();
        let a = cells_with(&[(9, 70), (100, 71)]);
        let b = cells_with(&[(10, 60), (200, 61)]);
        {
            let s = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
            s.put_cells(4, 4, &a).unwrap();
            s.put_cells(4, 5, &b).unwrap();
        }
        let dpath = dir.path().join("z6.qtsd");
        let full = std::fs::metadata(&dpath).unwrap().len();
        // Simulate the host-crash window: index entries durable, data inode
        // size update lost — the data log reports a stale short length.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&dpath)
            .unwrap()
            .set_len(HEADER_BYTES + 1)
            .unwrap();

        let s = TileStore::open(dir.path(), 6, true).unwrap();
        let more = cells_with(&[(3, 40)]);
        let stored = s.put_cells(5, 5, &more).unwrap() as u64;
        // The new blob must land AFTER the highest indexed end (recovered from
        // the index), never over the committed entries' ranges.
        assert_eq!(std::fs::metadata(&dpath).unwrap().len(), full + stored);
        assert_eq!(s.get_cells(5, 5).unwrap().unwrap(), more);
        // The truncated old blobs are the documented torn-data case: they fail
        // loudly (read/decode error), they are never silently wrong.
        assert!(s.get_cells(4, 4).is_err());
    }

    #[test]
    fn writable_open_rejects_an_indexed_range_that_overflows_u64() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
            store.put_cells(4, 4, &cells_with(&[(9, 70)])).unwrap();
            store.sync_all().unwrap();
        }

        let index_path = dir.path().join("z6.qtsi");
        let index = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(index_path)
            .unwrap();
        let entry_pos =
            HEADER_BYTES + (u64::from(4u32) * u64::from(1u32 << 6) + u64::from(4u32)) * ENTRY_BYTES;
        let mut raw = [0u8; ENTRY_BYTES as usize];
        index.read_exact_at(&mut raw, entry_pos).unwrap();
        raw[..8].copy_from_slice(&(u64::MAX - 1).to_le_bytes());
        index.write_all_at(&raw, entry_pos).unwrap();
        drop(index);

        let error = TileStore::open(dir.path(), 6, true)
            .err()
            .expect("overflowing indexed range must fail instead of panic or wrap");
        assert!(error.to_string().contains("overflows u64"));
    }

    #[test]
    fn failed_tail_reservation_does_not_wrap_the_atomic_tail() {
        let dir = tempfile::tempdir().unwrap();
        let store = TileStore::create(dir.path(), 6, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        store.tail.store(u64::MAX - 1, Ordering::Relaxed);

        let error = store
            .put_blob(1, 1, TileCodec::BrotliHm3, b"xx")
            .expect_err("reservation past u64::MAX must fail");
        assert!(error.to_string().contains("offset overflow"));
        assert_eq!(store.tail.load(Ordering::Relaxed), u64::MAX - 1);
        assert!(!store.present(1, 1).unwrap());
    }

    #[test]
    fn get_hm3_ships_brotli_verbatim_and_composes_zstd() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 5, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        // Verbatim path: a stored HM3 blob comes back byte-identical.
        let cells = cells_with(&[(7, 33)]);
        let blob = wire_hm3::encode_tile_bytes(&cells, SOURCE_ID_ROAD).unwrap();
        s.put_blob(1, 2, TileCodec::BrotliHm3, &blob).unwrap();
        assert_eq!(s.get_hm3(1, 2).unwrap().unwrap(), blob);
        // Compose path: a zstd working tile ships as a valid HM3 image.
        s.put_cells(3, 4, &cells).unwrap();
        let shipped = s.get_hm3(3, 4).unwrap().unwrap();
        assert_eq!(wire_hm3::read_tile_bytes(&shipped).unwrap(), cells);
        assert!(s.get_hm3(0, 0).unwrap().is_none());
    }

    #[test]
    fn get_hm3_ships_corrupt_brotli_blob_verbatim_but_validate_rejects_it() {
        // The 2026-07-13 concurrent-writer bug left invalid Brotli/HM3 bytes
        // under an otherwise-readable index entry. Between 2026-07-14 and
        // 2026-07-16 get_hm3_by_entry itself decode-validated every BrotliHm3
        // blob to catch this before it rode verbatim into a published
        // archive — but that made EVERY read (i.e. every publish) pay the
        // decode cost, not just the one pre-publish check that actually
        // needs it. 2026-07-16: ship-out went back to verbatim, and the
        // validating twin (validate_hm3_by_entry) moved to tile-store-fsck,
        // which worldctl's publish verb now runs as a MANDATORY gate before
        // every tile-store-pack invocation — so the corrupt blob is still
        // caught, just once per publish instead of once per read.
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 5, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        s.put_blob(1, 1, TileCodec::BrotliHm3, b"not a brotli stream")
            .unwrap();
        assert_eq!(
            s.get_hm3(1, 1).unwrap().unwrap(),
            b"not a brotli stream",
            "ship-out is verbatim even for a corrupt blob now — validation moved off this path"
        );
        let mut entry = None;
        s.for_each_present(|x, y, e| {
            if (x, y) == (1, 1) {
                entry = Some(e);
            }
            Ok(())
        })
        .unwrap();
        assert!(
            s.validate_hm3_by_entry(&entry.unwrap()).is_err(),
            "the mandatory pre-publish fsck path must still reject a corrupt blob"
        );
        // The same corruption is what get_cells (and pyramid/combine's
        // decode_cells-based get_cells_lenient) must be resilient to instead.
        assert!(s.get_cells(1, 1).is_err());
    }

    #[test]
    fn put_cells_hm3_round_trips_and_ships_verbatim() {
        // The new central-writer path (2026-07-16): combine/pyramid write
        // BrotliHm3 directly instead of the legacy ZstdCells working codec,
        // moving the Brotli-q9 encode to write time so publish no longer
        // redoes it. Cells must read back identical via get_cells, and the
        // stored blob must be exactly what get_hm3 ships (pack's own
        // ship-out, get_hm3_by_entry, is a verbatim copy for BrotliHm3).
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();
        let cells = cells_with(&[(0, 88), (12_345, 200)]);
        let stored = s.put_cells_hm3(2, 3, &cells).unwrap();
        assert!(stored > 0);
        let (codec, blob) = s.get_blob(2, 3).unwrap().unwrap();
        assert_eq!(
            codec,
            TileCodec::BrotliHm3,
            "central writer now tags BrotliHm3"
        );
        assert_eq!(s.get_cells(2, 3).unwrap().unwrap(), cells);
        assert_eq!(
            s.get_hm3(2, 3).unwrap().unwrap(),
            blob,
            "ship-out is the stored blob verbatim"
        );
        assert_eq!(wire_hm3::read_tile_bytes(&blob).unwrap(), cells);
    }

    #[test]
    fn put_cells_hm3_all_silent_deletes_tile() {
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();
        assert_eq!(
            s.put_cells_hm3(0, 0, &vec![NO_DATA; TILE_PX * TILE_PX])
                .unwrap(),
            0
        );
        assert!(!s.present(0, 0).unwrap());
    }

    #[test]
    fn mixed_codec_store_both_present_read_and_ship_correctly() {
        // A store mid-cutover: some entries already rewritten through the new
        // put_cells_hm3 (BrotliHm3) path, some still legacy put_cells
        // (ZstdCells) — both codecs must coexist in one (layer, zoom) store
        // (the per-entry codec tag is exactly what makes that safe) and both
        // must decode and ship correctly.
        let dir = tempfile::tempdir().unwrap();
        let s = TileStore::create(dir.path(), 6, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();
        let cells_a = cells_with(&[(1, 50)]);
        let cells_b = cells_with(&[(2, 60)]);
        s.put_cells(0, 0, &cells_a).unwrap(); // legacy ZstdCells
        s.put_cells_hm3(0, 1, &cells_b).unwrap(); // new BrotliHm3
        assert_eq!(s.get_blob(0, 0).unwrap().unwrap().0, TileCodec::ZstdCells);
        assert_eq!(s.get_blob(0, 1).unwrap().unwrap().0, TileCodec::BrotliHm3);
        assert_eq!(s.get_cells(0, 0).unwrap().unwrap(), cells_a);
        assert_eq!(s.get_cells(0, 1).unwrap().unwrap(), cells_b);
        // Both ship as valid HM3 images regardless of source codec.
        assert_eq!(
            wire_hm3::read_tile_bytes(&s.get_hm3(0, 0).unwrap().unwrap()).unwrap(),
            cells_a
        );
        assert_eq!(
            wire_hm3::read_tile_bytes(&s.get_hm3(0, 1).unwrap().unwrap()).unwrap(),
            cells_b
        );
    }

    #[test]
    fn readonly_store_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        TileStore::create(dir.path(), 4, SOURCE_ID_ROAD, TILE_PX as u16).unwrap();
        let s = TileStore::open(dir.path(), 4, false).unwrap();
        assert!(s.put_cells(0, 0, &cells_with(&[(0, 1)])).is_err());
        assert!(s.delete(0, 0).is_err());
    }

    /// A writable store must hold the per-(layer,zoom) exclusive flock for its
    /// whole life; a read-only open must take no lock. Deterministic: an
    /// independent `LOCK_EX | LOCK_NB` probe on the lock file is exactly what a
    /// second writer's blocking `flock` would attempt, so its success/failure
    /// reports whether the lock is held — no threads, no sleeps.
    #[test]
    fn writable_open_holds_exclusive_flock_readonly_takes_none() {
        use std::os::unix::io::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        drop(TileStore::create(dir.path(), 4, SOURCE_ID_ROAD, TILE_PX as u16).unwrap());

        // True iff the per-store lock is currently FREE. Non-blocking so the
        // test never hangs; the fresh fd closes at return, releasing anything
        // this probe itself grabbed.
        let free = || -> bool {
            let f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(zoom_store_lock_path(dir.path(), 4))
                .unwrap();
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
        };

        assert!(free(), "lock is free before any writable open");
        let w = TileStore::open(dir.path(), 4, true).unwrap();
        assert!(
            !free(),
            "a writable open must hold the exclusive per-store lock"
        );
        // A read-only open reserves nothing → takes no lock and cannot disturb
        // the writer's hold.
        let r = TileStore::open(dir.path(), 4, false).unwrap();
        assert!(
            !free(),
            "the writer's lock survives an interleaved read-only open"
        );
        drop(r);
        assert!(!free(), "read-only drop must not release the writer's lock");
        drop(w);
        assert!(free(), "the lock releases when the writable store drops");
    }

    /// End-to-end: two writable `open`s of the same store SERIALIZE (the second
    /// blocks until the first drops), while a read-only open never blocks behind
    /// the writer. This is the anti-corruption invariant — without the flock the
    /// second writer would seed the same tail and reserve overlapping offsets.
    #[test]
    fn concurrent_writable_opens_serialize_reads_never_block() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        drop(TileStore::create(dir.path(), 4, SOURCE_ID_ROAD, TILE_PX as u16).unwrap());

        let w1 = TileStore::open(dir.path(), 4, true).unwrap(); // holds the write lock

        // A read-only open must return promptly even while w1 holds the lock.
        let (rtx, rrx) = mpsc::channel();
        let rp = dir.path().to_path_buf();
        let rh = std::thread::spawn(move || {
            let _r = TileStore::open(&rp, 4, false).unwrap();
            rtx.send(()).unwrap();
        });
        rrx.recv_timeout(Duration::from_secs(10))
            .expect("read-only open blocked behind the writer's lock");
        rh.join().unwrap();

        // A second writable open must BLOCK while w1 is alive, then succeed once
        // w1 drops. The `started` handshake fires immediately BEFORE the blocking
        // open, so the 250 ms negative window measures a thread that has actually
        // reached the flock — not one merely slow to be scheduled (which would
        // pass this check even with no lock at all).
        let (started_tx, started_rx) = mpsc::channel();
        let (wtx, wrx) = mpsc::channel();
        let wp = dir.path().to_path_buf();
        let wh = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _w2 = TileStore::open(&wp, 4, true).unwrap(); // blocks until w1 drops
            wtx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            wrx.recv_timeout(Duration::from_millis(250)).is_err(),
            "second writable open acquired the lock while the first was still held"
        );
        drop(w1);
        wrx.recv_timeout(Duration::from_secs(10))
            .expect("second writable open never unblocked after the first dropped");
        wh.join().unwrap();
    }
}
