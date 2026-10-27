//! Low ends: a flight read segment by segment ends where ADS-B found or lost it low, at its first
//! and last segments and on both sides of every stretch on the ground, seen or a turnaround nobody
//! saw, in every provider's view and in the primary's own; each movement counts once, at the
//! weight of the provider that saw it.

use super::*;
use crate::airport::low_ends::{Airborne, FlightTrack, LowEnd};
use crate::airport::rolls::movements;

/// An A320's airborne segment from `from_m` to `to_m` metres east along the test runway's line,
/// `height_m` above the ground, climbing (a departure) or descending, seen by the primary.
fn flying(from_m: f64, to_m: f64, height_m: f32, departure: bool) -> Airborne {
    let point = |east_m| at(0.0, east_m).map(|degrees| degrees as f32);
    Airborne {
        start: point(from_m),
        end: point(to_m),
        height_m,
        departure,
        secondary_only: false,
        class: class("A320-232"),
        period: 0,
    }
}

fn secondary(segment: Airborne) -> Airborne {
    Airborne {
        secondary_only: true,
        ..segment
    }
}

/// The low ends and take-offs of flight 1 read from `rows` (`None` a ground row).
fn read(rows: &[Option<Airborne>]) -> (Vec<LowEnd>, HashSet<(u64, u16)>) {
    let (mut ends, mut lift_offs) = (Vec::new(), HashSet::new());
    let mut track = FlightTrack::new(1);
    for row in rows {
        match row {
            Some(segment) => track.airborne(*segment, &mut lift_offs, &mut ends),
            None => {
                track.ground(&mut lift_offs, &mut ends);
            }
        }
    }
    track.close(&mut ends);
    (ends, lift_offs)
}

/// The rolls the ends place on the test runway: (landing, stretch on the ground, primary), the
/// take-offs first.
fn rolls_of(ends: &[LowEnd]) -> Vec<(bool, u16, bool)> {
    let runways = Runways::new(&airport(10));
    let mut rolls: Vec<_> = (movements(Vec::new(), ends, &runways).iter())
        .map(|roll| (roll.key.landing, roll.episode, !roll.secondary_only))
        .collect();
    rolls.sort();
    rolls
}

/// Stage 1 keeps an arrival and its next departure one flight when no receiver saw it on the
/// ground: the descent lost low and the climb found low again land and take off once each, from
/// one stretch on the ground, eastwards on the runway; a gap on final is no turnaround.
#[test]
fn a_turnaround_unseen_on_the_ground_lands_and_takes_off_again() {
    let approach = flying(-3_000.0, -1_000.0, 100.0, false);
    let turnaround = [
        Some(flying(-20_000.0, -15_000.0, 1_000.0, false)),
        Some(approach),
        Some(flying(3_000.0, 5_000.0, 120.0, true)),
        Some(flying(5_000.0, 15_000.0, 1_500.0, true)),
    ];
    let (ends, _) = read(&turnaround);
    assert_eq!(rolls_of(&ends), vec![(false, 1, true), (true, 1, true)]);
    let runways = Runways::new(&airport(10));
    let rolls = movements(Vec::new(), &ends, &runways);
    assert!(
        rolls
            .iter()
            .all(|roll| roll.key.forward && roll.key.runway == 0)
    );
    let gap_on_final = [Some(approach), Some(flying(-800.0, -200.0, 40.0, false))];
    let (ends, _) = read(&gap_on_final);
    assert_eq!(rolls_of(&ends), vec![(true, 1, true)]);
}

/// A turnaround's break is longer than Stage 1's shortest segment (10 m): a shorter step between a
/// descent and a climb is a segment it dropped, a longer one a gap in the track. An approach lost
/// over the runway and a climb found 15 m on land and take off; found 5 m on, neither.
#[test]
fn a_turnarounds_break_is_longer_than_stage_1s_shortest_segment() {
    let approach = flying(-3_000.0, 1_000.0, 60.0, false);
    let climb_from = |east_m| flying(east_m, 2_000.0, 80.0, true);
    let above = flying(2_000.0, 12_000.0, 900.0, true);
    let (ends, _) = read(&[Some(approach), Some(climb_from(1_015.0)), Some(above)]);
    assert_eq!(rolls_of(&ends), vec![(false, 1, true), (true, 1, true)]);
    let (ends, _) = read(&[Some(approach), Some(climb_from(1_005.0)), Some(above)]);
    assert_eq!(rolls_of(&ends), vec![]);
}

