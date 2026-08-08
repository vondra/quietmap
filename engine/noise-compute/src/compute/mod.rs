//! Compute orchestrators that consume row views from upstream readers.
//! `aircraft_v6` reads the popup aircraft arrows directly via typed
//! column views, avoiding any `AircraftSegment` synthesis at the popup
//! boundary.

pub mod aircraft_v6;
pub(crate) mod point_sources;
pub(crate) mod railways;
pub(crate) mod roads;

/// Borrow a `HashMap`'s entries in ascending key order.
///
/// The popup is the project's acoustic reference: the same click must
/// return bit-identical numbers, or nothing downstream can be compared or
/// regression-tested. Every compute layer here ends by summing per-group
/// f64 energies across a whole accumulator map, and f64 addition is not
/// associative — so the iteration order is part of the result. The default
/// `RandomState` re-seeds on every `HashMap::new()`, i.e. on every popup
/// query rather than merely every process, which moved `total_lden` by
/// ±1 ULP between repeats of one click (measured 2026-08-05: three
/// distinct values in four runs of the same Praha click).
///
/// Sorting by key was chosen over pinning a fixed hasher. A fixed hasher
/// makes iteration order a function of the key set AND the insertion
/// history (bucket layout follows the collision/resize sequence), so it
/// holds only while nothing upstream reorders or reshards the input rows,
/// and when it breaks it breaks silently — with exactly the ±1 ULP drift
/// we are removing. Sorting makes the order a function of the key SET
/// alone: an invariant a test can pin and a reader can check locally. The
/// maps are small (one entry per road group / flight / point source in
/// receiver radius) and the sort runs once per popup.
pub(crate) fn key_sorted<K: Ord, V>(map: &std::collections::HashMap<K, V>) -> Vec<(&K, &V)> {
    let mut pairs: Vec<(&K, &V)> = map.iter().collect();
    pairs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    pairs
}

/// Owning [`key_sorted`] — consumes the map, yields entries in ascending
/// key order. Same rationale.
pub(crate) fn into_key_sorted<K: Ord, V>(map: std::collections::HashMap<K, V>) -> Vec<(K, V)> {
    let mut pairs: Vec<(K, V)> = map.into_iter().collect();
    pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    pairs
}
