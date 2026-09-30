//! A tile's aircraft boxes at the receiver: every box through the click-time equation
//! (`physics::doc29::boxes`), all of them, since a box costs well under a microsecond; their SEL
//! sums per period become period energies (Leq), as the ground layers report.

use super::horizons::Horizons;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use rayon::prelude::*;
use tiles::aircraft::Aircraft;
use tiles::geo::{LocalFrame, TileId};

/// Aircraft within this horizontal distance of the receiver are heard (dev4's airborne reach;
/// the kernel has no cut of its own).
pub const AIRCRAFT_REACH_M: f64 = 16_000.0;

/// The receiver the boxes are heard at: click metres and altitude.
#[derive(Debug, Clone, Copy)]
pub struct AircraftReceiver {
    pub position: [f64; 2],
    pub altitude_m: f64,
}

/// The period energies (Leq, linear) of one tile's boxes and how many were within reach.
pub fn tile_energy(
    aircraft: &Aircraft<'_>,
    tile: TileId,
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &Horizons,
) -> ([f64; PERIODS], usize) {
    (0..aircraft.box_count())
        .into_par_iter()
        .with_min_len(1_024)
        .map(|index| {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let centroid = frame.metres_of_steps([global.x as f64, global.y as f64]);
            let east_m = centroid[0] - receiver.position[0];
            let north_m = centroid[1] - receiver.position[1];
            if east_m.hypot(north_m) > AIRCRAFT_REACH_M {
                return ([0.0; PERIODS], 0);
            }
            let at_receiver = AircraftBoxAtReceiver {
                centroid_m: [
                    east_m,
                    north_m,
                    record.centroid_altitude_m - receiver.altitude_m,
                ],
                axis_rad: record.axis_rad,
                gradient: record.gradient,
                piece_length_m: record.piece_length_m,
                levels_db: &record.energy_db,
                scaled_distance_m: &record.scaled_distance_m,
                installation_shares: record.installation_shares,
                ground_m: record.ground_m - receiver.altitude_m,
            };
            let energy =
                box_sel_at_receiver(&at_receiver, horizons).map_or([0.0; PERIODS], |sel| {
                    std::array::from_fn(|period| {
                        10f64.powf(sel.sel_db[period] / 10.0) / (PERIOD_HOURS[period] * 3_600.0)
                    })
                });
            (energy, 1)
        })
        .reduce(
            || ([0.0; PERIODS], 0),
            |a, b| {
                (
                    std::array::from_fn(|period| a.0[period] + b.0[period]),
                    a.1 + b.1,
                )
            },
        )
}
