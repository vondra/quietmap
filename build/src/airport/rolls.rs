//! Take-off and landing rolls as Doc 29 flies them (`physics::doc29::roll`), one per runway
//! direction, operation and noise class ([`super::runways`]). A flight rolls where ADS-B saw it
//! rolling on a runway (a take-off leg, or a leg of 40 kt or more), else where it was found or lost
//! low along one ([`super::low_ends`]): ADS-B sees an aircraft on the ground only near a receiver,
//! and at Sao Paulo Congonhas about 40 % of the flights seen low over the airport had no ground
//! leg, where Prague misses almost none. A low end lies along a runway when a climb found low
//! starts beyond the runway's end, or a descent lost low ends before or over it.

use super::legs::GroundLeg;
use super::low_ends::LowEnd;
use super::runways::{RollKey, Runways};
use std::collections::{HashMap, HashSet};

/// A leg rolling along a runway: its runway, direction, class, speed, period and Stage 1's take-off
/// flag.
#[derive(Clone, Copy)]
struct Rolling {
    runway: u32,
    forward: bool,
    class: u8,
    speed_kt: f64,
    period: u8,
    departure: bool,
}

/// Rolling legs of one operation: the fastest, whether the primary saw any, and whether any is
/// flagged a take-off and any not.
#[derive(Clone, Copy, Default)]
struct Legs {
    fastest: Option<Rolling>,
    primary: bool,
    flagged: bool,
    unflagged: bool,
}

impl Legs {
    fn with(self, other: Legs) -> Legs {
        let fastest = match (self.fastest, other.fastest) {
            (Some(a), Some(b)) => Some(if b.speed_kt > a.speed_kt { b } else { a }),
            (a, b) => a.or(b),
        };
        Legs {
            fastest,
            primary: self.primary || other.primary,
            flagged: self.flagged || other.flagged,
            unflagged: self.unflagged || other.unflagged,
        }
    }

    /// The operation's roll and whether the primary saw it, where `seen` says its legs show it.
    fn roll(self, landing: bool, seen: bool) -> Option<(bool, Rolling, bool)> {
        let fastest = self.fastest.filter(|_| seen)?;
        Some((landing, fastest, self.primary))
    }
}

/// A flight's stretch on the ground rolling along runways, its legs in the order flown: up to and
/// with the slowest (a landing's, before a stop or a touch-and-go turns it round), and after it.
#[derive(Default)]
pub struct Stretch {
    before: Legs,
    after: Legs,
    slowest_kt: Option<f64>,
}

impl Stretch {
    pub fn add(&mut self, leg: &GroundLeg, runway: u32, forward: bool, class: u8) {
        let one = Legs {
            fastest: Some(Rolling {
                runway,
                forward,
                class,
                speed_kt: leg.speed_kt,
                period: leg.period,
                departure: leg.departure,
            }),
            primary: !leg.secondary_only,
            flagged: leg.departure,
            unflagged: !leg.departure,
        };
        if self.slowest_kt.is_none_or(|slowest| leg.speed_kt < slowest) {
            self.before = self.before.with(self.after).with(one);
            self.after = Legs::default();
            self.slowest_kt = Some(leg.speed_kt);
        } else {
            self.after = self.after.with(one);
        }
    }
}

/// One flight's roll of a day.
#[derive(Clone, Debug, PartialEq)]
pub struct RollMovement {
    pub key: RollKey,
    pub flight_id: u64,
    /// The flight's stretch on the ground it rolls in.
    pub episode: u16,
    pub period: u8,
    pub secondary_only: bool,
}

impl RollMovement {
    /// The movement it is: the flight, its stretch on the ground, a landing or a take-off.
    pub fn identity(&self) -> (u64, u16, bool) {
        (self.flight_id, self.episode, self.key.landing)
    }
}

