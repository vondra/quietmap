//! Concurrent GPU passes returned in spatial order, preserving the first façade on an energy tie.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

/// Passes at once with a card.
pub(crate) const PASSES_AT_ONCE: usize = 4;

/// Of those, the most whose pairs are on the card at once, so that the others sound their pixels
/// on the cores beside them: all four let on the card together kept in step, all on the card and
/// then all on the cores, the card waiting while the cores sounded them.
pub(crate) const PASSES_ON_THE_CARD: usize = 2;

/// The card's turns among a square's passes, [`PASSES_ON_THE_CARD`] at once.
#[derive(Default)]
pub(crate) struct CardTurns {
    taken: Mutex<usize>,
    changed: Condvar,
}

impl CardTurns {
    /// `work` in a turn, once one is free.
    pub(crate) fn in_turn<T>(&self, work: impl FnOnce() -> T) -> T {
        let taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        let mut taken = (self.changed)
            .wait_while(taken, |taken| *taken == PASSES_ON_THE_CARD)
            .unwrap_or_else(PoisonError::into_inner);
        *taken += 1;
        drop(taken);
        let _given_back = TurnGivenBack(self);
        work()
    }
}

/// A turn given back as its work ends, by a panic too: the square's other passes wait for it.
struct TurnGivenBack<'a>(&'a CardTurns);

impl Drop for TurnGivenBack<'_> {
    fn drop(&mut self) {
        *(self.0.taken.lock()).unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.changed.notify_one();
    }
}

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
    use std::sync::atomic::AtomicBool;
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

    #[test]
    fn card_turns_admit_their_count_and_come_back_from_a_panic() {
        let turns = CardTurns::default();
        let failed = std::panic::catch_unwind(|| turns.in_turn(|| panic!("a pass panicked")));
        assert!(failed.is_err());
        assert_eq!(*turns.taken.lock().unwrap(), 0);
        // Every turn held at once: one more pass enters only once a holder has left.
        let (held, release) = (
            Barrier::new(PASSES_ON_THE_CARD + 1),
            Barrier::new(PASSES_ON_THE_CARD + 1),
        );
        let holding = AtomicBool::new(true);
        std::thread::scope(|scope| {
            for _ in 0..PASSES_ON_THE_CARD {
                scope.spawn(|| {
                    turns.in_turn(|| {
                        held.wait();
                        release.wait();
                    })
                });
            }
            held.wait();
            scope.spawn(|| turns.in_turn(|| assert!(!holding.load(Ordering::SeqCst))));
            std::thread::sleep(std::time::Duration::from_millis(20));
            holding.store(false, Ordering::SeqCst);
            release.wait();
        });
    }
}
