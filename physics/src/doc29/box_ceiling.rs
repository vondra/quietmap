//! A ceiling of an aircraft box's SEL energy at any receiver of a box of receivers: what a sum may
//! leave out of its boxes.
//!
//! [`super::boxes::box_sel_at_receiver`] reads a box's level and d_lambda at the slant to its
//! average piece's line, times each period's piece's finite-segment fraction, its installation
//! correction, and the screening and lateral attenuation, of which the larger is taken. Over every
//! receiver within `half_extent_m` of the origin:
//!
//! - the slant runs from the nearest the region allows to its farthest from the centroid, which the
//!   line passes through; it is taken in intervals of [`INTERVAL_RATIO`] up to the gap between the
//!   region and the piece, and one beyond. In one the level is at most the curve's largest from
//!   the interval's near end out, and the fraction, `(1/pi)` times
//!   the integral of `2 / (1 + a^2)^2` over the piece's span `l / d_lambda` (Eq. 4-12), at most
//!   `(2/pi) (l / d_lambda) / (1 + q^2 / d_lambda^2)^2`, `q` the receiver's offset along the line
//!   from its closest point to the piece, which the gap between the region and the piece bounds
//!   (`q^2 >= gap^2 - slant^2`);
//! - the lateral attenuation, larger at a lower elevation angle and a longer lateral distance
//!   (Eqs. 4-17, 4-18), is at least its value at the region's steepest angle and least distance;
//! - the installation correction, which rises with the elevation's sin^2 to beyond what a ground
//!   receiver meets (an airliner's to 45 deg), is at most its largest up to the region's steepest.

use super::boxes::{AircraftBoxAtReceiver, LineOffsets, average_length_m, box_level_db};
use super::corrections::{installation_energy_gain, lateral_attenuation_db};
use super::npd::{Installation, NPD_DISTANCES, NPD_LAST_DISTANCE_M, NpdPosition};
use crate::bands::{PERIODS, energy};
use std::f64::consts::{FRAC_2_PI, LN_10};
use std::sync::LazyLock;

/// Ratio of an interval's far slant to its near one below the gap: what the offset along the line
/// is bounded in steps of.
const INTERVAL_RATIO: f64 = 1.1;

/// Steps of sin^2 the installation's ceiling is tabulated in.
const SINE_SQUARED_STEPS: usize = 256;

/// Per installation and step `k`, the largest its correction takes at any sin^2 up to
/// `k / SINE_SQUARED_STEPS`: a scan in 64 parts of each step, a percent above for what lies between
/// them (the corrections are smooth in sin^2).
static INSTALLATION_UP_TO: LazyLock<[[f64; SINE_SQUARED_STEPS + 1]; 3]> = LazyLock::new(|| {
    [
        Installation::Wing,
        Installation::Fuselage,
        Installation::Propeller,
    ]
    .map(|installation| {
        let gain =
            |sine_squared: f64| installation_energy_gain(installation, sine_squared.sqrt(), 1.0);
        let mut table = [gain(0.0) * 1.01; SINE_SQUARED_STEPS + 1];
        for step in 1..=SINE_SQUARED_STEPS {
            let largest = (0..=64)
                .map(|part| {
                    gain((step as f64 - 1.0 + part as f64 / 64.0) / SINE_SQUARED_STEPS as f64)
                })
                .fold(0.0, f64::max);
            table[step] = table[step - 1].max(largest * 1.01);
        }
        table
    })
});

/// The farthest any receiver within `half_extent_m` of the origin is from `centroid_m` (m), which
/// bounds its slant to any line through the centroid.
pub fn farthest_centroid_slant_m(centroid_m: [f64; 3], half_extent_m: [f64; 3]) -> f64 {
    let [east, north, up] =
        std::array::from_fn(|axis| centroid_m[axis].abs() + half_extent_m[axis]);
    east.hypot(north).hypot(up)
}

