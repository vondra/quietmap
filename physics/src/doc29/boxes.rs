//! One aircraft box at one receiver: the click-time equation of PLAN section 5. A box sums the
//! pieces of every flight that crossed it; the click reads the summed energies at the slant to
//! its average aircraft (the emission-weighted centroid, one axis and gradient, the mean piece
//! length) and applies to that average piece what the kernel applies to one segment: Delta_F with
//! the box's scaled distance, the lateral attenuation, the installation correction of the box's
//! installation shares, dev4's Filter D and the screening.
//!
//! Why it matches the pieces: the energies add at every NPD distance; the geometry terms are the
//! kernel's for one segment; Delta_F of short pieces (length << d_lambda) is proportional to
//! length / d_lambda, which the energy-weighted mean length and harmonic mean d_lambda keep; the
//! centroid is emission-weighted, so the first-order error of moving the pieces to it cancels. A
//! box of one segment reads exactly as the kernel reads that segment (`boxes_tests`).

use super::box_sums::INSTALLATION_SHARE_SLANTS_M;
use super::corrections::{
    finite_segment_correction_db, installation_correction_db, lateral_attenuation_db,
};
use super::npd::{
    Installation, METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, NPD_LAST_DISTANCE_M,
    NPD_NEAREST_SLANT_M, TAIL_ANCHOR_M, curve_level_db,
};
use super::screening::{ReceiverHorizons, SCREENING_CEILING_ABOVE_GROUND_M, screened_sel_db};
use super::segment::{ClosestPoints, closest_points};
use crate::bands::PERIODS;

/// dev4 Filter D, as the kernel applies it to one segment.
const EXTENSION_BELOW_GROUND_M: f64 = 30.0;

/// A box in the receiver's frame: metres east and north of the receiver, heights above it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftBoxAtReceiver<'a> {
    pub centroid_m: [f64; 3],
    /// Horizontal axis, radians anticlockwise from east (either direction along it).
    pub axis_rad: f64,
    /// Climb (rise over horizontal run) along `axis_rad`.
    pub gradient: f64,
    /// Energy-weighted mean horizontal length of the pieces (m).
    pub piece_length_m: f64,
    /// Per period, the summed SEL energy's level at each NPD distance (dB, -inf silent).
    pub levels_db: &'a [[f64; NPD_DISTANCES]; PERIODS],
    /// The pieces' energy-weighted harmonic mean d_lambda (m) at each NPD distance.
    pub scaled_distance_m: &'a [f64; NPD_DISTANCES],
    /// Per period, the summed level at the tail anchor (dB, -inf silent).
    pub tail_levels_db: &'a [f64; PERIODS],
    /// Energy shares of wing-mounted jets, fuselage-mounted jets and propellers at 1,000 ft and
    /// at the tail anchor (propellers fall off slower than jets, so the mix changes with
    /// distance).
    pub installation_shares: [[f64; 3]; 2],
    /// The terrain the box's clearance counts from, above the receiver (m).
    pub ground_m: f64,
}

/// A box's SEL sums at the receiver per period and the terms that made them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxSel {
    /// After screening (dB per period, -inf silent).
    pub sel_db: [f64; PERIODS],
    pub closest: ClosestPoints,
    pub finite_segment_correction_db: f64,
    pub lateral_attenuation_db: f64,
    pub installation_correction_db: f64,
    pub terrain_loss_db: f64,
    pub building_loss_db: f64,
}

/// The ends of the average piece: the mean length centred on the centroid along the axis.
pub fn average_piece_ends(aircraft_box: &AircraftBoxAtReceiver) -> [[f64; 3]; 2] {
    let half = 0.5 * aircraft_box.piece_length_m.max(1.0);
    let (sin, cos) = aircraft_box.axis_rad.sin_cos();
    let step = [cos * half, sin * half, aircraft_box.gradient * half];
    let c = aircraft_box.centroid_m;
    [
        [c[0] - step[0], c[1] - step[1], c[2] - step[2]],
        [c[0] + step[0], c[1] + step[1], c[2] + step[2]],
    ]
}

/// The box's d_lambda at `slant_m`: lg d_lambda linear in lg distance through the ten distances
/// and extrapolated with the end intervals, as the kernel reads each power row.
fn scaled_distance_at(scaled_distance_m: &[f64; NPD_DISTANCES], slant_m: f64) -> f64 {
    let log_d = (slant_m.max(NPD_NEAREST_SLANT_M) / METRES_PER_FOOT).log10();
    let logs = NPD_DISTANCES_FT.map(f64::log10);
    let interval = (1..NPD_DISTANCES - 1)
        .rev()
        .find(|&k| log_d > logs[k])
        .unwrap_or(0);
    let fraction = (log_d - logs[interval]) / (logs[interval + 1] - logs[interval]);
    let (low, high) = (
        scaled_distance_m[interval].log10(),
        scaled_distance_m[interval + 1].log10(),
    );
    10f64.powf(low + fraction * (high - low))
}

