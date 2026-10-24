//! Concurrent GPU passes returned in spatial order, preserving the first façade on an energy tie.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Passes at once with a card.
pub(crate) const PASSES_AT_ONCE: usize = 4;

pub(crate) fn paint_passes<T: Send>(
    runs: &[Range<usize>],
    pass: &(impl Fn(Range<usize>) -> Result<T, String> + Sync),
) -> Result<Vec<(Range<usize>, T)>, String> {
    let next = AtomicUsize::new(0);
    let worker = || {
        let mut completed = Vec::new();
        while let Some(run) = runs.get(next.fetch_add(1, Ordering::Relaxed)) {
            completed.push(pass(run.clone()).map(|painted| (run.clone(), painted)));
        }
        completed
    };
    let mut completed = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..PASSES_AT_ONCE).map(|_| scope.spawn(worker)).collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| vec![Err("a pass panicked".into())])
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    // Façade ties keep the first candidate, even when its pass finishes after a later one.
    completed.sort_unstable_by_key(|(run, _)| run.start);
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Condvar, Mutex};

    #[test]
    fn delayed_passes_keep_spatial_order() {
        let first_passes = Barrier::new(PASSES_AT_ONCE);
        let following_passes_done = (Mutex::new(false), Condvar::new());
        let runs: Vec<_> = (0..12).map(|at| at..at + 1).collect();
        let painted = paint_passes(&runs, &|run| {
            if run.start < PASSES_AT_ONCE {
                first_passes.wait();
                if run.start > 0 {
                    let (done, changed) = &following_passes_done;
                    drop(
                        changed
                            .wait_while(done.lock().unwrap(), |done| !*done)
                            .unwrap(),
                    );
                }
            }
            if run.start == 11 {
                let (done, changed) = &following_passes_done;
                *done.lock().unwrap() = true;
                changed.notify_all();
            }
            Ok(run.start)
        })
        .unwrap();
        assert_eq!(
            painted,
            runs.into_iter()
                .map(|run| (run.clone(), run.start))
                .collect::<Vec<_>>()
        );
    }
}
