//! Aircraft box SEL as linear period energies from its average pieces through shared Doc 29 corrections.

use super::box_sums::INSTALLATION_SHARE_SLANTS_M;
use super::corrections::{
    finite_segment_energy_fraction, installation_energy_gain, lateral_attenuation_db,
};
use super::npd::{Installation, NPD_DISTANCES, NPD_LAST_DISTANCE_M, NpdPosition, TAIL_ANCHOR_M};
use super::screening::{ReceiverHorizons, SCREENING_CEILING_ABOVE_GROUND_M, screened_sel_db};
use super::segment::{ClosestPoints, closest_points};
use crate::bands::{PERIODS, energy};

/// The NPD distance (1,000 ft) whose energy weighs the periods' piece lengths into one.
const GEOMETRY_DISTANCE: usize = 3;

/// A box in the receiver's frame: metres east and north of the receiver, heights above it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AircraftBoxAtReceiver<'a> {
    pub centroid_m: [f64; 3],
    /// Horizontal axis, radians anticlockwise from east (either direction along it).
    pub axis_rad: f64,
    /// Climb (rise over horizontal run) along `axis_rad`.
    pub gradient: f64,
    /// The spread of the pieces' gradients about it.
    pub gradient_spread: f64,
    /// Per period, the energy-weighted mean horizontal length of the pieces (m).
    pub piece_length_m: [f64; PERIODS],
    /// Per period, the summed SEL energy's level at each NPD distance (dB, -inf silent).
    pub levels_db: &'a [[f64; NPD_DISTANCES]; PERIODS],
    /// lg of the pieces' energy-weighted harmonic mean d_lambda (m) at each NPD distance.
    pub lg_scaled_distance: &'a [f64; NPD_DISTANCES],
    /// Per period, the summed level at the tail anchor (dB, -inf silent).
    pub tail_levels_db: &'a [f64; PERIODS],
    /// Per period, the energy shares of wing-mounted jets, fuselage-mounted jets and propellers
    /// at 1,000 ft and at the tail anchor (propellers fall off slower than jets, so the mix
    /// changes with distance; jets by day and propellers by night are two mixes).
    pub installation_shares: [[[f64; 3]; 2]; PERIODS],
    /// The terrain the box's clearance counts from, above the receiver (m).
    pub ground_m: f64,
}

/// A box's SEL energies at the receiver per period and the terms that made them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxSel {
    /// After screening (linear energy per period, zero silent).
    pub sel_energy: [f64; PERIODS],
    pub closest: ClosestPoints,
    pub closest_slant_m: f64,
    /// Per period (each with its mean piece length).
    pub finite_segment_energy_fraction: [f64; PERIODS],
    pub lateral_attenuation_db: f64,
    /// Per period, the energy gain of its installation mix.
    pub installation_energy_gain: [f64; PERIODS],
    /// Per period, the received energy's shares of wing-mounted jets, fuselage-mounted jets and
    /// propellers.
    pub installation_fractions: [[f64; 3]; PERIODS],
    pub terrain_loss_db: f64,
    pub building_loss_db: f64,
}

/// A lower bound on the closest-line slant for receivers within `half_extent_m` of the origin.
pub fn minimum_line_slant_m(
    centroid_m: [f64; 3],
    axis_rad: f64,
    gradient: f64,
    half_extent_m: [f64; 3],
) -> f64 {
    let (sin, cos) = axis_rad.sin_cos();
    let [east, north, up] = centroid_m;
    let [east_extent, north_extent, up_extent] = half_extent_m;
    let across = -sin * east + cos * north;
    let across_extent = sin.abs() * east_extent + cos.abs() * north_extent;
    let stretch = gradient.hypot(1.0);
    let vertical = (up - gradient * (cos * east + sin * north)) / stretch;
    let vertical_extent = (up_extent
        + gradient.abs() * (cos.abs() * east_extent + sin.abs() * north_extent))
        / stretch;
    (across.abs() - across_extent)
        .max(0.0)
        .hypot((vertical.abs() - vertical_extent).max(0.0))
}