/// A ceiling of `aircraft_box`'s SEL energy per period, as [`super::boxes::box_sel_at_receiver`]
/// gives it, at any receiver within `half_extent_m` of the origin (the box's coordinates taken
/// from the region's middle), screened or not.
pub fn box_sel_ceiling(
    aircraft_box: &AircraftBoxAtReceiver,
    half_extent_m: [f64; 3],
) -> [f64; PERIODS] {
    let average_length_m = average_length_m(aircraft_box.levels_db, &aircraft_box.piece_length_m);
    let piece =
        |gradient: f64| piece_ceiling(aircraft_box, gradient, half_extent_m, average_length_m);
    let spread = aircraft_box.gradient_spread;
    if spread <= 0.0 {
        return piece(aircraft_box.gradient);
    }
    let (lower, upper) = (
        piece(aircraft_box.gradient - spread),
        piece(aircraft_box.gradient + spread),
    );
    std::array::from_fn(|period| 0.5 * (lower[period] + upper[period]))
}

/// One period's curve: its levels' largest from each NPD distance out, its piece's length (m) and
/// the squared gap between the region and the piece.
struct PeriodPiece {
    largest_from: [f64; NPD_DISTANCES],
    length_m: f64,
    gap_squared_m2: f64,
}

/// The ceiling of the average piece at `gradient` per period.
fn piece_ceiling(
    aircraft_box: &AircraftBoxAtReceiver,
    gradient: f64,
    half_extent_m: [f64; 3],
    average_length_m: f64,
) -> [f64; PERIODS] {
    let centroid_m = aircraft_box.centroid_m;
    let stretch = gradient.hypot(1.0);
    let (sin, cos) = aircraft_box.axis_rad.sin_cos();
    let unit = [cos / stretch, sin / stretch, gradient / stretch];
    let pieces: [Option<PeriodPiece>; PERIODS] = std::array::from_fn(|period| {
        let levels = &aircraft_box.levels_db[period];
        if levels.iter().all(|level| *level == f64::NEG_INFINITY) {
            return None;
        }
        let mut largest_from = *levels;
        for k in (0..NPD_DISTANCES - 1).rev() {
            largest_from[k] = largest_from[k].max(largest_from[k + 1]);
        }
        // The period's piece, as long as `box_sel_at_receiver` takes it and centred on the
        // centroid, and the gap between its bounding box and the region's.
        let length_m = (aircraft_box.piece_length_m[period] * stretch).max(1.0);
        let gap_squared_m2 = (0..3)
            .map(|axis| {
                let reach = 0.5 * length_m * unit[axis].abs();
                let gap = (centroid_m[axis].abs() - reach - half_extent_m[axis]).max(0.0);
                gap * gap
            })
            .sum();
        Some(PeriodPiece {
            largest_from,
            length_m,
            gap_squared_m2,
        })
    });
    let offsets = LineOffsets::of(centroid_m, aircraft_box.axis_rad, gradient, half_extent_m);
    let farthest_m = farthest_centroid_slant_m(centroid_m, half_extent_m);
    // Beyond the widest gap no interval bounds the offset along the line: one takes the rest.
    let widest_gap_m = (pieces.iter().flatten())
        .map(|piece| piece.gap_squared_m2)
        .fold(0.0, f64::max)
        .sqrt();
    let mut ceiling = [0.0; PERIODS];
    // Slants below the NPD curves' nearest read as it.
    let mut near = NpdPosition::at(offsets.least_slant_m());
    let farthest_m = farthest_m.max(near.slant_m);
    loop {
        let far_m = match near.slant_m < widest_gap_m {
            true => (near.slant_m * INTERVAL_RATIO).min(widest_gap_m),
            false => farthest_m,
        };
        let far = NpdPosition::at(far_m.min(farthest_m));
        let lg_scaled = aircraft_box.lg_scaled_distance;
        let (lowest, highest) = (near.interval + 1..=far.interval)
            .map(|k| lg_scaled[k])
            .chain([near.linear(lg_scaled), far.linear(lg_scaled)])
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), lg| {
                (low.min(lg), high.max(lg))
            });
        let (shortest_m, longest_m) = ((lowest * LN_10).exp(), (highest * LN_10).exp());
        for (period, piece) in pieces.iter().enumerate() {
            let Some(piece) = piece else { continue };
            let level_db = largest_level_from(aircraft_box, period, &piece.largest_from, &near);
            let fraction = if shortest_m < 1.0 {
                1.0
            } else {
                let offset_squared_m2 = (piece.gap_squared_m2 - far.slant_m * far.slant_m).max(0.0);
                let spread = 1.0 + offset_squared_m2 / (longest_m * longest_m);
                (FRAC_2_PI * piece.length_m / shortest_m / (spread * spread)).min(1.0)
            };
            ceiling[period] = f64::max(ceiling[period], energy(level_db) * fraction.max(1e-15));
        }
        if far.slant_m >= farthest_m {
            break;
        }
        near = far;
    }
    let half_rise_m = 0.5 * average_length_m.max(1.0) * gradient.abs();
    let corrections = corrections_ceiling(
        aircraft_box,
        gradient,
        &offsets,
        half_extent_m[2] + half_rise_m,
    );
    std::array::from_fn(|period| ceiling[period] * corrections[period])
}

