//! A tile's aircraft boxes at the receiver: every box through the click-time equation
//! (`physics::doc29::boxes`), all of them, since a box costs well under a microsecond; their SEL
//! sums per period become period energies (Leq), as the ground layers report.

use physics::bands::{PERIOD_HOURS, PERIODS, energy};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::npd::lamax_rise_bound_db;
use physics::doc29::screening::ReceiverHorizons;
use rayon::prelude::*;
use tiles::aircraft::{Aircraft, AircraftBox};
use tiles::geo::{LocalFrame, Mercator, TileId};

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
    /// Per box within reach that keeps pieces: its index and the most LAmax (dB) its pieces can
    /// reach at the receiver.
    pub lamax_bounds: Vec<(usize, f64)>,
}

/// The nearest a box's pieces can come to the receiver (m): its cell and slab widened by a
/// quarter edge (a piece reaches past its box by an eighth at most), the first band down to any
/// depth.
fn nearest_slant_m(
    record: &AircraftBox,
    tile: TileId,
    frame: &LocalFrame,
    receiver: AircraftReceiver,
) -> f64 {
    let cells = f64::from(1u32 << (record.zoom - 12));
    let corner = |offset: f64| {
        frame.to_metres(Mercator {
            x: f64::from(tile.x) + (f64::from(record.cell[0]) + offset) / cells,
            y: f64::from(tile.y) + (f64::from(record.cell[1]) + offset) / cells,
        })
    };
    let (a, b) = (corner(0.0), corner(1.0));
    let margin = record.height_m / 4.0;
    let gap = |low: f64, high: f64, at: f64| (low - margin - at).max(at - high - margin).max(0.0);
    let east = gap(a[0].min(b[0]), a[0].max(b[0]), receiver.position[0]);
    let north = gap(a[1].min(b[1]), a[1].max(b[1]), receiver.position[1]);
    let floor = if record.clearance_m == 0.0 {
        f64::NEG_INFINITY
    } else {
        record.ground_m + record.clearance_m
    };
    let top = record.ground_m + record.clearance_m + record.height_m;
    let up = gap(floor, top, receiver.altitude_m);
    east.hypot(north).hypot(up)
}

/// The boxes of one tile at the receiver.
pub fn tile_energy(
    aircraft: &Aircraft<'_>,
    tile: TileId,
    frame: &LocalFrame,
    receiver: AircraftReceiver,
    horizons: &(impl ReceiverHorizons + Sync),
) -> TileAnswer {
    let per_box: Vec<([f64; PERIODS], Option<f64>, usize)> = (0..aircraft.box_count())
        .into_par_iter()
        .with_min_len(1_024)
        .map(|index| {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let centroid = frame.metres_of_steps([global.x as f64, global.y as f64]);
            let east_m = centroid[0] - receiver.position[0];
            let north_m = centroid[1] - receiver.position[1];
            if east_m.hypot(north_m) > AIRCRAFT_REACH_M {
                return ([0.0; PERIODS], None, 0);
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
                tail_levels_db: &record.tail_energy_db,
                lg_scaled_distance: &record.lg_scaled_distance,
                installation_shares: record.installation_shares,
                ground_m: record.ground_m - receiver.altitude_m,
            };
            let sel = box_sel_at_receiver(&at_receiver, horizons);
            let energy: [f64; PERIODS] = std::array::from_fn(|period| {
                energy(sel.sel_db[period]) / (PERIOD_HOURS[period] * 3_600.0)
            });
            let bound = (record.piece_count > 0).then(|| {
                record.loudest_lamax_db
                    + lamax_rise_bound_db(nearest_slant_m(&record, tile, frame, receiver))
            });
            (energy, bound, 1)
        })
        .collect();
    let mut answer = TileAnswer {
        energy: [0.0; PERIODS],
        boxes: 0,
        lamax_bounds: Vec::new(),
    };
    for (index, (energy, bound, count)) in per_box.iter().enumerate() {
        for (total, value) in answer.energy.iter_mut().zip(energy) {
            *total += value;
        }
        answer.boxes += count;
        if let Some(bound) = bound {
            answer.lamax_bounds.push((index, *bound));
        }
    }
    answer
}
