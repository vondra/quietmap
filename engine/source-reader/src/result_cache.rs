//! Recent popup results by exact point, so one compute serves the click, its Segments tab and repeat clicks.

use std::collections::VecDeque;
use std::sync::Mutex;

/// (lat bits, lng bits): exact coordinates, since the follow-up requests of
/// one click repeat them verbatim.
pub type PointKey = (u64, u64);

/// Most recently used last; the cap evicts the entry used longest ago.
pub struct ResultCache<T> {
    entries: Mutex<VecDeque<(PointKey, T)>>,
    cap: usize,
}

impl<T> ResultCache<T> {
    pub const fn new(cap: usize) -> Self {
        ResultCache {
            entries: Mutex::new(VecDeque::new()),
            cap,
        }
    }

    /// Applies `view` to the cached entry under the cache lock and keeps the
    /// entry young. A view that serializes a few MB holds the lock for tens
    /// of milliseconds; a concurrent `put` waits that long.
    pub fn get_with<R>(&self, key: PointKey, view: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let idx = entries.iter().position(|(k, _)| *k == key)?;
        let mut entry = entries.remove(idx)?;
        let out = view(&mut entry.1);
        entries.push_back(entry);
        Some(out)
    }

    pub fn put(&self, key: PointKey, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|(k, _)| *k != key);
        while entries.len() >= self.cap {
            entries.pop_front();
        }
        entries.push_back((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::ResultCache;

    /// A hit hands the entry to the view and keeps it young; the cap evicts
    /// the entry used longest ago.
    #[test]
    fn serves_the_view_and_evicts_least_recently_used() {
        let cache = ResultCache::new(3);
        for i in 0..3u64 {
            cache.put((i, 0), format!("r{i}"));
        }
        assert_eq!(
            cache.get_with((0, 0), |r| r.clone()),
            Some("r0".to_string())
        );
        cache.put((3, 0), "r3".into());
        assert!(
            cache.get_with((0, 0), |_| ()).is_some(),
            "just used, must survive"
        );
        assert!(
            cache.get_with((1, 0), |_| ()).is_none(),
            "the oldest untouched entry goes"
        );
    }
}
