//! The runways' take-off and landing rolls at the receiver (`tiles::aircraft_rolls`), by path (a
//! runway direction's take-offs or landings): each roll's Doc 29 segments (`physics::doc29::roll`)
//! times its movements, heard in the aircraft layer beside the boxes, in the boxes' atmosphere (its
//! square's) with their impedance, each segment heard for the seconds the aircraft rolls it.

use super::boxes::{AircraftReceiver, FLIGHT_KINDS};
use physics::bands::{PERIOD_HOURS, PERIODS, energy, lden_energy};
use physics::doc29::atmosphere::{PlaceAtmosphere, receiver_impedance_db};
use physics::doc29::npd::{Installation, class_anchor};
use physics::doc29::roll::{ENGINE_HEIGHT_M, RollKind, roll_step_sel_energy};
use physics::doc29::roll_profile::{RollProfile, RollStep};
use physics::doc29::screening::ReceiverHorizons;
use std::collections::BTreeMap;
use tiles::aircraft_rolls::Roll;
use tiles::geo::{LocalFrame, Mercator, TileId};

/// Rolls whose start lies within this horizontal distance of the receiver are heard (m): there a
/// hundred take-offs a day of the loudest class leave 33 dB Lden (`popup/tests/rolls.rs`).
pub const ROLL_REACH_M: f64 = 8_000.0;

/// One class rolling a path: its profile (kind, class, length, speed and field altitude) and
/// segments, its movements, and its flight kind (`FLIGHT_KINDS`: its class's engine installation).
#[derive(Debug, Clone)]
pub struct HeardRoll {
    pub profile: RollProfile,
    pub steps: Vec<RollStep>,
    pub movements_per_day: [f64; PERIODS],
    pub flight_kind: usize,
}

/// A runway direction's take-offs or landings: their start (the start of roll, the touchdown
/// point), their direction of travel (a unit vector, east and north), one for every class as the
/// builder writes it (`build/src/airport/roll_tiles.rs`), and the classes rolling it.
#[derive(Debug, Clone)]
pub struct HeardPath {
    pub start: Mercator,
    pub direction: [f64; 2],
    pub rolls: Vec<HeardRoll>,
}

/// The paths of a tile's rolls ready for receivers, their curves in `place`, the yearly
/// atmosphere of the tile's square (`tiles::square::Square::middle_tile`); a roll without segments
/// (a helicopter's) is dropped.
pub fn prepare(tile: TileId, rolls: &[Roll], place: &PlaceAtmosphere) -> Vec<HeardPath> {
    // A path is its start and heading (as decoded, exact); each roll carries its own kind.
    let mut paths: BTreeMap<([i16; 2], u64), HeardPath> = BTreeMap::new();
    for roll in rolls {
        let kind = if roll.landing {
            RollKind::Landing
        } else {
            RollKind::TakeOff
        };
        let class = usize::from(roll.class);
        let profile = RollProfile {
            kind,
            class,
            length_m: roll.length_m,
            speed_ms: roll.speed_ms,
            field_altitude_m: roll.field_m,
        };
        let steps: Vec<RollStep> = (profile.steps().into_iter())
            .map(|step| RollStep {
                emission: step.emission.in_atmosphere(place),
                ..step
            })
            .collect();
        if steps.is_empty() {
            continue;
        }
        let key = (roll.start, roll.heading_rad.to_bits());
        let path = paths.entry(key).or_insert_with(|| HeardPath {
            start: tile.to_mercator(roll.start),
            direction: [roll.heading_rad.cos(), roll.heading_rad.sin()],
            rolls: Vec::new(),
        });
        path.rolls.push(HeardRoll {
            profile,
            steps,
            movements_per_day: roll.movements_per_day,
            flight_kind: match class_anchor(class).installation {
                Installation::Wing => 0,
                Installation::Fuselage => 1,
                Installation::Propeller => 2,
            },
        });
    }
    paths.into_values().collect()
}

/// The rolls heard at a receiver: per period their energy (Leq, linear) and their passes (each
/// Doc 29 segment at its received level for the seconds the aircraft spends rolling it); their
/// Lden energy per flight kind, and how many were within reach.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RollSums {
    pub energy: [f64; PERIODS],
    pub passes: crate::aircraft::FlightPasses,
    pub kinds: [f64; FLIGHT_KINDS],
    pub heard: usize,
}

/// Every path of `paths` whose start lies within [`ROLL_REACH_M`] of the receiver, its rolls at
/// the NPD curves of the receiver's elevation (Doc 29 4.2.1) as the boxes take them. Each Doc 29
/// segment enters [`FlightPasses::add_pass`] with its own received energy and the period share the
/// aircraft spends rolling it, just as a flight box does, so each class and segment stays in its
/// own level class without slant-based streams or a point-source pi factor.
pub fn roll_sums(
    paths: &[HeardPath],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &impl ReceiverHorizons,
) -> RollSums {
    let impedance_gain = energy(receiver_impedance_db(receiver.altitude_m));
    let mut sums = RollSums::default();
    for path in paths {
        let [east, north] = frame.to_metres(path.start);
        let start = [east - receiver.position[0], north - receiver.position[1]];
        if start[0].hypot(start[1]) > ROLL_REACH_M {
            continue;
        }
        for roll in &path.rolls {
            let field_m = roll.profile.field_altitude_m;
            let origin = [
                start[0],
                start[1],
                field_m + ENGINE_HEIGHT_M - receiver.altitude_m,
            ];
            let ground_m = field_m - receiver.altitude_m;
            let rates: [f64; PERIODS] = std::array::from_fn(|period| {
                roll.movements_per_day[period] / (PERIOD_HOURS[period] * 3_600.0)
            });
            let mut roll_energy = [0.0; PERIODS];
            for step in &roll.steps {
                let sel_energy = roll_step_sel_energy(
                    roll.profile.kind,
                    step,
                    (origin, path.direction),
                    ground_m,
                    horizons,
                );
                for period in 0..PERIODS {
                    let step_energy = rates[period] * sel_energy * impedance_gain;
                    roll_energy[period] += step_energy;
                    sums.energy[period] += step_energy;
                    sums.passes
                        .add_pass(period, step_energy, rates[period] * step.duration_s);
                }
            }
            sums.kinds[roll.flight_kind] += lden_energy(&roll_energy);
            sums.heard += 1;
        }
    }
    sums
}
