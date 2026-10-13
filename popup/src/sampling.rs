//! Shared bound-proportional sampling rounds, weights and confidence for quiet source energies.

use crate::selection::SplitMix;
use physics::bands::PERIODS;
use std::collections::BTreeMap;
use std::ops::Range;

/// Two standard errors may reach this fraction of the whole answer: 0.05 dB.
const SAMPLING_TOLERANCE: f64 = 0.011_579_454_376_269_092;

pub struct BoundSample {
    first: usize,
    size: usize,
    seed: u64,
}

pub struct SampleRound {
    pub certain: Range<usize>,
    pub draws: Vec<usize>,
    pub exhaustive: bool,
    pub population: usize,
    rest_order: f64,
}

impl BoundSample {
    pub fn new(seed: u64) -> Self {
        Self {
            first: 0,
            size: 512,
            seed,
        }
    }

    /// Orders descend, so certainty pieces precede the sampled population.
    pub fn round(&self, orders: &[f64]) -> SampleRound {
        let mut rest_order: f64 = orders[self.first..].iter().sum();
        let mut certain = self.first;
        while certain < orders.len() && orders[certain] * self.size as f64 >= rest_order {
            rest_order -= orders[certain];
            certain += 1;
        }
        let population = orders.len() - certain;
        let exhaustive = population <= self.size;
        let mut random = SplitMix(self.seed ^ self.size as u64);
        let draws = if exhaustive {
            (certain..orders.len()).collect()
        } else {
            let cumulative: Vec<f64> = orders[certain..]
                .iter()
                .scan(0.0, |sum, order| {
                    *sum += order;
                    Some(*sum)
                })
                .collect();
            let total = *cumulative.last().expect("a nonempty population");
            (0..self.size)
                .map(|_| {
                    let target = random.unit() * total;
                    certain
                        + cumulative
                            .partition_point(|&sum| sum <= target)
                            .min(population - 1)
                })
                .collect()
        };
        SampleRound {
            certain: self.first..certain,
            draws,
            exhaustive,
            population,
            rest_order,
        }
    }

    pub fn converged(
        &self,
        round: &SampleRound,
        variance: [f64; PERIODS],
        answer: [f64; PERIODS],
    ) -> bool {
        round.exhaustive
            || self.size >= 32_768
            || (0..PERIODS)
                .all(|period| 2.0 * variance[period].sqrt() <= SAMPLING_TOLERANCE * answer[period])
    }

    pub fn advance(&mut self, round: &SampleRound) {
        self.first = round.certain.end;
        self.size *= 2;
    }
}

impl SampleRound {
    pub fn estimate(
        &self,
        orders: &[f64],
        energy: impl Fn(usize) -> [f64; PERIODS],
    ) -> ([f64; PERIODS], [f64; PERIODS]) {
        let mut mean = [0.0; PERIODS];
        let mut square = [0.0; PERIODS];
        let count = self.draws.len() as f64;
        for &index in &self.draws {
            let probability = if self.exhaustive {
                1.0
            } else {
                orders[index] / self.rest_order
            };
            let scale = if self.exhaustive { 1.0 } else { 1.0 / count };
            for (period, value) in energy(index).into_iter().enumerate() {
                let value = value / probability;
                mean[period] += value * scale;
                square[period] += value * value * scale;
            }
        }
        let variance = if self.exhaustive {
            [0.0; PERIODS]
        } else {
            std::array::from_fn(|period| {
                (square[period] - mean[period] * mean[period]).max(0.0) * count
                    / (count - 1.0)
                    / count
            })
        };
        (mean, variance)
    }

    pub fn weights(&self, orders: &[f64]) -> BTreeMap<usize, f64> {
        let mut weights = BTreeMap::new();
        for &index in &self.draws {
            *weights.entry(index).or_default() += 1.0;
        }
        if !self.exhaustive {
            for (&index, weight) in &mut weights {
                *weight /= self.draws.len() as f64 * orders[index] / self.rest_order;
            }
        }
        weights
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certainty_and_random_draws_preserve_energy_and_silent_periods() {
        let mut orders = vec![1.0; 1_025];
        orders[0] = 20.0;
        let sample = BoundSample::new(3);
        let round = sample.round(&orders);
        assert_eq!(round.certain, 0..1);
        assert!(!round.exhaustive);
        let (estimated, variance) = round.estimate(&orders, |_| [2.0, 0.0, 4.0]);
        assert_eq!(estimated, [2_048.0, 0.0, 4_096.0]);
        assert_eq!(variance, [0.0; PERIODS]);
        assert!(sample.converged(&round, variance, estimated));
        let weighted_energy: f64 = round
            .weights(&orders)
            .values()
            .map(|weight| weight * 2.0)
            .sum();
        assert_eq!(weighted_energy, estimated[0]);
    }
}
