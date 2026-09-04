//! Memory-mapped node coordinate cache.
//!
//! Stores lat/lon as i32 microdegrees (×1e7) in a sparse file indexed by node ID.
//! File layout: node_id × 8 bytes → [lat_i32, lon_i32].
//! Sparse file on NVMe — OS only allocates pages that are written.

use anyhow::Result;
use memmap2::MmapMut;
use osmpbf::{Element, ElementReader};
use std::fs::OpenOptions;
use std::path::Path;

/// Upper bound on OSM node IDs — the global id counter, ~13.7 B in our
/// Apr-2026 CZ PBF and growing ~2 B/yr. Sized with years of headroom because
/// the cache file is SPARSE: ids above the real max cost virtual address
/// space only, never resident pages. A node whose id is >= this bound is
/// silently lost (every way referencing it loses that vertex → gaps / false
/// long edges), so keep this comfortably above the real max.
const MAX_NODE_ID: u64 = 25_000_000_000;
const ENTRY_SIZE: u64 = 8; // 4 bytes lat + 4 bytes lon

pub struct NodeCache {
    mmap: MmapMut,
    count: u64,
}

/// Raw shared pointer wrapper for parallel writes to disjoint mmap offsets.
///
/// Safety: every write targets a unique `node_id`, so the per-node 8-byte
/// slice is never touched by another thread. The `MmapMut` is kept alive
/// by the owning `NodeCache::build` for the whole scope of par_map_reduce.
#[derive(Clone, Copy)]
struct MmapWriter {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: disjoint writes, see MmapWriter doc.
unsafe impl Send for MmapWriter {}
unsafe impl Sync for MmapWriter {}

/// Outcome of a single node write. Distinguishes the two silent-drop causes
/// (id above the cap vs mmap offset overflow) from a real write so `build`
/// can hard-fail instead of losing geometry without a trace.
enum WriteOutcome {
    Written,
    OverCap,
    Oob,
}

impl MmapWriter {
    #[inline]
    fn write(&self, node_id: u64, lat: f64, lon: f64) -> WriteOutcome {
        if node_id >= MAX_NODE_ID {
            return WriteOutcome::OverCap;
        }
        let offset = (node_id * ENTRY_SIZE) as usize;
        if offset + 8 > self.len {
            return WriteOutcome::Oob;
        }
        let lat_i32 = (lat * 1e7) as i32;
        let lon_i32 = (lon * 1e7) as i32;
        // SAFETY: disjoint per-node 8-byte slices; offset + 8 <= len checked above.
        unsafe {
            let p = self.ptr.add(offset);
            std::ptr::copy_nonoverlapping(lat_i32.to_le_bytes().as_ptr(), p, 4);
            std::ptr::copy_nonoverlapping(lon_i32.to_le_bytes().as_ptr(), p.add(4), 4);
        }
        WriteOutcome::Written
    }
}

/// Reduced node tally across all PBF blocks. `max_id_seen` lets the success
/// log show headroom to `MAX_NODE_ID`; the drop counters trigger a hard fail
/// so a cap-exhausted extract can never silently ship corrupt geometry — the
/// exact failure mode this de-silencing fixes.
#[derive(Clone, Copy, Default)]
struct Tally {
    written: u64,
    dropped_over_cap: u64,
    dropped_oob: u64,
    max_id_seen: u64,
}

impl Tally {
    fn merge(self, other: Tally) -> Tally {
        Tally {
            written: self.written + other.written,
            dropped_over_cap: self.dropped_over_cap + other.dropped_over_cap,
            dropped_oob: self.dropped_oob + other.dropped_oob,
            max_id_seen: self.max_id_seen.max(other.max_id_seen),
        }
    }
}

impl NodeCache {
    /// Build the node cache by streaming all nodes from the PBF.
    ///
    /// Uses `par_map_reduce` — each rayon worker decodes PBF blocks
    /// independently and writes to the shared sparse mmap. Writes are
    /// per-node-id (8 bytes at `node_id * 8` offset), so worker threads
    /// never touch the same byte range.
    pub fn build(pbf_path: &Path, cache_path: &Path) -> Result<Self> {
        let file_size = MAX_NODE_ID * ENTRY_SIZE;

        // QM_REUSE_NODE_CACHE=1: mmap an existing full-size cache instead of
        // re-streaming the planet's nodes (~3 h). SAFE ONLY when the cache was
        // fully built from the SAME pbf — the file content is a pure function
        // of the pbf, and a build that died mid-node-pass left a SMALLER file
        // (set_len happens first, but a crashed run is the operator's risk to
        // assess — this is an explicit opt-in recovery knob, not a default).
        // Added for the 2026-06-12 phase-2 world re-extract restart (disk
        // rescope after the full-scope spill outgrew the host).
        if std::env::var("QM_REUSE_NODE_CACHE").as_deref() == Ok("1") {
            let file = OpenOptions::new().read(true).write(true).open(cache_path);
            if let Ok(file) = file {
                if file.metadata().map(|m| m.len()).ok() == Some(file_size) {
                    eprintln!(
                        "  node cache: REUSING {} (QM_REUSE_NODE_CACHE=1)",
                        cache_path.display()
                    );
                    let mmap = unsafe { MmapMut::map_mut(&file)? };
                    return Ok(NodeCache { mmap, count: 0 });
                }
            }
            anyhow::bail!(
                "QM_REUSE_NODE_CACHE=1 but {} is missing or not exactly {} bytes — \
                 refusing to silently rebuild; unset the env to build fresh",
                cache_path.display(),
                file_size
            );
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(cache_path)?;
        file.set_len(file_size)?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };

        let writer = MmapWriter {
            ptr: mmap.as_mut_ptr(),
            len: mmap.len(),
        };
        let reader = ElementReader::from_path(pbf_path)?;
        let tally = reader.par_map_reduce(
            |element| {
                let (id, lat, lon) = match element {
                    Element::Node(n) => (n.id() as u64, n.lat(), n.lon()),
                    Element::DenseNode(n) => (n.id as u64, n.lat(), n.lon()),
                    _ => return Tally::default(),
                };
                let mut t = Tally {
                    max_id_seen: id,
                    ..Tally::default()
                };
                match writer.write(id, lat, lon) {
                    WriteOutcome::Written => t.written = 1,
                    WriteOutcome::OverCap => t.dropped_over_cap = 1,
                    WriteOutcome::Oob => t.dropped_oob = 1,
                }
                t
            },
            Tally::default,
            Tally::merge,
        )?;

        // Hard-fail on any drop BEFORE flushing the (large) cache to disk: the
        // bug this fixes was silence — a dropped node makes every way referencing
        // it lose that vertex (truncation / false long edges), so never finalize
        // a cap-exhausted extract.
        if tally.dropped_over_cap > 0 {
            anyhow::bail!(
                "node cache: {} nodes have id >= MAX_NODE_ID ({}) and were DROPPED \
                 (highest id seen {}). Raise MAX_NODE_ID above the highest id in \
                 engine/osm-extract/src/node_cache.rs and re-extract.",
                tally.dropped_over_cap,
                MAX_NODE_ID,
                tally.max_id_seen
            );
        }
        if tally.dropped_oob > 0 {
            anyhow::bail!(
                "node cache: {} nodes overflowed the mmap (sizing bug) — \
                 file_size = MAX_NODE_ID * ENTRY_SIZE must cover every id < MAX_NODE_ID.",
                tally.dropped_oob
            );
        }

        mmap.flush()?;
        eprintln!(
            "  Node cache: {} nodes written | highest id seen {} | cap {} | {}",
            tally.written,
            tally.max_id_seen,
            MAX_NODE_ID,
            cache_path.display()
        );

        Ok(NodeCache {
            mmap,
            count: tally.written,
        })
    }

    /// Look up coordinates for a node ID. Returns [lat, lon] as f64.
    pub fn get(&self, node_id: i64) -> Option<[f64; 2]> {
        let id = node_id as u64;
        if id >= MAX_NODE_ID {
            return None;
        }

        let offset = (id * ENTRY_SIZE) as usize;
        let lat_i32 = i32::from_le_bytes(self.mmap[offset..offset + 4].try_into().ok()?);
        let lon_i32 = i32::from_le_bytes(self.mmap[offset + 4..offset + 8].try_into().ok()?);

        // Skip unwritten entries (all zeros = 0°N 0°E = middle of ocean, treat as missing)
        if lat_i32 == 0 && lon_i32 == 0 {
            return None;
        }

        Some([lat_i32 as f64 / 1e7, lon_i32 as f64 / 1e7])
    }

    pub fn count(&self) -> u64 {
        self.count
    }
}
