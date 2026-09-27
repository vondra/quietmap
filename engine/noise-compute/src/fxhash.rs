//! Fixed-seed FxHash maps for popup grouping accumulators.
//!
//! The popup groups millions of rows (flights, road groups, cells) through
//! `HashMap`s whose only order-sensitive consumers sort first (`key_sorted`)
//! or commute per key (the airborne chunk merge). `RandomState` re-seeds per
//! map, so it both pays SipHash per lookup (~30 ns) and perturbs nothing —
//! the sorts normalize it away. FxHash (multiply-xor, fixed seed) hashes a
//! `u64` key in ~2 ns with bit-identical outputs, proven by the existing
//! bit-identity tests (same-process fresh maps already carry distinct
//! `RandomState` seeds, so any hash-order leak would fail them today).
//!
//! Deliberately dependency-free (the FxHash algorithm is public domain):
//! [`FxHashMap`] / [`FxHashSet`] are plain std maps with [`FxBuildHasher`].
//! Boundaries that tests also feed stay generic over the hasher (`S:
//! BuildHasher`) so fixtures keep `RandomState` and its leak detection.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

/// FxHash seed: the 64-bit golden-ratio-ish constant from rustc-hash.
const FX_SEED: usize = 0x51_7c_c1_b7_27_22_0a_95;

/// FxHash hasher: rotate-5 + multiply per word. Deterministic across
/// processes (fixed seed) — popup bytes never depend on it, because every
/// order-sensitive consumer sorts first (see `crate::compute::key_sorted`).
#[derive(Debug, Clone)]
pub struct FxHasher {
    hash: usize,
}

impl FxHasher {
    /// rustc-hash FxHash step, verbatim: rotate 5, xor the word, multiply.
    #[inline]
    fn add_to_hash(&mut self, word: usize) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(FX_SEED);
    }
}

impl Default for FxHasher {
    #[inline]
    fn default() -> Self {
        Self { hash: 0 }
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for word in bytes.chunks_exact(std::mem::size_of::<usize>()) {
            self.add_to_hash(usize::from_ne_bytes(word.try_into().unwrap()));
        }
        let rem = bytes.len() % std::mem::size_of::<usize>();
        if rem > 0 {
            let mut buf = [0u8; std::mem::size_of::<usize>()];
            buf[..rem].copy_from_slice(&bytes[bytes.len() - rem..]);
            self.add_to_hash(usize::from_ne_bytes(buf));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        #[cfg(target_pointer_width = "64")]
        self.add_to_hash(i as usize);
        #[cfg(target_pointer_width = "32")]
        {
            self.add_to_hash(i as usize);
            self.add_to_hash((i >> 32) as usize);
        }
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i);
    }

    #[inline]
    fn write_i8(&mut self, i: i8) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_i16(&mut self, i: i16) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_isize(&mut self, i: isize) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash as u64
    }
}

/// [`BuildHasher`] for [`FxHasher`]: every map gets the same fixed seed.
#[derive(Debug, Clone, Copy, Default)]
pub struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> FxHasher {
        FxHasher::default()
    }
}

/// `HashMap` with the fixed-seed FxHash: fast lookups, deterministic layout.
/// Outputs never depend on the layout (consumers sort); see module docs.
pub type FxHashMap<K, V> = HashMap<K, V, FxBuildHasher>;

/// `HashSet` with the fixed-seed FxHash. Same contract as [`FxHashMap`].
pub type FxHashSet<T> = HashSet<T, FxBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fx_maps_agree_with_std_maps() {
        let mut fx: FxHashMap<u64, u64> = FxHashMap::default();
        let mut std: HashMap<u64, u64> = HashMap::new();
        for i in 0..1000u64 {
            let k = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            fx.insert(k, i);
            std.insert(k, i);
        }
        // No cross-hasher `==` in std: compare sorted contents.
        let mut fx_pairs: Vec<_> = fx.iter().collect();
        let mut std_pairs: Vec<_> = std.iter().collect();
        fx_pairs.sort();
        std_pairs.sort();
        assert_eq!(fx_pairs, std_pairs);
    }

    #[test]
    fn fx_layout_is_deterministic_across_maps() {
        let build = || {
            let mut m: FxHashMap<u64, u64> = FxHashMap::default();
            for i in 0..500u64 {
                m.insert(i * 31, i);
            }
            m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }
}