/// Per period, the installation correction at most times the lateral attenuation's gain at most.
/// The attenuation's height is the closest point's alongside the piece, else the nearer end's
/// above the receiver along the climb: at most the centroid's highest above a receiver plus half
/// the average piece's rise (`end_reach_m` the two beyond the centroid's own height). The shares
/// run linearly between the period's two mixes, so the correction is at most either mix on each
/// installation's largest (1 for a mix without energy, as it reads).
fn corrections_ceiling(
    aircraft_box: &AircraftBoxAtReceiver,
    gradient: f64,
    offsets: &LineOffsets,
    end_reach_m: f64,
) -> [f64; PERIODS] {
    let (across_m, vertical_m) = (offsets.least_across_m(), offsets.largest_vertical_m());
    let end_height_m = (aircraft_box.centroid_m[2].abs() + end_reach_m) * gradient.hypot(1.0);
    let attenuation = energy(-lateral_attenuation_db(
        vertical_m.max(end_height_m),
        across_m,
    ));
    let sine_squared = if across_m > 0.0 {
        vertical_m * vertical_m / (vertical_m * vertical_m + across_m * across_m)
    } else {
        1.0
    };
    let step = ((sine_squared * SINE_SQUARED_STEPS as f64).ceil() as usize).min(SINE_SQUARED_STEPS);
    let installation: [f64; 3] = std::array::from_fn(|k| INSTALLATION_UP_TO[k][step]);
    aircraft_box.installation_shares.map(|mixes| {
        let correction = (mixes.iter())
            .map(|shares| match shares.iter().sum::<f64>() > 0.0 {
                true => (0..3)
                    .map(|k| shares[k].max(0.0) * installation[k])
                    .sum::<f64>(),
                false => 1.0,
            })
            .fold(0.0, f64::max);
        correction * attenuation
    })
}

/// A period's level at most at any slant from `position` out: there or at a farther NPD distance
/// (linear in log distance between them, falling past the last).
fn largest_level_from(
    aircraft_box: &AircraftBoxAtReceiver,
    period: usize,
    largest_from: &[f64; NPD_DISTANCES],
    position: &NpdPosition,
) -> f64 {
    let levels = &aircraft_box.levels_db[period];
    let at = box_level_db(levels, aircraft_box.tail_levels_db[period], position);
    if position.slant_m >= NPD_LAST_DISTANCE_M {
        return at;
    }
    at.max(largest_from[position.interval + 1])
}

#[cfg(test)]
#[path = "box_ceiling_tests.rs"]
mod tests;
