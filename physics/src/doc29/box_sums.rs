//! The builder's side of the click-time equation: the sums a box keeps of the pieces that crossed
//! it and the box values they give ([`super::boxes`] reads them at a click). Every sum is a plain
//! sum, so a box is independent of the order and the day its pieces arrive in.

use super::npd::{Installation, NPD_DISTANCES};
use super::segment::NpdDistanceLevels;
use crate::bands::PERIODS;

/// The NPD distance whose energy weighs a piece's geometry (1,000 ft): near enough for the
/// geometry that is heard, far enough for every class to be on its curve.
const GEOMETRY_WEIGHT_DISTANCE: usize = 3;

/// The running sums of one box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoxSums {
    /// Per period and NPD distance, the pieces' weighted energy.
    energy: [[f64; NPD_DISTANCES]; PERIODS],
    /// Per NPD distance, the pieces' energy (all periods) over their d_lambda.
    energy_over_scaled_distance: [f64; NPD_DISTANCES],
    /// Geometry weights: the pieces' energy at the weight distance, all periods.
    weight: f64,
    weighted_midpoint_m: [f64; 3],
    weighted_length_m: f64,
    /// Doubled-angle direction sums of the axis, and the gradient carried on each piece's own
    /// horizontal direction (its sign follows the direction the axis ends up pointing).
    weighted_doubled_direction: [f64; 2],
    weighted_gradient_direction: [f64; 2],
    weighted_installation: [f64; 3],
    pieces: u64,
}

/// What a finished box stores.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxValues {
    pub levels_db: [[f64; NPD_DISTANCES]; PERIODS],
    pub scaled_distance_m: [f64; NPD_DISTANCES],
    pub centroid_m: [f64; 3],
    pub axis_rad: f64,
    pub gradient: f64,
    pub piece_length_m: f64,
    pub installation_shares: [f64; 3],
}

impl BoxSums {
    /// Adds one piece from `start_m` to `end_m` (metres in any frame shared by the box's pieces,
    /// heights in metres), with its levels at the NPD distances and its installation, weighted
    /// per period by `period_weights` (the provider weights of its day, 0 outside its period).
    pub fn add(
        &mut self,
        levels: &NpdDistanceLevels,
        installation: Installation,
        period_weights: [f64; PERIODS],
        start_m: [f64; 3],
        end_m: [f64; 3],
    ) {
        let unweighted: [f64; NPD_DISTANCES] = levels.sel_db.map(|level| 10f64.powf(level / 10.0));
        let total_weight: f64 = period_weights.iter().sum();
        if total_weight <= 0.0 {
            return;
        }
        for (period, weight) in period_weights.iter().enumerate() {
            for (sum, energy) in self.energy[period].iter_mut().zip(unweighted) {
                *sum += weight * energy;
            }
        }
        for (k, sum) in self.energy_over_scaled_distance.iter_mut().enumerate() {
            *sum += total_weight * unweighted[k] / levels.scaled_distance_m[k].max(1.0);
        }
        let weight = total_weight * unweighted[GEOMETRY_WEIGHT_DISTANCE];
        let delta = [0, 1, 2].map(|axis| end_m[axis] - start_m[axis]);
        let length_m = delta[0].hypot(delta[1]);
        self.weight += weight;
        for axis in 0..3 {
            self.weighted_midpoint_m[axis] += weight * 0.5 * (start_m[axis] + end_m[axis]);
        }
        self.weighted_length_m += weight * length_m;
        if length_m > 0.0 {
            let (cos, sin) = (delta[0] / length_m, delta[1] / length_m);
            self.weighted_doubled_direction[0] += weight * (cos * cos - sin * sin);
            self.weighted_doubled_direction[1] += weight * 2.0 * sin * cos;
            let gradient = delta[2] / length_m;
            self.weighted_gradient_direction[0] += weight * gradient * cos;
            self.weighted_gradient_direction[1] += weight * gradient * sin;
        }
        let slot = match installation {
            Installation::Wing => 0,
            Installation::Fuselage => 1,
            Installation::Propeller => 2,
        };
        self.weighted_installation[slot] += weight;
        self.pieces += 1;
    }

    /// Pieces added so far.
    pub fn pieces(&self) -> u64 {
        self.pieces
    }

    /// The box's values; `None` for a box without energy.
    pub fn values(&self) -> Option<BoxValues> {
        if self.weight <= 0.0 {
            return None;
        }
        let levels_db = self.energy.map(|energies| {
            energies.map(|energy| {
                if energy > 0.0 {
                    10.0 * energy.log10()
                } else {
                    f64::NEG_INFINITY
                }
            })
        });
        let scaled_distance_m = std::array::from_fn(|k| {
            let energy: f64 = (0..PERIODS).map(|period| self.energy[period][k]).sum();
            energy / self.energy_over_scaled_distance[k]
        });
        let axis_rad = (0.5
            * self.weighted_doubled_direction[1].atan2(self.weighted_doubled_direction[0]))
        .rem_euclid(std::f64::consts::PI);
        let (sin, cos) = axis_rad.sin_cos();
        let gradient = (self.weighted_gradient_direction[0] * cos
            + self.weighted_gradient_direction[1] * sin)
            / self.weight;
        Some(BoxValues {
            levels_db,
            scaled_distance_m,
            centroid_m: self.weighted_midpoint_m.map(|sum| sum / self.weight),
            axis_rad,
            gradient,
            piece_length_m: self.weighted_length_m / self.weight,
            installation_shares: self.weighted_installation.map(|sum| sum / self.weight),
        })
    }
}
