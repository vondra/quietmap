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
    pub(crate) fn leave(&mut self, candidates: usize) {
        self.squares -= 1;
        self.candidates -= candidates;
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

    /// Waits until the card has room for a square of `candidates` ([`has_room`]) and counts it
    /// there: false when a painter stopped first.
    pub(crate) fn admitted(&self, candidates: usize) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut state = (self.changed.wait_while(state, |state| {
            !has_room(state.squares, state.candidates, candidates) && !state.stopped
        }))
        .unwrap_or_else(PoisonError::into_inner);
        if !state.stopped {
            state.squares += 1;
            state.candidates += candidates;
        }
        !state.stopped
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
}