/// A box's summed level at `slant_m`: its NPD curve up to 25,000 ft, beyond it spherical
/// divergence and the absorption that takes the curve through its tail anchor (a mix of rows
/// falls slower than one curve fitted to their sum).
fn box_level_db(levels: &[f64; NPD_DISTANCES], tail_db: f64, slant_m: f64) -> f64 {
    let last = levels[NPD_DISTANCES - 1];
    if slant_m < NPD_LAST_DISTANCE_M || last == f64::NEG_INFINITY {
        return curve_level_db(levels, 0.0, slant_m);
    }
    let spherical_db = |d: f64| 20.0 * (d / NPD_LAST_DISTANCE_M).log10();
    let absorption_db_per_m = ((last - spherical_db(TAIL_ANCHOR_M) - tail_db)
        / (TAIL_ANCHOR_M - NPD_LAST_DISTANCE_M))
        .max(0.0);
    last - spherical_db(slant_m) - absorption_db_per_m * (slant_m - NPD_LAST_DISTANCE_M)
}

/// The installation shares at `slant_m`: linear in lg distance between the two share slants,
/// held beyond them.
fn shares_at(shares: [[f64; 3]; 2], slant_m: f64) -> [f64; 3] {
    let [near, far] = INSTALLATION_SHARE_SLANTS_M;
    let t = ((slant_m / near).log10() / (far / near).log10()).clamp(0.0, 1.0);
    std::array::from_fn(|k| shares[0][k] + t * (shares[1][k] - shares[0][k]))
}

/// The box's SEL sums at the receiver, or `None` when dev4's Filter D rejects its average piece.
pub fn box_sel_at_receiver(
    aircraft_box: &AircraftBoxAtReceiver,
    horizons: &impl ReceiverHorizons,
) -> Option<BoxSel> {
    let [start, end] = average_piece_ends(aircraft_box);
    let closest = closest_points(start, end);
    let [east_m, north_m, height_m] = closest.on_line_m;
    let below_extension = height_m < aircraft_box.ground_m - EXTENSION_BELOW_GROUND_M;
    if below_extension && !(0.0..=1.0).contains(&closest.along) {
        return None;
    }
    let lateral_m = east_m.hypot(north_m);
    let slant_m = lateral_m.hypot(height_m);
    let finite = finite_segment_correction_db(
        closest.along * closest.horizontal_length_m,
        closest.horizontal_length_m,
        scaled_distance_at(aircraft_box.scaled_distance_m, slant_m),
    );
    let lateral_attenuation = lateral_attenuation_db(height_m, lateral_m);
    let installations = [
        Installation::Wing,
        Installation::Fuselage,
        Installation::Propeller,
    ];
    let installation_energy: f64 = installations
        .iter()
        .zip(shares_at(aircraft_box.installation_shares, slant_m))
        .map(|(&installation, share)| {
            share * 10f64.powf(installation_correction_db(installation, height_m, slant_m) / 10.0)
        })
        .sum();
    let installation = if installation_energy > 0.0 {
        10.0 * installation_energy.log10()
    } else {
        0.0
    };
    let height_above_ground_m = aircraft_box.centroid_m[2] - aircraft_box.ground_m;
    let (terrain_loss_db, building_loss_db) =
        if height_above_ground_m < SCREENING_CEILING_ABOVE_GROUND_M {
            (
                horizons.terrain_loss_db(closest.on_line_m),
                horizons.building_loss_db(closest.on_segment_m),
            )
        } else {
            (0.0, 0.0)
        };
    let sel_db: [f64; PERIODS] = std::array::from_fn(|period| {
        let levels = &aircraft_box.levels_db[period];
        if levels.iter().all(|level| *level == f64::NEG_INFINITY) {
            return f64::NEG_INFINITY;
        }
        let free = box_level_db(levels, aircraft_box.tail_levels_db[period], slant_m)
            + finite
            + installation
            - lateral_attenuation;
        screened_sel_db(free, lateral_attenuation, terrain_loss_db, building_loss_db)
    });
    Some(BoxSel {
        sel_db,
        closest,
        finite_segment_correction_db: finite,
        lateral_attenuation_db: lateral_attenuation,
        installation_correction_db: installation,
        terrain_loss_db,
        building_loss_db,
    })
}

#[cfg(test)]
#[path = "boxes_tests.rs"]
mod tests;
