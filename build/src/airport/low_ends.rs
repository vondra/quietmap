//! A flight's low ends, read segment by segment in Stage 1's order: where ADS-B found or lost it
//! low, as the traffic pass places a roll the ground receivers missed, and the stretch on the
//! ground each leaves or reaches. A view of a flight ends at its first and last airborne segments
//! and on both sides of every stretch on the ground between two of them: one seen, or a turnaround
//! unseen, a break (the next segment starting elsewhere) between a descent and a climb. Stage 1
//! splits a flight only at a rest it saw on the ground, so where no receiver sees the ground an
//! aircraft's arrival and its next departure stay one flight, and their rolls lie in its middle.
//! Two views: every provider's segments, whose ends only the secondary saw count on the increment
//! days; and the primary provider's own, whose ends witness its movements there as it sees them
//! alone on the others.

use crate::aircraft::MIN_SEGMENT_LENGTH_M;
use crate::aircraft::flat::flat_distance_m;
use std::collections::HashSet;

/// A flight's first or last airborne segment is a low end below this height above the ground (m).
pub const LOW_END_M: f32 = 150.0;

/// Where a flight was found or lost low: where it left or reached the ground, seen from the air
/// (a roll the ground receivers may have missed).
#[derive(Debug, Clone, PartialEq)]
pub struct LowEnd {
    pub flight_id: u64,
    /// The flight's stretch on the ground it left or reached ([`super::legs::GroundLeg::episode`]).
    pub episode: u16,
    pub class: u8,
    pub period: u8,
    /// Where a view of the flight begins (else where one ends).
    pub first: bool,
    /// Stage 1 took the segment for a departure (climbing).
    pub departure: bool,
    pub secondary_only: bool,
    /// Latitude and longitude of both ends (deg).
    pub start: [f64; 2],
    pub end: [f64; 2],
}

impl LowEnd {
    /// The end of the segment nearer its runway: a take-off's start, a landing's end.
    pub fn runway_side(&self) -> [f64; 2] {
        if self.first { self.start } else { self.end }
    }
}

/// One airborne segment as the low ends read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Airborne {
    /// Latitude and longitude of both ends (deg), as Stage 1 wrote them: consecutive segments
    /// share a sample, so a break is a start other than the last end.
    pub start: [f32; 2],
    pub end: [f32; 2],
    /// Mean height above the ground (m).
    pub height_m: f32,
    /// Stage 1 took it for a departure (climbing).
    pub departure: bool,
    pub secondary_only: bool,
    pub class: u8,
    pub period: u8,
}

impl Airborne {
    fn low_end(&self, flight_id: u64, episode: u16, first: bool) -> Option<LowEnd> {
        (self.height_m < LOW_END_M).then(|| LowEnd {
            flight_id,
            episode,
            class: self.class,
            period: self.period,
            first,
            departure: self.departure,
            secondary_only: self.secondary_only,
            start: self.start.map(f64::from),
            end: self.end.map(f64::from),
        })
    }
}

/// Where the aircraft's last row was: none yet, on the ground, or airborne (lifted from the ground
/// or first seen in the air, and the highest it has been since, m above the ground).
#[derive(Clone, Copy)]
enum Phase {
    Start,
    Ground,
    Airborne { lifted: bool, highest_m: f32 },
}

/// A view's last segment and the flight's stretch on the ground then (it reaches the next).
#[derive(Clone, Copy)]
struct Seen {
    segment: Airborne,
    episode: u16,
}

/// One aircraft flight read row by row: the stretch on the ground of each ground row, the
/// stretches a take-off ended, and the low ends.
pub struct FlightTrack {
    flight_id: u64,
    /// One more after every time it was airborne (each touch-and-go of a circuit rolls again).
    episode: u16,
    phase: Phase,
    /// The last airborne segment of every provider, and of the primary's own.
    every: Option<Seen>,
    primary: Option<Seen>,
    /// Both views and the number of ends found when it last left the ground: an airborne moment
    /// that returns as a flicker is taken back.
    lifted_from: Option<(Option<Seen>, Option<Seen>, usize)>,
}

impl FlightTrack {
    pub fn new(flight_id: u64) -> Self {
        FlightTrack {
            flight_id,
            episode: 0,
            phase: Phase::Start,
            every: None,
            primary: None,
            lifted_from: None,
        }
    }

