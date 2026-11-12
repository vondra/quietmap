//! A ceiling of an aircraft box's period energies, and of them times its flights' lambda, at any
//! receiver of an area: what a sum of the boxes may leave out, proven rather than estimated.
//!
//! [`super::boxes::tile_flight_sums`] reads a box's SEL at a receiver (its ceiling over the area:
//! `physics::doc29::box_ceiling`), times the NPD levels' impedance gain at the receiver's elevation
//! (fixed wing; it falls with the elevation, so the area's lowest or highest takes the larger),
//! over the period's seconds; its lambda grows with the closest slant, at most the area's farthest
//! from the box's centroid.

use super::boxes::{
    AIRCRAFT_REACH_M, AircraftReceiver, ReceiverArea, box_lambda, centroid_offset_m,
};
use physics::bands::{PERIOD_HOURS, PERIODS, energy};
use physics::doc29::atmosphere::receiver_impedance_db;
use physics::doc29::box_ceiling::{box_sel_ceiling, farthest_centroid_slant_m};
use physics::doc29::boxes::AircraftBoxAtReceiver;
use tiles::aircraft::{AircraftBox, Group};
use tiles::geo::{LocalFrame, TileId};

/// A box's ceilings: per period its energy and its energy times its flights' lambda.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BoxCeiling {
    pub energy: [f64; PERIODS],
    pub energy_lambda: [f64; PERIODS],
}

/// The receivers a ceiling holds for: an area's middle, how far its receivers stand from it, and
/// the NPD levels' impedance gain at most among their elevations.
#[derive(Debug, Clone, Copy)]
pub struct CeilingArea {
    middle: AircraftReceiver,
    half_extent_m: [f64; 3],
    impedance_gain: f64,
}

impl CeilingArea {
    pub fn of(area: ReceiverArea) -> Self {
        let (middle, half_extent_m) = area.middle_and_half_extent_m();
        let impedance_gain = (area
            .altitudes_m
            .map(|altitude_m| energy(receiver_impedance_db(altitude_m))))
        .into_iter()
        .fold(0.0, f64::max);
        CeilingArea {
            middle,
            half_extent_m,
            impedance_gain,
        }
    }
}

/// The ceiling of box `record` of `tile` at any receiver of `area`, screened or not, as
/// `tile_flight_sums` sums it wholly (zero where its centroid is beyond the aircraft reach of every
/// receiver, on the card too).
pub fn box_flight_ceiling(
    record: &AircraftBox,
    tile: TileId,
    frame: &LocalFrame,
    area: &CeilingArea,
) -> BoxCeiling {
    let (middle, half_extent_m) = (area.middle, area.half_extent_m);
    let [east_m, north_m] = centroid_offset_m(record.place.centroid, tile, frame, middle);
    let nearest_m = (east_m.abs() - half_extent_m[0])
        .max(0.0)
        .hypot((north_m.abs() - half_extent_m[1]).max(0.0));
    // A metre over the reach for the card, which takes the distance in single precision.
    if nearest_m > AIRCRAFT_REACH_M + 1.0 {
        return BoxCeiling::default();
    }
    let centroid_m = [
        east_m,
        north_m,
        record.centroid_altitude_m - middle.altitude_m,
    ];
    let at_middle = AircraftBoxAtReceiver {
        centroid_m,
        axis_rad: record.axis_rad,
        gradient: record.gradient,
        gradient_spread: record.gradient_spread,
        piece_length_m: record.piece_length_m,
        levels_db: &record.energy_db,
        tail_levels_db: &record.tail_energy_db,
        lg_scaled_distance: &record.lg_scaled_distance,
        installation_shares: record.installation_shares,
        ground_m: record.place.ground_m - middle.altitude_m,
    };
    let sel = box_sel_ceiling(&at_middle, half_extent_m);
    let impedance_gain = match record.place.group {
        Group::FixedWing => area.impedance_gain,
        Group::Helicopter => 1.0,
    };
    let energy: [f64; PERIODS] = std::array::from_fn(|period| {
        sel[period] * impedance_gain / (PERIOD_HOURS[period] * 3_600.0)
    });
    let lambda = box_lambda(record, farthest_centroid_slant_m(centroid_m, half_extent_m));
    BoxCeiling {
        energy,
        energy_lambda: std::array::from_fn(|period| energy[period] * lambda[period]),
    }
}

#[cfg(test)]
#[path = "ceilings_tests.rs"]
mod tests;
