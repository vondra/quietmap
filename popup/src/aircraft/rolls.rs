//! The runways' take-off and landing rolls at the receiver (`tiles::aircraft_rolls`): each roll's
//! Doc 29 segments (`physics::doc29::roll`) times its movements, heard in the aircraft layer
//! beside the boxes, in the boxes' atmosphere (its square's) with their impedance, and passing
//! the receiver for as long as the roll's own speeds take.

use super::boxes::{AircraftReceiver, FLIGHT_KINDS};
use physics::bands::{PERIOD_HOURS, PERIODS, energy, lden_energy};
use physics::doc29::atmosphere::{PlaceAtmosphere, receiver_impedance_db};
use physics::doc29::npd::{Installation, NPD_NEAREST_SLANT_M, class_anchor};
use physics::doc29::roll::{ENGINE_HEIGHT_M, RollKind, roll_heard};
use physics::doc29::roll_profile::{RollProfile, RollStep};
use physics::doc29::screening::ReceiverHorizons;
use std::collections::HashMap;
use tiles::aircraft_rolls::Roll;
use tiles::geo::{LocalFrame, Mercator, TileId};

/// Rolls whose start lies within this horizontal distance of the receiver are heard (m): there a
/// hundred take-offs a day of the loudest class leave 33 dB Lden (`popup/tests/rolls.rs`).
pub const ROLL_REACH_M: f64 = 8_000.0;

/// A roll ready for receivers: its start and direction, its profile (kind, length, speed and
/// field altitude) and segments, and its movements.
#[derive(Debug, Clone)]
pub struct HeardRoll {
    pub start: Mercator,
    /// Unit vector of the direction of travel, east and north.
    pub direction: [f64; 2],
    pub profile: RollProfile,
    pub steps: Vec<RollStep>,
    pub movements_per_day: [f64; PERIODS],
    /// Its flight kind (`FLIGHT_KINDS`): its class's engine installation.
    pub flight_kind: usize,
}

/// The rolls of a tile ready for receivers, their curves in `place`, the yearly atmosphere of the
/// tile's square (`tiles::square::Square::middle_tile`); a roll without segments (a helicopter's) is
/// dropped.
pub fn prepare(tile: TileId, rolls: &[Roll], place: &PlaceAtmosphere) -> Vec<HeardRoll> {
    rolls
        .iter()
        .filter_map(|roll| {
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
            let steps = (profile.steps().into_iter())
                .map(|step| RollStep {
                    emission: step.emission.in_atmosphere(place),
                    ..step
                })
                .collect::<Vec<_>>();
            (!steps.is_empty()).then(|| HeardRoll {
                start: tile.to_mercator(roll.start),
                direction: [roll.heading_rad.cos(), roll.heading_rad.sin()],
                profile,
                steps,
                movements_per_day: roll.movements_per_day,
                flight_kind: match class_anchor(class).installation {
                    Installation::Wing => 0,
                    Installation::Fuselage => 1,
                    Installation::Propeller => 2,
                },
            })
        })
        .collect()
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

/// Every roll of `rolls` whose start lies within [`ROLL_REACH_M`] of the receiver, at the NPD
/// curves of the receiver's elevation (Doc 29 4.2.1) as the boxes take them.
pub fn roll_sums(
    rolls: &[HeardRoll],
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &impl ReceiverHorizons,
) -> RollSums {
    let impedance_gain = energy(receiver_impedance_db(receiver.altitude_m));
    let mut sums = RollSums::default();
    // The classes rolling one path (its start, direction and operation) pass the receiver in one
    // stream, whose lambda is theirs summed, as a box's takes all its flights; the paths join at
    // their energy-weighted lambda, as the boxes do.
    let mut paths: HashMap<(u64, u64, u64, bool), [[f64; PERIODS]; 2]> = HashMap::new();
    for roll in rolls {
        let [east, north] = frame.to_metres(roll.start);
        let start = [east - receiver.position[0], north - receiver.position[1]];
        if start[0].hypot(start[1]) > ROLL_REACH_M {
            continue;
        }
        let field_m = roll.profile.field_altitude_m;
        let heard = roll_heard(
            roll.profile.kind,
            &roll.steps,
            (
                [
                    start[0],
                    start[1],
                    field_m + ENGINE_HEIGHT_M - receiver.altitude_m,
                ],
                roll.direction,
            ),
            field_m - receiver.altitude_m,
            horizons,
        );
        // Kurze's lambda takes half the time the roll spends within the pass's distance of its
        // nearest point (distance over speed at a steady speed), that distance at least the NPD
        // curves' nearest, where its sound exposure is read.
        let passing_s = 0.5
            * (roll.profile).seconds_within(
                heard.nearest_along_m,
                heard.nearest_m.max(NPD_NEAREST_SLANT_M),
            );
        let path = (paths.entry((
            roll.start.x.to_bits(),
            roll.start.y.to_bits(),
            roll.direction[0].to_bits(),
            roll.profile.kind == RollKind::Landing,
        )))
        .or_default();
        let mut alone = [0.0; PERIODS];
        for (period, value) in alone.iter_mut().enumerate() {
            let seconds = PERIOD_HOURS[period] * 3_600.0;
            *value = roll.movements_per_day[period] * heard.sel_energy * impedance_gain / seconds;
            path[0][period] += *value;
            path[1][period] += roll.movements_per_day[period] / seconds * passing_s;
        }
        sums.kinds[roll.flight_kind] += lden_energy(&alone);
        sums.heard += 1;
    }
    for [energy, lambda] in paths.into_values() {
        for period in 0..PERIODS {
            sums.energy[period] += energy[period];
            sums.energy_lambda[period] += energy[period] * lambda[period];
        }
    }
    sums
}