/// The average piece's length (m): the periods' lengths weighted by their energy at 1,000 ft.
fn average_length_m(aircraft_box: &AircraftBoxAtReceiver) -> f64 {
    let weights = aircraft_box
        .levels_db
        .map(|levels| energy(levels[GEOMETRY_DISTANCE]));
    let total: f64 = weights.iter().sum();
    if total > 0.0 {
        (0..PERIODS)
            .map(|period| weights[period] * aircraft_box.piece_length_m[period])
            .sum::<f64>()
            / total
    } else {
        aircraft_box.piece_length_m[0]
    }
}

/// The ends of the average piece: the mean length centred on the centroid along the axis.
fn average_piece_ends(aircraft_box: &AircraftBoxAtReceiver, length_m: f64) -> [[f64; 3]; 2] {
    let half = 0.5 * length_m.max(1.0);
    let (sin, cos) = aircraft_box.axis_rad.sin_cos();
    let step = [cos * half, sin * half, aircraft_box.gradient * half];
    let c = aircraft_box.centroid_m;
    [
        [c[0] - step[0], c[1] - step[1], c[2] - step[2]],
        [c[0] + step[0], c[1] + step[1], c[2] + step[2]],
    ]
}

/// The box's d_lambda at a slant: lg d_lambda linear in lg distance through the ten distances
/// and extrapolated with the end intervals, as the kernel reads each power row.
fn scaled_distance_at(lg_scaled_distance: &[f64; NPD_DISTANCES], position: &NpdPosition) -> f64 {
    (position.linear(lg_scaled_distance) * std::f64::consts::LN_10).exp()
}

/// A box's d_lambda at `slant_m` (m).
pub fn scaled_distance_at_slant(lg_scaled_distance: &[f64; NPD_DISTANCES], slant_m: f64) -> f64 {
    scaled_distance_at(lg_scaled_distance, &NpdPosition::at(slant_m))
}

/// A box's summed level at `slant_m`: its NPD curve up to 25,000 ft, beyond it spherical
/// divergence and the absorption that takes the curve through its tail anchor (a mix of rows
/// falls slower than one curve fitted to their sum).
fn box_level_db(levels: &[f64; NPD_DISTANCES], tail_db: f64, position: &NpdPosition) -> f64 {
    let (last, slant_m) = (levels[NPD_DISTANCES - 1], position.slant_m);
    if slant_m < NPD_LAST_DISTANCE_M {
        return position.linear(levels);
    }
    if last == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    let spherical_db = |d: f64| 20.0 * (d / NPD_LAST_DISTANCE_M).log10();
    let absorption_db_per_m = ((last - spherical_db(TAIL_ANCHOR_M) - tail_db)
        / (TAIL_ANCHOR_M - NPD_LAST_DISTANCE_M))
        .max(0.0);
    last - spherical_db(slant_m) - absorption_db_per_m * (slant_m - NPD_LAST_DISTANCE_M)
}

/// The installation shares at the clamped fraction between their two reference slants.
fn shares_at(shares: [[f64; 3]; 2], fraction: f64) -> [f64; 3] {
    std::array::from_fn(|k| shares[0][k] + fraction * (shares[1][k] - shares[0][k]))
}

/// The box's SEL sums at the receiver: its two average pieces, or one without a spread (the terms
/// besides the sums are the lower piece's).
pub fn box_sel_at_receiver(
    aircraft_box: &AircraftBoxAtReceiver,
    horizons: &impl ReceiverHorizons,
) -> BoxSel {
    let length_m = average_length_m(aircraft_box);
    let spread = aircraft_box.gradient_spread;
    if spread <= 0.0 {
        return average_piece_sel(aircraft_box, horizons, length_m);
    }
    let at = |gradient: f64| {
        let piece = AircraftBoxAtReceiver {
            gradient,
            ..*aircraft_box
        };
        average_piece_sel(&piece, horizons, length_m)
    };
    let (lower, upper) = (
        at(aircraft_box.gradient - spread),
        at(aircraft_box.gradient + spread),
    );
    let sel_energy =
        std::array::from_fn(|period| 0.5 * (lower.sel_energy[period] + upper.sel_energy[period]));
    // Each period's two installation mixes, weighed by what each piece brings to the receiver.
    let installation_fractions = std::array::from_fn(|period| {
        let [lower_energy, upper_energy] = [lower.sel_energy[period], upper.sel_energy[period]];
        if lower_energy + upper_energy > 0.0 {
            std::array::from_fn(|k| {
                (lower_energy * lower.installation_fractions[period][k]
                    + upper_energy * upper.installation_fractions[period][k])
                    / (lower_energy + upper_energy)
            })
        } else {
            lower.installation_fractions[period]
        }
    });
    BoxSel {
        sel_energy,
        installation_fractions,
        ..lower
    }
}

