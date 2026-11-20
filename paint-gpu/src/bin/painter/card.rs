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

impl CardState {
    /// A square of `candidates` leaves the card.
    fn leave(&mut self, candidates: usize) {
        self.squares -= 1;
        self.candidates -= candidates;
    }
}

/// One square's reservation on the card: returns its slot and candidate count on drop (including
/// on failed hand-over or unwind), and wakes waiters.
pub(crate) struct Reservation<'a> {
    card: &'a Card,
    candidates: usize,
    released: bool,
}

impl Reservation<'_> {
    /// Releases the square from the card and marks the card stopped when `stop` is true, under one
    /// lock so a preparer waiting for room or an empty card never sees the free slot first.
    pub(crate) fn finish(mut self, stop: bool) {
        self.released = true;
        self.card.change(|state| {
            state.leave(self.candidates);
            state.stopped |= stop;
        });
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.released {
            self.card.change(|state| state.leave(self.candidates));
        }
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
            released: false,
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

/// Squares whose pixels are painted at once, at most: one square's passes leave the card idle
/// between them (two painter processes on one card: light squares 1.4x, 30-300 KB 1.17x, Dobris
/// 1.12x, 2026-10-14); two processes also kept two copies of the kernels' card memory. A third
/// beside two small ones ([`has_room`]) took 2-5 % off squares of up to 300 KB on two kinds of
/// card, each square's own pixels over a quarter slower beside two (2026-11-19).
pub(crate) const SQUARES_ON_THE_CARD: usize = 3;

/// A square's sources at most this many candidates paint while the next one is prepared (Dobris's
/// about 130,000 do; Prague's centre, 470,000 and 30-45 GB, paints alone).
const PIPELINE_CANDIDATES_MAX: usize = 250_000;

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