/// A last segment that rises, Stage 1's climb flag unset (under 500 ft/min), is still the
/// approach's: barometric altitude comes in 25 ft steps, and at Madrid on 2025-09-01 each of the 78
/// of the primary's 575 landings whose last segment rose was a landing the secondary also saw.
/// Lost there the flight lands; found again climbing beyond a break, it landed and left again.
#[test]
fn a_rising_last_segment_still_lands() {
    let approach = flying(-5_000.0, -3_000.0, 110.0, false);
    let rising = flying(-3_000.0, -1_000.0, 130.0, false);
    let (ends, _) = read(&[Some(approach), Some(rising)]);
    assert_eq!(rolls_of(&ends), vec![(true, 1, true)]);
    let climb = flying(2_600.0, 3_200.0, 40.0, true);
    let above = flying(3_200.0, 12_000.0, 900.0, true);
    let (ends, _) = read(&[Some(approach), Some(rising), Some(climb), Some(above)]);
    assert_eq!(rolls_of(&ends), vec![(false, 1, true), (true, 1, true)]);
}

/// The two operations of one stop are two movements, each at the weight of the provider that saw
/// it: a primary approach and a climb only the secondary found count a primary landing and a
/// secondary take-off; the primary's own first low segment witnesses its take-off, which the
/// secondary's end of it does not count again.
#[test]
fn each_operation_counts_at_the_weight_of_the_provider_that_saw_it() {
    let approach = flying(-3_000.0, -1_000.0, 100.0, false);
    let climb = flying(3_000.0, 5_000.0, 120.0, true);
    let (ends, _) = read(&[Some(approach), Some(secondary(climb))]);
    assert_eq!(rolls_of(&ends), vec![(false, 1, false), (true, 1, true)]);
    let (ends, _) = read(&[Some(secondary(approach)), Some(climb)]);
    assert_eq!(rolls_of(&ends), vec![(false, 1, true), (true, 1, false)]);
    let rows = [
        Some(secondary(flying(2_600.0, 3_200.0, 40.0, true))),
        Some(flying(3_200.0, 4_500.0, 110.0, true)),
        Some(flying(4_500.0, 12_000.0, 900.0, true)),
    ];
    let (ends, _) = read(&rows);
    assert_eq!(rolls_of(&ends), vec![(false, 0, true)]);
}

/// A stop seen on the ground in the middle of a flight (shorter than Stage 1's five-minute rest)
/// lands and takes off from the ends beside it, by whichever provider saw them.
#[test]
fn a_stop_seen_on_the_ground_lands_and_takes_off_by_its_ends() {
    let approach = flying(-3_000.0, -1_000.0, 100.0, false);
    let climb = flying(2_600.0, 3_200.0, 40.0, true);
    let rows = [Some(approach), None, None, Some(climb)];
    let (ends, lift_offs) = read(&rows.map(|row| row.map(secondary)));
    assert_eq!(rolls_of(&ends), vec![(false, 1, false), (true, 1, false)]);
    assert_eq!(lift_offs, HashSet::from([(1, 1)]));
    let rows = [
        Some(approach),
        None,
        Some(secondary(climb)),
        Some(flying(3_400.0, 4_500.0, 110.0, true)),
    ];
    let (ends, _) = read(&rows);
    assert_eq!(rolls_of(&ends), vec![(false, 1, true), (true, 1, true)]);
}

/// The air-ground flag flickering within a roll is no lift-off and no low end: the stretch on the
/// ground goes on, a landing that accelerates to vacate (Stage 1's take-off flag) takes off nowhere,
/// and a take-off rolls once, from its real lift-off.
#[test]
fn a_flicker_within_a_roll_is_taken_back() {
    let flicker = flying(1_000.0, 1_100.0, 20.0, true);
    let approach = flying(-3_000.0, -1_000.0, 100.0, false);
    let (ends, lift_offs) = read(&[Some(approach), None, Some(flicker), None]);
    assert_eq!(rolls_of(&ends), vec![(true, 1, true)]);
    assert!(lift_offs.is_empty());
    let rows = [
        None,
        Some(flicker),
        None,
        Some(flying(1_800.0, 2_600.0, 60.0, true)),
    ];
    let (ends, lift_offs) = read(&rows);
    assert_eq!(rolls_of(&ends), vec![(false, 0, true)]);
    assert_eq!(lift_offs, HashSet::from([(1, 0)]));
}
