//! The builder's side of the click-time equation: the sums a box keeps of the pieces that crossed
//! it and the box values they give ([`super::boxes`] reads them at a click). Every sum is a plain
//! sum, so a box is independent of the order and the day its pieces arrive in.

use super::npd::{Installation, METRES_PER_FOOT, NPD_DISTANCES, TAIL_ANCHOR_M};
use super::segment::NpdDistanceLevels;
use crate::bands::PERIODS;

/// The NPD distance whose energy weighs a piece's geometry (1,000 ft): near enough for the
/// geometry that is heard, far enough for every class to be on its curve.
const GEOMETRY_WEIGHT_DISTANCE: usize = 3;
/// The slants the installation shares are stated at (m): 1,000 ft and the tail anchor. The mix
/// changes with distance, propellers falling off slower than jets.
pub const INSTALLATION_SHARE_SLANTS_M: [f64; 2] = [1_000.0 * METRES_PER_FOOT, TAIL_ANCHOR_M];

/// The running sums of one box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoxSums {
    /// Per period and NPD distance, the pieces' weighted energy, and per period at the tail
    /// anchor.
    energy: [[f64; NPD_DISTANCES]; PERIODS],
    tail_energy: [f64; PERIODS],
    /// Per NPD distance, the pieces' energy (all periods) over their d_lambda.
    energy_over_scaled_distance: [f64; NPD_DISTANCES],
    /// Geometry weights: the pieces' energy at the weight distance, all periods.
    weight: f64,
    weighted_midpoint_m: [f64; 3],
    /// Per period, the same weights and the pieces' horizontal length under them: a period's
    /// pieces may be longer than another's in the same box (a piece's energy grows with it).
    period_weight: [f64; PERIODS],
    period_weighted_length_m: [f64; PERIODS],
    /// Doubled-angle direction sums of the axis, and the gradient carried on each piece's own
    /// horizontal direction (its sign follows the direction the axis ends up pointing).
    weighted_doubled_direction: [f64; 2],
    weighted_gradient_direction: [f64; 2],
    /// The squared gradients on the doubled-angle terms (cos^2, sin^2, sin cos of each piece's
    /// direction): the spread of the gradients along the axis, once it is known.
    weighted_squared_gradient: [f64; 3],
    /// Per period and share slant, the pieces' energy by installation.
    installation_energy: [[[f64; 3]; 2]; PERIODS],
    pieces: u64,
}

