//! Run independent tasks largest-first, starting each when its own allocation allowance fits the unreserved memory budget.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use anyhow::Result;

/// Task indices still waiting, ascending by allowance, plus the allowances of running tasks.
pub(crate) struct AdmissionQueue {
    waiting_ascending: Vec<usize>,
    reserved_bytes: u64,
    running_tasks: usize,
}

impl AdmissionQueue {
    pub(crate) fn new(allowances: &[u64]) -> Self {
        let mut waiting_ascending: Vec<usize> = (0..allowances.len()).collect();
        waiting_ascending.sort_by_key(|&task| (allowances[task], task));
        Self {
            waiting_ascending,
            reserved_bytes: 0,
            running_tasks: 0,
        }
    }

    /// The largest waiting task that fits beside the running ones. An idle
    /// process starts the largest task even above the concurrent budget, where it
    /// runs alone: the caller has already refused any task above the memory limit.
    pub(crate) fn admit_largest_fitting(&mut self, allowances: &[u64], budget: u64) -> Option<usize> {
        let position = if self.running_tasks == 0 {
            self.waiting_ascending.len().checked_sub(1)?
        } else {
            let unreserved = budget.saturating_sub(self.reserved_bytes);
            self.waiting_ascending
                .partition_point(|&task| allowances[task] <= unreserved)
                .checked_sub(1)?
        };
        let task = self.waiting_ascending.remove(position);
        self.reserved_bytes += allowances[task];
        self.running_tasks += 1;
        Some(task)
    }

    pub(crate) fn release(&mut self, allowance: u64) {
        self.reserved_bytes -= allowance;
        self.running_tasks -= 1;
    }
}

struct Shared {
    queue: Mutex<AdmissionQueue>,
    released: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, AdmissionQueue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Returns the allowance on every exit, including a panicking task, so waiting workers never hang.
struct Reservation<'a>(&'a Shared, u64);

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.0.lock().release(self.1);
        self.0.released.notify_all();
    }
}

/// Runs `work(task_index)` for every allowance. Returns the first error after
/// running tasks finish; tasks not yet started are then abandoned.
pub fn run_largest_first_within_memory_budget(
    allowances: &[u64],
    threads: usize,
    budget: u64,
    work: impl Fn(usize) -> Result<()> + Sync,
) -> Result<()> {
    let shared = Shared {
        queue: Mutex::new(AdmissionQueue::new(allowances)),
        released: Condvar::new(),
    };
    let first_error = Mutex::new(None);
    let worker = || loop {
        let task = {
            let mut queue = shared.lock();
            loop {
                if queue.waiting_ascending.is_empty() {
                    return;
                }
                match queue.admit_largest_fitting(allowances, budget) {
                    Some(task) => break task,
                    None => {
                        queue = shared
                            .released
                            .wait(queue)
                            .unwrap_or_else(PoisonError::into_inner)
                    }
                }
            }
        };
        let reservation = Reservation(&shared, allowances[task]);
        if let Err(error) = work(task) {
            first_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert(error);
            shared.lock().waiting_ascending.clear();
        }
        drop(reservation);
    };
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, allowances.len().max(1)) {
            scope.spawn(worker);
        }
    });
    match first_error
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
    {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[test]
    fn admits_the_largest_task_that_fits_and_lets_an_oversized_task_run_alone() {
        let allowances = [10, 70, 30, 120, 30];
        let mut queue = AdmissionQueue::new(&allowances);
        // 120 exceeds the budget of 100 and may only start on an idle process.
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), Some(3));
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), None);
        queue.release(120);
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), Some(1));
        // 30 B remain: equal allowances go out highest index first, 10 B no longer fits beside them.
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), Some(4));
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), None);
        queue.release(70);
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), Some(2));
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), Some(0));
        assert_eq!(queue.admit_largest_fitting(&allowances, 100), None);
        assert_eq!((queue.reserved_bytes, queue.running_tasks), (70, 3));
    }

    #[test]
    fn every_task_runs_once_and_concurrent_allowances_stay_within_the_budget() {
        let allowances: Vec<u64> = (0..200).map(|task| 1 + (task * 37) % 50).collect();
        let reserved = AtomicU64::new(0);
        let peak = AtomicU64::new(0);
        let runs: Vec<AtomicUsize> = allowances.iter().map(|_| AtomicUsize::new(0)).collect();
        run_largest_first_within_memory_budget(&allowances, 8, 100, |task| {
            let now = reserved.fetch_add(allowances[task], Ordering::SeqCst) + allowances[task];
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::yield_now();
            runs[task].fetch_add(1, Ordering::SeqCst);
            reserved.fetch_sub(allowances[task], Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(runs.iter().all(|count| count.load(Ordering::SeqCst) == 1));
        assert!(peak.load(Ordering::SeqCst) <= 100);
    }

    #[test]
    fn the_first_error_is_returned_and_no_further_task_starts() {
        let allowances = [1_u64; 50];
        let started = AtomicUsize::new(0);
        let error = run_largest_first_within_memory_budget(&allowances, 1, 10, |task| {
            started.fetch_add(1, Ordering::SeqCst);
            anyhow::ensure!(task != 47, "task {task} failed");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "task 47 failed");
        assert_eq!(started.load(Ordering::SeqCst), 3);
    }
}
