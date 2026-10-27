//! The runways' take-off and landing rolls at the receiver (`tiles::aircraft_rolls`), by path (a
//! runway direction's take-offs or landings): each roll's Doc 29 segments (`physics::doc29::roll`)
//! times its movements, heard in the aircraft layer beside the boxes, in the boxes' atmosphere (its
//! square's) with their impedance, a path's classes passing the receiver in one stream for as long
//! as their own speeds take.

use super::boxes::{AircraftReceiver, FLIGHT_KINDS};
use physics::bands::{PERIOD_HOURS, PERIODS, energy, lden_energy};
use physics::doc29::atmosphere::{PlaceAtmosphere, receiver_impedance_db};
use physics::doc29::npd::{Installation, NPD_NEAREST_SLANT_M, class_anchor};
use physics::doc29::roll::{ENGINE_HEIGHT_M, RollKind, roll_sel_energy};
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
/// builder writes it (`build/src/airport/roll_tiles.rs`), and the classes rolling it, whose passes
/// make one stream.
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
    // A path is its start, heading (as decoded, exact) and operation.
    let mut paths: BTreeMap<([i16; 2], u64, bool), HeardPath> = BTreeMap::new();
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
        let key = (roll.start, roll.heading_rad.to_bits(), roll.landing);
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

/// The rolls heard at a receiver: per period their energy (Leq, linear) and energy times their
/// Kurze lambda, their Lden energy per flight kind, and how many were within reach.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RollSums {
    pub energy: [f64; PERIODS],
    pub energy_lambda: [f64; PERIODS],
    pub kinds: [f64; FLIGHT_KINDS],
    pub heard: usize,
}

/// Every path of `paths` whose start lies within [`ROLL_REACH_M`] of the receiver, its rolls at
/// the NPD curves of the receiver's elevation (Doc 29 4.2.1) as the boxes take them. A class's
/// passes take the time its roll needs over the pass's distance (at least the NPD curves' nearest,
/// where the sound exposure is read) around its point nearest the receiver, the stretch shifted to
/// lie on the roll. The classes of a path passing the receiver nearest at one point make one stream,
/// whose lambda is theirs summed, as a box's flights do: those whose rolls reach past the
/// receiver's nearest point on the path, and those ending at one point before it; the streams join
/// at their energy-weighted lambda, as the boxes do, so a short roll over long before a long one
/// passes adds its sound at its own rate, not to the long one's.
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
        // The receiver's nearest point on the path's line from its start.
        let along_m = (-(start[0] * path.direction[0] + start[1] * path.direction[1])).max(0.0);
        // Per stream: where its classes pass nearest, their energy and their lambda per period.
        let mut streams: Vec<(f64, [f64; PERIODS], [f64; PERIODS])> = Vec::new();
        for roll in &path.rolls {
            let (field_m, roll_m) = (roll.profile.field_altitude_m, roll.profile.length_m);
            let up_m = field_m + ENGINE_HEIGHT_M - receiver.altitude_m;
            let sel_energy = roll_sel_energy(
                roll.profile.kind,
                &roll.steps,
                ([start[0], start[1], up_m], path.direction),
                field_m - receiver.altitude_m,
                horizons,
            );
            let nearest_m = along_m.min(roll_m);
            let nearest = [0, 1].map(|k| start[k] + path.direction[k] * nearest_m);
            let reach_m = (nearest[0].hypot(nearest[1]).hypot(up_m)).max(NPD_NEAREST_SLANT_M);
            let from_m = (nearest_m - 0.5 * reach_m).clamp(0.0, (roll_m - reach_m).max(0.0));
            let passing_s = roll.profile.seconds_between(from_m, from_m + reach_m);
            let at = match streams.iter().position(|stream| stream.0 == nearest_m) {
                Some(at) => at,
                None => {
                    streams.push((nearest_m, [0.0; PERIODS], [0.0; PERIODS]));
                    streams.len() - 1
                }
            };
            let stream = &mut streams[at];
            let mut alone = [0.0; PERIODS];
            for (period, value) in alone.iter_mut().enumerate() {
                let per_second = roll.movements_per_day[period] / (PERIOD_HOURS[period] * 3_600.0);
                *value = per_second * sel_energy * impedance_gain;
                stream.1[period] += *value;
                stream.2[period] += per_second * passing_s;
            }
            sums.kinds[roll.flight_kind] += lden_energy(&alone);
            sums.heard += 1;
        }
        for (_, energy, lambda) in streams {
            for period in 0..PERIODS {
                sums.energy[period] += energy[period];
                sums.energy_lambda[period] += energy[period] * lambda[period];
            }
        }
    }
    sums
}