/// What a finished box stores.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxValues {
    pub levels_db: [[f64; NPD_DISTANCES]; PERIODS],
    /// Per period, the summed level at the tail anchor.
    pub tail_levels_db: [f64; PERIODS],
    pub scaled_distance_m: [f64; NPD_DISTANCES],
    pub centroid_m: [f64; 3],
    pub axis_rad: f64,
    pub gradient: f64,
    /// The spread (standard deviation) of the pieces' gradients along the axis about it.
    pub gradient_spread: f64,
    /// Per period, the energy-weighted mean horizontal length of the pieces.
    pub piece_length_m: [f64; PERIODS],
    /// Per period, the energy shares of the installations at each of
    /// [`INSTALLATION_SHARE_SLANTS_M`] (a period without pieces the whole day's).
    pub installation_shares: [[[f64; 3]; 2]; PERIODS],
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
        let tail = 10f64.powf(levels.tail_sel_db / 10.0);
        for (period, weight) in period_weights.iter().enumerate() {
            for (sum, energy) in self.energy[period].iter_mut().zip(unweighted) {
                *sum += weight * energy;
            }
            self.tail_energy[period] += weight * tail;
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
        for (period, period_weight) in period_weights.iter().enumerate() {
            let piece_weight = period_weight * unweighted[GEOMETRY_WEIGHT_DISTANCE];
            self.period_weight[period] += piece_weight;
            self.period_weighted_length_m[period] += piece_weight * length_m;
        }
        if length_m > 0.0 {
            let (cos, sin) = (delta[0] / length_m, delta[1] / length_m);
            self.weighted_doubled_direction[0] += weight * (cos * cos - sin * sin);
            self.weighted_doubled_direction[1] += weight * 2.0 * sin * cos;
            let gradient = delta[2] / length_m;
            self.weighted_gradient_direction[0] += weight * gradient * cos;
            self.weighted_gradient_direction[1] += weight * gradient * sin;
            let squared = weight * gradient * gradient;
            self.weighted_squared_gradient[0] += squared * cos * cos;
            self.weighted_squared_gradient[1] += squared * sin * sin;
            self.weighted_squared_gradient[2] += squared * sin * cos;
        }
        let slot = match installation {
            Installation::Wing => 0,
            Installation::Fuselage => 1,
            Installation::Propeller => 2,
        };
        for (period, weight) in period_weights.iter().enumerate() {
            self.installation_energy[period][0][slot] +=
                weight * unweighted[GEOMETRY_WEIGHT_DISTANCE];
            self.installation_energy[period][1][slot] += weight * tail;
        }
        self.pieces += 1;
    }

    /// Adds another box's sums (a partial sum from another thread or day).
    pub fn merge(&mut self, other: &BoxSums) {
        for (sums, others) in self.energy.iter_mut().zip(&other.energy) {
            for (sum, value) in sums.iter_mut().zip(others) {
                *sum += value;
            }
        }
        for (sum, value) in self.tail_energy.iter_mut().zip(other.tail_energy) {
            *sum += value;
        }
        for (sum, value) in self
            .energy_over_scaled_distance
            .iter_mut()
            .zip(other.energy_over_scaled_distance)
        {
            *sum += value;
        }
        self.weight += other.weight;
        for axis in 0..3 {
            self.weighted_midpoint_m[axis] += other.weighted_midpoint_m[axis];
        }
        for period in 0..PERIODS {
            self.period_weight[period] += other.period_weight[period];
            self.period_weighted_length_m[period] += other.period_weighted_length_m[period];
        }
        for axis in 0..2 {
            self.weighted_doubled_direction[axis] += other.weighted_doubled_direction[axis];
            self.weighted_gradient_direction[axis] += other.weighted_gradient_direction[axis];
        }
        for (sum, value) in self
            .weighted_squared_gradient
            .iter_mut()
            .zip(other.weighted_squared_gradient)
        {
            *sum += value;
        }
        for (sums, values) in self
            .installation_energy
            .iter_mut()
            .flatten()
            .flatten()
            .zip(other.installation_energy.iter().flatten().flatten())
        {
            *sums += values;
        }
        self.pieces += other.pieces;
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
        let level = |energy: f64| {
            if energy > 0.0 {
                10.0 * energy.log10()
            } else {
                f64::NEG_INFINITY
            }
        };
        let levels_db = self.energy.map(|energies| energies.map(level));
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
        let [cos_cos, sin_sin, sin_cos] = self.weighted_squared_gradient;
        let mean_square =
            (cos_cos * cos * cos + sin_sin * sin * sin + 2.0 * sin_cos * sin * cos) / self.weight;
        Some(BoxValues {
            levels_db,
            tail_levels_db: self.tail_energy.map(level),
            scaled_distance_m,
            centroid_m: self.weighted_midpoint_m.map(|sum| sum / self.weight),
            axis_rad,
            gradient,
            gradient_spread: (mean_square - gradient * gradient).max(0.0).sqrt(),
            piece_length_m: {
                let (weights, lengths) = (
                    self.period_weight.iter().sum::<f64>(),
                    self.period_weighted_length_m.iter().sum::<f64>(),
                );
                std::array::from_fn(|period| {
                    if self.period_weight[period] > 0.0 {
                        self.period_weighted_length_m[period] / self.period_weight[period]
                    } else {
                        lengths / weights
                    }
                })
            },
            installation_shares: {
                let shares = |energies: [f64; 3]| {
                    let total: f64 = energies.iter().sum();
                    (total > 0.0).then(|| energies.map(|energy| energy / total))
                };
                let day: [[f64; 3]; 2] = std::array::from_fn(|slant| {
                    let all = std::array::from_fn(|k| {
                        (0..PERIODS)
                            .map(|period| self.installation_energy[period][slant][k])
                            .sum()
                    });
                    shares(all).unwrap_or([0.0; 3])
                });
                self.installation_energy.map(|period| {
                    std::array::from_fn(|slant| shares(period[slant]).unwrap_or(day[slant]))
                })
            },
        })
    }
}
