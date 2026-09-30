//! A tile's aircraft boxes at the receiver: every box through the click-time equation
//! (`physics::doc29::boxes`), all of them, since a box costs well under a microsecond; their SEL
//! sums per period become period energies (Leq), as the ground layers report.

use crate::candidates::lden_weighted;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::screening::ReceiverHorizons;
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

/// One tile's boxes at the receiver.
pub struct TileAnswer {
    /// Period energies (Leq, linear) summed over the boxes within reach.
    pub energy: [f64; PERIODS],
    pub boxes: usize,
    /// The loudest boxes (index, Lden-weighted energy), at most `loudest` of them.
    pub loudest: Vec<(usize, f64)>,
}

/// The boxes of one tile at the receiver.
pub fn tile_energy(
    aircraft: &Aircraft<'_>,
    tile: TileId,
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
    loudest: usize,
) -> TileAnswer {
    let per_box: Vec<([f64; PERIODS], usize)> = (0..aircraft.box_count())
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
        .collect();
    let mut answer = TileAnswer {
        energy: [0.0; PERIODS],
        boxes: 0,
        loudest: Vec::new(),
    };
    for (index, (energy, count)) in per_box.iter().enumerate() {
        for (total, value) in answer.energy.iter_mut().zip(energy) {
            *total += value;
        }
        answer.boxes += count;
        if *count > 0 {
            answer.loudest.push((index, lden_weighted(energy)));
        }
    }
    answer
        .loudest
        .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    answer.loudest.truncate(loudest);
    answer
}