/// The SEL sums of one average piece at the receiver.
fn average_piece_sel(
    aircraft_box: &AircraftBoxAtReceiver,
    horizons: &impl ReceiverHorizons,
    length_m: f64,
) -> BoxSel {
    let [start, end] = average_piece_ends(aircraft_box, length_m);
    let closest = closest_points(start, end);
    let [east_m, north_m, up_m] = closest.on_line_m;
    let slant_m = east_m.hypot(north_m).hypot(up_m);
    let (height_m, lateral_m) = (closest.height_m, closest.lateral_m);
    let position = NpdPosition::at(slant_m);
    // Each period's pieces keep their own mean length (stored horizontal, taken along the average
    // piece's gradient), centred where the average piece is.
    let scaled_distance_m = scaled_distance_at(aircraft_box.lg_scaled_distance, &position);
    let along_m = closest.along * closest.length_m;
    let stretch = aircraft_box.gradient.hypot(1.0);
    let finite: [f64; PERIODS] = std::array::from_fn(|period| {
        let length_m = (aircraft_box.piece_length_m[period] * stretch).max(1.0);
        finite_segment_energy_fraction(
            along_m + 0.5 * (length_m - closest.length_m),
            length_m,
            scaled_distance_m,
        )
    });
    let lateral_attenuation = lateral_attenuation_db(closest.lambda_height_m, lateral_m);
    let installations = [
        Installation::Wing,
        Installation::Fuselage,
        Installation::Propeller,
    ];
    let corrections =
        installations.map(|installation| installation_energy_gain(installation, height_m, slant_m));
    let [near, far] = INSTALLATION_SHARE_SLANTS_M;
    let fraction = ((slant_m / near).log10() / (far / near).log10()).clamp(0.0, 1.0);
    let mixes = aircraft_box.installation_shares.map(|shares| {
        let shares = shares_at(shares, fraction);
        let parts: [f64; 3] = std::array::from_fn(|k| shares[k] * corrections[k]);
        let installation_energy: f64 = parts.iter().sum();
        if installation_energy > 0.0 {
            (
                installation_energy,
                parts.map(|part| part / installation_energy),
            )
        } else {
            (1.0, shares)
        }
    });
    let installation = mixes.map(|(correction, _)| correction);
    let installation_fractions = mixes.map(|(_, fractions)| fractions);
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
    let attenuation_gain = energy(
        screened_sel_db(0.0, lateral_attenuation, terrain_loss_db, building_loss_db)
            - lateral_attenuation,
    );
    let sel_energy: [f64; PERIODS] = std::array::from_fn(|period| {
        if aircraft_box.levels_db[period]
            .iter()
            .all(|level| *level == f64::NEG_INFINITY)
        {
            return 0.0;
        }
        energy(box_level_db(
            &aircraft_box.levels_db[period],
            aircraft_box.tail_levels_db[period],
            &position,
        )) * finite[period]
            * installation[period]
            * attenuation_gain
    });
    BoxSel {
        sel_energy,
        closest,
        closest_slant_m: slant_m,
        finite_segment_energy_fraction: finite,
        lateral_attenuation_db: lateral_attenuation,
        installation_energy_gain: installation,
        installation_fractions,
        terrain_loss_db,
        building_loss_db,
    }
}

#[cfg(test)]
#[path = "boxes_tests.rs"]
mod tests;