    pub fn flight_id(&self) -> u64 {
        self.flight_id
    }

    /// A ground row: returns the stretch on the ground it belongs to; a flicker takes back the
    /// ends it found from `ends`.
    pub fn ground(&mut self, lift_offs: &mut HashSet<(u64, u16)>, ends: &mut Vec<LowEnd>) -> u16 {
        if let Phase::Airborne { lifted, highest_m } = self.phase {
            // An end at the low ends' 150 m makes a segment average at least half of it.
            if !lifted || highest_m >= 0.5 * LOW_END_M {
                self.episode += 1;
            } else if let Some((every, primary, found)) = self.lifted_from {
                // Airborne a moment and never up to the low ends: the air-ground flag flickering
                // within a roll (a landing accelerating to vacate takes Stage 1's take-off flag),
                // not a touch-and-go.
                lift_offs.remove(&(self.flight_id, self.episode));
                (self.every, self.primary) = (every, primary);
                ends.truncate(found);
            }
        }
        self.phase = Phase::Ground;
        self.episode
    }

    /// An airborne row; the low ends it finds go to `ends`.
    pub fn airborne(
        &mut self,
        segment: Airborne,
        lift_offs: &mut HashSet<(u64, u16)>,
        ends: &mut Vec<LowEnd>,
    ) {
        let id = self.flight_id;
        let turnaround = matches!(self.phase, Phase::Airborne { .. })
            && self
                .every
                .is_some_and(|last| turned_around(&last.segment, &segment));
        if turnaround {
            self.episode += 1;
        }
        self.phase = match self.phase {
            Phase::Ground => {
                lift_offs.insert((id, self.episode));
                self.lifted_from = Some((self.every, self.primary, ends.len()));
                Phase::Airborne {
                    lifted: true,
                    highest_m: segment.height_m,
                }
            }
            Phase::Airborne { lifted, highest_m } if !turnaround => Phase::Airborne {
                lifted,
                highest_m: highest_m.max(segment.height_m),
            },
            _ => Phase::Airborne {
                lifted: false,
                highest_m: segment.height_m,
            },
        };
        let episode = self.episode;
        // Every provider's view gives the ends only the secondary saw, the primary's its own.
        let secondary = view_ends(id, self.every, segment, episode)
            .into_iter()
            .flatten();
        ends.extend(secondary.filter(|end| end.secondary_only));
        let seen = Seen { segment, episode };
        if !segment.secondary_only {
            ends.extend(
                view_ends(id, self.primary, segment, episode)
                    .into_iter()
                    .flatten(),
            );
            self.primary = Some(seen);
        }
        self.every = Some(seen);
    }

    /// The flight's last ends: where each view lost it.
    pub fn close(self, ends: &mut Vec<LowEnd>) {
        if let Some(last) = self.every.filter(|last| last.segment.secondary_only) {
            ends.extend(
                last.segment
                    .low_end(self.flight_id, last.episode + 1, false),
            );
        }
        if let Some(last) = self.primary {
            ends.extend(
                last.segment
                    .low_end(self.flight_id, last.episode + 1, false),
            );
        }
    }
}

/// The ends a view of flight `id` finds at `segment` after its `last`: its first, or both sides
/// of the stretches on the ground between them.
fn view_ends(id: u64, last: Option<Seen>, segment: Airborne, episode: u16) -> [Option<LowEnd>; 2] {
    match last {
        None => [None, segment.low_end(id, episode, true)],
        Some(last) if episode > last.episode => [
            last.segment.low_end(id, last.episode + 1, false),
            segment.low_end(id, episode, true),
        ],
        Some(_) => [None, None],
    }
}

/// A break between a descent and a climb, where the aircraft landed and left again unseen: the
/// next segment starts further from the last one's end than Stage 1's shortest segment (a shorter
/// step is a segment it dropped, not a gap in the track).
fn turned_around(before: &Airborne, after: &Airborne) -> bool {
    let ([lat, lon], [next_lat, next_lon]) = (before.end, after.start);
    flat_distance_m(lat, lon, next_lat, next_lon) > MIN_SEGMENT_LENGTH_M
        && !before.departure
        && after.departure
}
