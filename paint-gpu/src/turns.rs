//! The card's turns among a square's evaluations: at most two launch their pairs at once.

use std::sync::{Condvar, Mutex, PoisonError};

/// A square's evaluations launching on the card at once, so that its other passes list and sound
/// their pixels on the cores beside them: its four passes all let on the card kept in step, all on
/// the card and then all on the cores, the card waiting while the cores sounded them.
pub(crate) const EVALUATIONS_ON_THE_CARD: usize = 2;

/// The card's turns among a square's evaluations, [`EVALUATIONS_ON_THE_CARD`] at once.
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
            .wait_while(taken, |taken| *taken == EVALUATIONS_ON_THE_CARD)
            .unwrap_or_else(PoisonError::into_inner);
        *taken += 1;
        drop(taken);
        let _given_back = TurnGivenBack(self);
        work()
    }
}

/// A turn given back as its work ends, by a panic too: the square's other evaluations wait for it.
struct TurnGivenBack<'a>(&'a CardTurns);

impl Drop for TurnGivenBack<'_> {
    fn drop(&mut self) {
        *(self.0.taken.lock()).unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn card_turns_admit_their_count_and_come_back_from_a_panic() {
        let turns = CardTurns::default();
        let failed =
            std::panic::catch_unwind(|| turns.in_turn(|| panic!("an evaluation panicked")));
        assert!(failed.is_err());
        assert_eq!(*turns.taken.lock().unwrap(), 0);
        // Every turn held at once: one more evaluation enters only once a holder has left.
        let (held, release) = (
            Barrier::new(EVALUATIONS_ON_THE_CARD + 1),
            Barrier::new(EVALUATIONS_ON_THE_CARD + 1),
        );
        let holding = AtomicBool::new(true);
        std::thread::scope(|scope| {
            for _ in 0..EVALUATIONS_ON_THE_CARD {
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
