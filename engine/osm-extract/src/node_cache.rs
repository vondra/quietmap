//! Memory-mapped node coordinate cache.
//!
//! Stores lat/lon as i32 microdegrees (×1e7) in a sparse file indexed by node ID.
//! File layout: node_id × 8 bytes → [lat_i32, lon_i32].
//! Sparse file on NVMe — OS only allocates pages that are written.
//! Presence is a side bitmap so a real 0°N 0°E node is not treated as missing.

use anyhow::Result;
use memmap2::{Mmap, MmapMut};
use osmpbf::{Element, ElementReader};
use std::fs::OpenOptions;
use std::path::Path;

use crate::junctions::NodeIdBitmap;

/// Upper bound on OSM node IDs — the global id counter, ~13.7 B in our
/// Apr-2026 CZ PBF and growing ~2 B/yr. Sized with years of headroom because
/// the cache file is SPARSE: ids above the real max cost virtual address
/// space only, never resident pages. A source node at or above this bound
/// fails extraction before publication; the junction census uses the same cap.
pub(crate) const MAX_NODE_ID: u64 = 25_000_000_000;
const ENTRY_SIZE: u64 = 8; // 4 bytes lat + 4 bytes lon

pub struct NodeCache {
    mmap: Mmap,
    present: NodeIdBitmap,
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
    skipped_unneeded: u64,
    dropped_over_cap: u64,
    dropped_oob: u64,
    max_id_seen: u64,
}

impl Tally {
    fn merge(self, other: Tally) -> Tally {
        Tally {
            written: self.written + other.written,
            skipped_unneeded: self.skipped_unneeded + other.skipped_unneeded,
            dropped_over_cap: self.dropped_over_cap + other.dropped_over_cap,
            dropped_oob: self.dropped_oob + other.dropped_oob,
            max_id_seen: self.max_id_seen.max(other.max_id_seen),
        }
    }
}

impl NodeCache {
    /// Build the node cache by streaming nodes from the PBF.
    ///
    /// Only needed node IDs are written — Pass 0's selected
    /// way + relation-member census. Uses `par_map_reduce`: each rayon worker
    /// decodes PBF blocks independently and writes to the shared sparse mmap.
    /// Writes are per-node-id (8 bytes at `node_id * 8` offset), so workers
    /// never touch the same byte range.
    pub fn build(pbf_path: &Path, cache_path: &Path, needed: &NodeIdBitmap) -> Result<Self> {
        let file_size = MAX_NODE_ID * ENTRY_SIZE;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(cache_path)?;
        file.set_len(file_size)?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        let present = NodeIdBitmap::new()?;

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
                if id >= MAX_NODE_ID {
                    t.dropped_over_cap = 1;
                    return t;
                }
                if !needed.contains(id as i64) {
                    t.skipped_unneeded = 1;
                    return t;
                }
                match writer.write(id, lat, lon) {
                    WriteOutcome::Written => {
                        present.insert(id as i64);
                        t.written = 1;
                    }
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
        let mmap = mmap.make_read_only()?;
        eprintln!(
            "  Node cache: {} nodes written | {} not needed for selected layers | highest id seen {} | cap {} | {}",
            tally.written,
            tally.skipped_unneeded,
            tally.max_id_seen,
            MAX_NODE_ID,
            cache_path.display()
        );

        Ok(NodeCache {
            mmap,
            present,
            count: tally.written,
        })
    }

    /// Look up coordinates for a node ID. Returns [lat, lon] as f64.
    /// Absent IDs (never written, including referenced-but-missing source nodes)
    /// return None. A present 0°N 0°E node is returned as `[0.0, 0.0]`.
    pub fn get(&self, node_id: i64) -> Option<[f64; 2]> {
        if !self.present.contains(node_id) {
            return None;
        }

        let id = node_id as u64;
        let offset = (id * ENTRY_SIZE) as usize;
        let lat_i32 = i32::from_le_bytes(self.mmap[offset..offset + 4].try_into().ok()?);
        let lon_i32 = i32::from_le_bytes(self.mmap[offset + 4..offset + 8].try_into().ok()?);
        Some([lat_i32 as f64 / 1e7, lon_i32 as f64 / 1e7])
    }

    pub fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::{MmapWriter, NodeCache, WriteOutcome, ENTRY_SIZE, MAX_NODE_ID};
    use crate::junctions::NodeIdBitmap;
    use std::fs::OpenOptions;

    fn cache_from_entries(path: &std::path::Path, entries: &[(u64, f64, f64)]) -> NodeCache {
        let file_size = MAX_NODE_ID * ENTRY_SIZE;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        file.set_len(file_size).unwrap();
        let mut mmap = unsafe { memmap2::MmapMut::map_mut(&file).unwrap() };
        let present = NodeIdBitmap::new().unwrap();
        let writer = MmapWriter {
            ptr: mmap.as_mut_ptr(),
            len: mmap.len(),
        };
        for &(id, lat, lon) in entries {
            assert!(matches!(writer.write(id, lat, lon), WriteOutcome::Written));
            present.insert(id as i64);
        }
        mmap.flush().unwrap();
        NodeCache {
            mmap: mmap.make_read_only().unwrap(),
            present,
            count: entries.len() as u64,
        }
    }

    #[test]
    fn present_zero_zero_node_is_not_treated_as_missing() {
        let directory =
            std::env::temp_dir().join(format!("osm-cache-nullisland-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("nodes.cache");
        let cache = cache_from_entries(&path, &[(1, 0.0, 0.0), (2, 50.0, 14.0)]);
        assert_eq!(cache.get(1), Some([0.0, 0.0]));
        assert_eq!(cache.get(2), Some([50.0, 14.0]));
        assert_eq!(cache.get(3), None);
        assert_eq!(cache.count(), 2);
        drop(cache);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
