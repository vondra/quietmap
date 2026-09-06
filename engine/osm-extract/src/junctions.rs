//! Preserve repeated OSM linear-way node identities through geometric simplification.

use anyhow::Result;
use memmap2::{MmapMut, MmapRaw};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::node_cache::MAX_NODE_ID;

pub struct NodeIdBitmap(MmapRaw);

impl NodeIdBitmap {
    fn new() -> Result<Self> {
        // Anonymous zero pages allocate physical memory only for touched ID ranges.
        Ok(Self(MmapRaw::from(MmapMut::map_anon(
            MAX_NODE_ID.div_ceil(64) as usize * size_of::<u64>(),
        )?)))
    }

    fn word(&self, node_id: i64) -> Option<(&AtomicU64, u64)> {
        let id = u64::try_from(node_id).ok().filter(|id| *id < MAX_NODE_ID)?;
        let offset = (id / 64) as usize * size_of::<u64>();
        // SAFETY: mmap is writable and page-aligned, offset is in bounds and
        // u64-aligned. MmapRaw never creates shared byte slices; every memory
        // access is atomic. The map outlives all borrowed atomics and workers.
        let word = unsafe { AtomicU64::from_ptr(self.0.as_mut_ptr().add(offset).cast::<u64>()) };
        Some((word, 1 << (id % 64)))
    }

    fn insert(&self, node_id: i64) -> bool {
        self.word(node_id)
            .is_some_and(|(word, mask)| word.fetch_or(mask, Ordering::Relaxed) & mask == 0)
    }

    pub fn contains(&self, node_id: i64) -> bool {
        self.word(node_id)
            .is_some_and(|(word, mask)| word.load(Ordering::Relaxed) & mask != 0)
    }
}

pub struct JunctionCensus {
    seen: NodeIdBitmap,
    repeated: NodeIdBitmap,
    count: AtomicU64,
}

impl JunctionCensus {
    pub fn new() -> Result<Self> {
        Ok(Self {
            seen: NodeIdBitmap::new()?,
            repeated: NodeIdBitmap::new()?,
            count: AtomicU64::new(0),
        })
    }

    pub fn record(&self, node_id: i64) {
        if !self.seen.insert(node_id) && self.repeated.insert(node_id) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn finish(self) -> NodeIdBitmap {
        self.repeated
    }
}

#[cfg(test)]
mod tests {
    use super::{JunctionCensus, MAX_NODE_ID};
    use rayon::prelude::*;

    #[test]
    fn parallel_reference_census_preserves_only_repeated_exact_ids() {
        let census = JunctionCensus::new().unwrap();
        let ways = [vec![1, 63, 64], vec![2, 63, 3], vec![4, 65, 66, 65, 5]];
        ways.par_iter().for_each(|ids| {
            for &id in ids {
                census.record(id);
            }
        });
        // Exercise concurrent repeats of the same bit and the adjacent word.
        (0..1024).into_par_iter().for_each(|_| census.record(64));
        census.record(-1);
        census.record(MAX_NODE_ID as i64);
        assert_eq!(census.count(), 3);
        let protected = census.finish();
        for id in [63, 64, 65] {
            assert!(protected.contains(id));
        }
        for id in [-1, 0, 1, 2, 3, 4, 5, 66, MAX_NODE_ID as i64] {
            assert!(!protected.contains(id));
        }
    }
}
