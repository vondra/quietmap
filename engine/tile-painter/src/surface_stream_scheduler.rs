//! Fair one-region claims for the CPU surface stream.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// One queue lock acquisition may transfer at most one region to a worker.
pub const SURFACE_STREAM_MAX_REGIONS_PER_CLAIM: usize = 1;

struct QueueState<T> {
    pending: VecDeque<T>,
    closed: bool,
}

/// Blocking FIFO whose only claim operation transfers one region.
pub struct SurfaceStreamQueue<T> {
    state: Mutex<QueueState<T>>,
    available: Condvar,
}

impl<T> Default for SurfaceStreamQueue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(QueueState {
                pending: VecDeque::new(),
                closed: false,
            }),
            available: Condvar::new(),
        }
    }
}

impl<T> SurfaceStreamQueue<T> {
    /// Append one region unless EOF has already closed the queue.
    pub fn push(&self, region: T) -> Result<(), T> {
        let mut state = self.state.lock().expect("surface stream queue poisoned");
        if state.closed {
            return Err(region);
        }
        state.pending.push_back(region);
        self.available.notify_one();
        Ok(())
    }

    /// Claim exactly one FIFO region, or return `None` after EOF drains the queue.
    pub fn claim_one(&self) -> Option<T> {
        let mut state = self.state.lock().expect("surface stream queue poisoned");
        loop {
            if let Some(region) = state.pending.pop_front() {
                return Some(region);
            }
            if state.closed {
                return None;
            }
            state = self
                .available
                .wait(state)
                .expect("surface stream queue poisoned while waiting");
        }
    }

    /// Mark input EOF. Already queued regions remain claimable exactly once.
    pub fn close(&self) {
        let mut state = self.state.lock().expect("surface stream queue poisoned");
        state.closed = true;
        self.available.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{mpsc, Arc, Barrier};

    const PARITY_CELLS: [u64; 2] = [0x841e309ffffffff, 0x841e355ffffffff];

    #[test]
    fn first_two_of_four_regions_are_owned_by_distinct_blocked_workers() {
        let queue = Arc::new(SurfaceStreamQueue::default());
        for cell in 1..=4 {
            queue.push(cell).unwrap();
        }
        queue.close();

        let first_claims_ready = Arc::new(Barrier::new(3));
        let release_first_claims = Arc::new(Barrier::new(3));
        let (claims_tx, claims_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            for slot in 0..2 {
                let queue = Arc::clone(&queue);
                let first_claims_ready = Arc::clone(&first_claims_ready);
                let release_first_claims = Arc::clone(&release_first_claims);
                let claims_tx = claims_tx.clone();
                scope.spawn(move || {
                    let first = queue.claim_one().unwrap();
                    claims_tx.send((slot, first)).unwrap();
                    first_claims_ready.wait();
                    release_first_claims.wait();
                    while let Some(cell) = queue.claim_one() {
                        claims_tx.send((slot, cell)).unwrap();
                    }
                });
            }
            drop(claims_tx);
            first_claims_ready.wait();
            let mut first = [claims_rx.recv().unwrap(), claims_rx.recv().unwrap()];
            first.sort_unstable();
            assert_eq!(first[0].0, 0);
            assert_eq!(first[1].0, 1);
            assert_ne!(first[0].1, first[1].1);
            assert_eq!(
                first.iter().map(|&(_, cell)| cell).collect::<BTreeSet<_>>(),
                BTreeSet::from([1, 2])
            );
            release_first_claims.wait();
        });

        let mut claimed = vec![1, 2];
        claimed.extend(claims_rx.into_iter().map(|(_, cell)| cell));
        claimed.sort_unstable();
        assert_eq!(claimed, vec![1, 2, 3, 4]);
    }

    fn process_permutation(input: [u8; 4], workers: usize) -> Vec<(u8, Result<u8, &'static str>)> {
        let queue = Arc::new(SurfaceStreamQueue::default());
        for item in input {
            queue.push(item).unwrap();
        }
        queue.close();
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                scope.spawn(move || {
                    while let Some(item) = queue.claim_one() {
                        let outcome = if item == 2 {
                            Err("injected region failure")
                        } else {
                            Ok(item.wrapping_mul(17))
                        };
                        outcomes.lock().unwrap().push((item, outcome));
                    }
                });
            }
        });
        let mut outcomes = Arc::try_unwrap(outcomes).unwrap().into_inner().unwrap();
        outcomes.sort_by_key(|&(item, _)| item);
        outcomes
    }

    #[test]
    fn permutations_eof_and_one_failure_remain_exactly_once() {
        let permutations = [[1, 2, 3, 4], [4, 3, 2, 1], [2, 4, 1, 3], [3, 1, 4, 2]];
        for permutation in permutations {
            let outcomes = process_permutation(permutation, 3);
            assert_eq!(outcomes.len(), 4);
            assert_eq!(
                outcomes
                    .iter()
                    .map(|&(item, _)| item)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from([1, 2, 3, 4])
            );
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|(_, result)| result.is_err())
                    .count(),
                1
            );
            assert_eq!(outcomes[1], (2, Err("injected region failure")));
        }

        let empty = SurfaceStreamQueue::<u8>::default();
        empty.close();
        assert_eq!(empty.claim_one(), None);
        assert_eq!(empty.push(5), Err(5));
    }

    fn canonical_output_bytes(workers: usize) -> BTreeMap<u64, Vec<u8>> {
        let queue = Arc::new(SurfaceStreamQueue::default());
        for cell in PARITY_CELLS {
            queue.push(cell).unwrap();
        }
        queue.close();
        let outputs = Arc::new(Mutex::new(BTreeMap::new()));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let queue = Arc::clone(&queue);
                let outputs = Arc::clone(&outputs);
                scope.spawn(move || {
                    while let Some(cell) = queue.claim_one() {
                        outputs
                            .lock()
                            .unwrap()
                            .insert(cell, cell.to_be_bytes().repeat(4));
                    }
                });
            }
        });
        Arc::try_unwrap(outputs).unwrap().into_inner().unwrap()
    }

    #[test]
    fn scheduler_preserves_mock_payloads_for_canonical_cell_ids() {
        assert_eq!(canonical_output_bytes(1), canonical_output_bytes(2));
    }

    #[test]
    fn claim_limit_is_the_reviewed_one_region_constant() {
        assert_eq!(SURFACE_STREAM_MAX_REGIONS_PER_CLAIM, 1);
    }
}