/// The movements seen rolling. A stretch on the ground lands when the flight was airborne before
/// it and takes off when an airborne segment follows it; in neither, its fastest leg's take-off
/// flag says (Stage 1 flags a leg accelerating, a landing speeding up to vacate too: it counted 99
/// of Prague's 307 take-offs of 2026-06-15 as a second movement). A stretch's one operation owns
/// all its rolling legs. A stretch that lands and takes off (a stop shorter than Stage 1's
/// five-minute rest stays one flight) gives the landing its legs up to the slowest, if one of them
/// slows (a vacate accelerates after it), and the take-off those after, if one accelerates; an
/// operation is placed by its fastest leg and is the primary's if the primary saw any of its legs;
/// one not seen so was not seen rolling, and its low end places it.
pub fn observed(
    stretches: HashMap<(u64, u16), Stretch>,
    lift_offs: &HashSet<(u64, u16)>,
) -> Vec<RollMovement> {
    let mut movements = Vec::new();
    for ((flight_id, episode), stretch) in stretches {
        let all = stretch.before.with(stretch.after);
        let operations = match (episode > 0, lift_offs.contains(&(flight_id, episode))) {
            (true, true) => [
                stretch.before.roll(true, stretch.before.unflagged),
                stretch.after.roll(false, stretch.after.flagged),
            ],
            (true, false) => [all.roll(true, true), None],
            (false, true) => [all.roll(false, true), None],
            // Never seen airborne: the fastest leg's flag says which.
            (false, false) => {
                let lands = !all.fastest.is_some_and(|fastest| fastest.departure);
                [all.roll(lands, true), None]
            }
        };
        for (landing, roll, primary) in operations.into_iter().flatten() {
            movements.push(RollMovement {
                key: RollKey {
                    runway: roll.runway,
                    forward: roll.forward,
                    landing,
                    class: roll.class,
                },
                flight_id,
                episode,
                period: roll.period,
                secondary_only: !primary,
            });
        }
    }
    movements
}

/// The day's movements, one count each (review item 8, by operation): the rolls seen on a runway,
/// and for every movement not seen rolling, a low end placing it along a runway. A movement the
/// primary provider witnessed (its own legs, or its low end placing the roll) counts as the
/// primary's on every day it is seen, and what only the secondary saw of it goes, or an increment
/// day counts it twice; what only the secondary saw of a movement stays, at the increment's weight:
/// a landing the primary missed, the other operation of a turnaround, a circuit's later
/// touch-and-go.
pub fn movements(
    observed: Vec<RollMovement>,
    low_ends: &[LowEnd],
    runways: &Runways,
) -> Vec<RollMovement> {
    let inferred: Vec<RollMovement> = (low_ends.iter())
        .filter_map(|end| inferred(end, runways))
        .collect();
    let witnessed: HashSet<(u64, u16, bool)> = (observed.iter().chain(&inferred))
        .filter(|movement| !movement.secondary_only)
        .map(RollMovement::identity)
        .collect();
    let kept = |movement: &RollMovement| {
        !(movement.secondary_only && witnessed.contains(&movement.identity()))
    };
    let mut counted: Vec<RollMovement> = observed.into_iter().filter(kept).collect();
    let mut taken: HashSet<(u64, u16, bool)> = counted.iter().map(RollMovement::identity).collect();
    for movement in inferred {
        if kept(&movement) && taken.insert(movement.identity()) {
            counted.push(movement);
        }
    }
    counted
}

/// The roll a low end places along a runway: a take-off where a view found the flight climbing, a
/// landing where one lost it otherwise (a departure lost while still low places none).
fn inferred(end: &LowEnd, runways: &Runways) -> Option<RollMovement> {
    let take_off = end.first && end.departure;
    let landing = !end.first && !end.departure;
    if !(take_off || landing) {
        return None;
    }
    let (runway, forward) = runways.fitting(end, take_off)?;
    Some(RollMovement {
        key: RollKey {
            runway,
            forward,
            landing,
            class: end.class,
        },
        flight_id: end.flight_id,
        episode: end.episode,
        period: end.period,
        secondary_only: end.secondary_only,
    })
}
