//! The squares on the card at once: a heavy square alone, two together, a third beside two small ones.

use std::sync::{Condvar, Mutex, PoisonError};

/// The squares handed to the painters and not yet painted, the candidates they hold together, and
/// whether a painter stopped on an error: the preparer waits for the card's room before a square
/// and for an empty card after a heavy one.
#[derive(Default)]
pub(crate) struct Card {
    state: Mutex<CardState>,
    changed: Condvar,
}

#[derive(Default)]
pub(crate) struct CardState {
    squares: usize,
    candidates: usize,
    pub(crate) stopped: bool,
}

/// One square's reservation on the card: returns its slot and candidate count on drop (including
/// on failed hand-over or unwind), optionally stopping the card under the same lock, and wakes
/// waiters.
pub(crate) struct Reservation<'a> {
    card: &'a Card,
    candidates: usize,
    stop_on_drop: bool,
}

impl Reservation<'_> {
    /// Releases the square from the card and marks the card stopped when `stop` is true, under one
    /// lock so a preparer waiting for room or an empty card never sees the free slot first.
    pub(crate) fn finish(mut self, stop: bool) {
        self.stop_on_drop = stop;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.card.change(|state| {
            state.squares -= 1;
            state.candidates -= self.candidates;
            state.stopped |= self.stop_on_drop;
        });
    }
}

/// Whether a square of `candidates` is heavy: it paints alone, and the next one is prepared only
/// once it has left the card.
pub(crate) fn paints_alone(candidates: usize) -> bool {
    candidates > PIPELINE_CANDIDATES_MAX
}

/// Whether a square of `candidates` may paint beside the `squares` on the card, which hold
/// `on_card` candidates together: a heavy square alone; two that are not heavy together; a third
/// only while the three hold no more candidates than one of two may.
fn has_room(squares: usize, on_card: usize, candidates: usize) -> bool {
    match squares {
        0 => true,
        1 => !paints_alone(on_card.max(candidates)),
        _ => squares < SQUARES_ON_THE_CARD && !paints_alone(on_card + candidates),
    }
}

impl Card {
    pub(crate) fn change(&self, change: impl FnOnce(&mut CardState)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.changed.notify_all();
    }

    pub(crate) fn stopped(&self) -> bool {
        (self.state.lock().unwrap_or_else(PoisonError::into_inner)).stopped
    }

    /// Number of squares currently reserved on the card.
    pub(crate) fn active_squares(&self) -> usize {
        (self.state.lock().unwrap_or_else(PoisonError::into_inner)).squares
    }

    /// Waits until the card has room for a square of `candidates` ([`has_room`]) and reserves it
    /// there: `None` when a painter stopped first.
    pub(crate) fn admit(&self, candidates: usize) -> Option<Reservation<'_>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut state = (self.changed.wait_while(state, |state| {
            !has_room(state.squares, state.candidates, candidates) && !state.stopped
        }))
        .unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            return None;
        }
        state.squares += 1;
        state.candidates += candidates;
        Some(Reservation {
            card: self,
            candidates,
            stop_on_drop: false,
        })
    }

    /// Waits until no square is on the card: false when a painter stopped first.
    pub(crate) fn emptied(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = (self
            .changed
            .wait_while(state, |state| state.squares > 0 && !state.stopped))
        .unwrap_or_else(PoisonError::into_inner);
        !state.stopped
    }
}

/// Squares whose pixels are painted at once, at most, in one painter process: one square's passes
/// leave the card idle between them, while sharing one process avoids duplicating the kernels'
/// card memory. A third square joins only beside two small ones ([`has_room`]).
pub(crate) const SQUARES_ON_THE_CARD: usize = 3;

/// A square's sources at most this many candidates paint beside another on the card (a heavier
/// square is admitted alone before its device scene is uploaded; three join only while they hold
/// this many together). Two squares at the cap hold 1,200,000 candidates together; on a 12 GiB
/// card, keeping the three densest squares of the 54-square world sample resident simultaneously
/// (506,185 + 356,971 + 354,432 = 1,217,588 candidates) peaked at 6,565 MiB (6.41 GiB) VRAM and
/// 20.27 GiB host RSS, leaving 4.59 GiB free on an 11 GiB card. Under the 600,000 cap, the six
/// densest squares peaked at 6,471 MiB (6.32 GiB) VRAM and 13.0 GiB host RSS (down from 7,243 MiB
/// / 7.07 GiB VRAM and 15.0 GiB RSS on the 506,185 + 356,971 pair before scratch bounding).
pub(crate) const PIPELINE_CANDIDATES_MAX: usize = 600_000;

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc::sync_channel;

    /// A heavy square paints alone, two that are not heavy together, and a third joins only while
    /// the three hold what one of two may.
    #[test]
    fn a_third_square_joins_only_two_small_ones() {
        let most = PIPELINE_CANDIDATES_MAX;
        assert!(has_room(0, 0, 3 * most));
        assert!(has_room(1, most, most));
        assert!(!has_room(1, most + 1, 1));
        assert!(!has_room(1, 1, most + 1));
        assert!(has_room(2, most / 2, most / 2));
        assert!(!has_room(2, most / 2, most / 2 + 1));
        assert!(!has_room(SQUARES_ON_THE_CARD, 2, 1));
    }

    /// A reservation returns its slot and candidates when hand-over disconnects or unwinds, and
    /// `finish(true)` stops subsequent admissions atomically.
    #[test]
    fn a_reservation_returns_on_failed_handover_and_unwind() {
        let card = Card::default();
        let counts = || {
            let state = card.state.lock().unwrap();
            (state.squares, state.candidates, state.stopped)
        };
        let (sender, receiver) = sync_channel::<Reservation<'_>>(0);
        drop(receiver);
        let reservation = card.admit(120_000).unwrap();
        assert_eq!(counts(), (1, 120_000, false));
        assert!(sender.send(reservation).is_err());
        assert_eq!(counts(), (0, 0, false));
        assert!(card.emptied());

        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _reservation = card.admit(80_000).unwrap();
            assert_eq!(counts(), (1, 80_000, false));
            panic!("simulated preparer unwind");
        }));
        assert!(unwound.is_err());
        assert_eq!(counts(), (0, 0, false));
        assert!(card.emptied());

        let reservation = card.admit(50_000).unwrap();
        reservation.finish(true);
        assert_eq!(counts(), (0, 0, true));
        assert!(card.admit(1).is_none());
    }
}
