//! A tile's aircraft boxes at the receiver: every box through the click-time equation
//! (`physics::doc29::boxes`), all of them, since a box costs well under a microsecond; their SEL
//! sums per period become period energies (Leq), as the ground layers report.

use physics::bands::{PERIOD_HOURS, PERIODS, energy, lden_energy};
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::corrections::INSTALLATION_CORRECTION_MAX_DB;
use physics::doc29::npd::lamax_rise_bound_db;
use physics::doc29::screening::ReceiverHorizons;
use rayon::prelude::*;
use tiles::aircraft::{Aircraft, AircraftBox, Group};
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

/// Speed of a flight near the ground for its passing's duration (m/s): 135 kt.
pub const FLIGHT_SPEED_M_S: f64 = 70.0;

/// What the flights heard are: wing-mounted jets (airliners), fuselage-mounted jets (regional and
/// business jets), propeller aircraft (Doc 29's engine installations) and helicopters.
pub const FLIGHT_KINDS: usize = 4;

/// One box at the receiver.
struct BoxAnswer {
    energy: [f64; PERIODS],
    lambda: [f64; PERIODS],
    /// The box's received Lden energy shared over the flight kinds.
    kinds: [f64; FLIGHT_KINDS],
    bound: Option<f64>,
    within_reach: bool,
}

const BEYOND_REACH: BoxAnswer = BoxAnswer {
    energy: [0.0; PERIODS],
    lambda: [0.0; PERIODS],
    kinds: [0.0; FLIGHT_KINDS],
    bound: None,
    within_reach: false,
};

/// One tile's boxes at the receiver.
pub struct TileAnswer {
    /// Period energies (Leq, linear) summed over the boxes within reach.
    pub energy: [f64; PERIODS],
    /// The boxes' Lden energy per flight kind.
    pub kinds: [f64; FLIGHT_KINDS],
    /// Per period the boxes' energies times their flights' Kurze lambda (flights per second of
    /// the period times the slant over the speed): the energy-weighted lambda of the flights heard.
    pub energy_lambda: [f64; PERIODS],
    pub boxes: usize,
    /// Per box within reach that keeps pieces: its index and the most maximum level (dB) its
    /// pieces can reach at the receiver (Eq. 4-8a: Delta_I adds at most
    /// [`INSTALLATION_CORRECTION_MAX_DB`], Lambda only attenuates).
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
    let per_box: Vec<BoxAnswer> = (0..aircraft.box_count())
        .into_par_iter()
        .with_min_len(1_024)
        .map(|index| {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let centroid = frame.metres_of_steps([global.x as f64, global.y as f64]);
            let east_m = centroid[0] - receiver.position[0];
            let north_m = centroid[1] - receiver.position[1];
            if east_m.hypot(north_m) > AIRCRAFT_REACH_M {
                return BEYOND_REACH;
            }
            let at_receiver = AircraftBoxAtReceiver {
                centroid_m: [
                    east_m,
                    north_m,
                    record.centroid_altitude_m - receiver.altitude_m,
                ],
                axis_rad: record.axis_rad,
                gradient: record.gradient,
                gradient_spread: record.gradient_spread,
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
            // The box's flights of a year split over the periods as its SEL energy is, each
            // period's rate over its hours; lambda is that rate times the slant over the speed.
            let period_sel: [f64; PERIODS] =
                std::array::from_fn(|period| physics::bands::energy(sel.sel_db[period]));
            let total_sel: f64 = period_sel.iter().sum();
            let slant = sel.closest.on_line_m[0]
                .hypot(sel.closest.on_line_m[1])
                .hypot(sel.closest.on_line_m[2]);
            let lambda: [f64; PERIODS] = std::array::from_fn(|period| {
                if total_sel <= 0.0 {
                    return 0.0;
                }
                let flights = f64::from(record.flights) / 365.25 * period_sel[period] / total_sel;
                flights / (PERIOD_HOURS[period] * 3_600.0) * slant / FLIGHT_SPEED_M_S
            });
            let bound = (record.piece_count > 0).then(|| {
                record.loudest_lamax_db
                    + lamax_rise_bound_db(nearest_slant_m(&record, tile, frame, receiver))
                    + INSTALLATION_CORRECTION_MAX_DB
            });
            // The installation shares are the box's over the whole day (the tiles keep no share per
            // period), so a box whose jets fly by day and propellers by night splits its Lden as the
            // day's mix.
            let [wing, fuselage, propeller] = sel.installation_fractions;
            let shares = match record.group {
                Group::FixedWing => [wing, fuselage, propeller, 0.0],
                Group::Helicopter => [0.0, 0.0, 0.0, 1.0],
            };
            let kinds = shares.map(|share| share * lden_energy(&energy));
            BoxAnswer {
                energy,
                lambda,
                kinds,
                bound,
                within_reach: true,
            }
        })
        .collect();
    let mut answer = TileAnswer {
        energy: [0.0; PERIODS],
        kinds: [0.0; FLIGHT_KINDS],
        energy_lambda: [0.0; PERIODS],
        boxes: 0,
        lamax_bounds: Vec::new(),
    };
    for (index, heard) in per_box.iter().enumerate() {
        for period in 0..PERIODS {
            answer.energy[period] += heard.energy[period];
            answer.energy_lambda[period] += heard.energy[period] * heard.lambda[period];
        }
        for (kind, part) in answer.kinds.iter_mut().zip(heard.kinds) {
            *kind += part;
        }
        answer.boxes += usize::from(heard.within_reach);
        if let Some(bound) = heard.bound {
            answer.lamax_bounds.push((index, bound));
        }
    }
    answer
}
