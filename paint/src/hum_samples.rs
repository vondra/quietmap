//! Quiet source importance samples batched across hum receivers through the shared exact evaluator.

use crate::exact::{Batch, LAYERS, Pair, Point, PointBands, whole_energy};
use crate::hum::HumPointSound;
use crate::paint::PASS_PAIRS;
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use physics::bound::received_energy_bound;
use popup::candidates::{GROUND_REACH_M, loud};
use popup::evaluate::{Bands, Scratch};
use popup::sampling::{BoundSample, SampleRound};
use popup::selection::OMITTED_ENERGY_FRACTION;
use rayon::prelude::*;
use std::collections::BTreeSet;

/// A layer's hum is sampled first with this many of its quiet sources (a click's sample starts with
/// 512): 72 % of the hums of the two lightest classes of ground squares lie under 10 dB, and a
/// first round of 512 was 37 % of their pairs where one of 64 is 7 % (15,377 hums of 28 squares
/// whose quiet sources were all traced; the world sample, 2026-11-18).
const FIRST_DRAWS: usize = 64;

/// Two standard errors of a layer's sampled hum may reach this Lden energy (15 dB) whatever the
/// layer's answer: under 20 dB no visitor hears a layer, and 15 dB moves a total of 32 dB, the
/// quietest held to half a decibel, by 0.09 dB. A click's budget, 0.05 dB of the layer's answer,
/// asked the same of a hum of 5 dB under no loud source: 85 % of those hums lie under 15 dB, and
/// with this the sample's map changes by over 1 phon at 0.001 % of its visible cells.
const UNHEARD_LDEN_ENERGY: f64 = 31.622_776_601_683_793;

/// Whether two standard errors of a sampled hum (its `variance` a period) stay under
/// [`UNHEARD_LDEN_ENERGY`] together: one sample draws all three periods, so their errors add.
fn unheard(variance: &[f64; PERIODS]) -> bool {
    lden_energy(&variance.map(|variance| 2.0 * variance.sqrt())) <= UNHEARD_LDEN_ENERGY
}

struct QuietSourceSample {
    point: usize,
    layer: usize,
    indices: Vec<u32>,
    orders: Vec<f64>,
    evaluated: Vec<Option<usize>>,
    values: Vec<([f64; PERIODS], Bands)>,
    certain_energy: [f64; PERIODS],
    sample: BoundSample,
    finished: Option<SampleRound>,
}

impl QuietSourceSample {
    fn energy(&self, index: usize) -> [f64; PERIODS] {
        self.values[self.evaluated[index].unwrap()].0
    }
}

pub(crate) fn sampled_hum(
    square: &Square,
    points: &[Point],
    loud_energy: &[[[f64; PERIODS]; LAYERS]],
    seeds: &[u64],
    batch: Option<&dyn Batch>,
) -> Result<Vec<HumPointSound>, String> {
    let mut samples: Vec<QuietSourceSample> = points
        .par_iter()
        .enumerate()
        .flat_map(|(at, point)| {
            let mut near = Vec::new();
            square
                .index
                .within(point.position, point.position, GROUND_REACH_M, &mut near);
            let mut pending: [Vec<(u32, f64)>; LAYERS] = std::array::from_fn(|_| Vec::new());
            let mut bounds = [[0.0; PERIODS]; LAYERS];
            const REACH_SQ: f64 = GROUND_REACH_M * GROUND_REACH_M * (1.0 + 16.0 * f64::EPSILON);
            for index in near {
                let candidate = &square.candidates[index as usize];
                let [dx, dy] = candidate.offset_from(point.position);
                if dx * dx + dy * dy > REACH_SQ {
                    continue;
                }
                let distance_m = dx.hypot(dy);
                if distance_m > GROUND_REACH_M {
                    continue;
                }
                let bound = received_energy_bound(
                    &square.attributes[candidate.attribute].energy,
                    square.spreads[index as usize],
                    distance_m,
                    &point.bound,
                );
                if !loud(&bound) {
                    let layer = candidate.layer as usize;
                    pending[layer].push((index, lden_energy(&bound)));
                    for (sum, value) in bounds[layer].iter_mut().zip(bound) {
                        *sum += value;
                    }
                }
            }
            pending
                .into_iter()
                .enumerate()
                .filter_map(|(layer, mut pending)| {
                    if pending.is_empty()
                        || (0..PERIODS).all(|period| {
                            bounds[layer][period]
                                <= OMITTED_ENERGY_FRACTION * loud_energy[at][layer][period]
                        })
                    {
                        return None;
                    }
                    pending.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
                    let (indices, orders): (Vec<_>, Vec<_>) = pending.into_iter().unzip();
                    Some(QuietSourceSample {
                        point: at,
                        layer,
                        evaluated: vec![None; indices.len()],
                        values: Vec::new(),
                        indices,
                        orders,
                        certain_energy: [0.0; PERIODS],
                        sample: BoundSample::starting_at(
                            seeds[at] ^ (layer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
                            FIRST_DRAWS,
                        ),
                        finished: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut quiet = vec![([[0.0; PERIODS]; LAYERS], PointBands::default()); points.len()];
    loop {
        let rounds: Vec<_> = samples
            .iter()
            .enumerate()
            .filter(|(_, sample)| sample.finished.is_none())
            .map(|(index, sample)| (index, sample.sample.round(&sample.orders)))
            .collect();
        if rounds.is_empty() {
            break;
        }
        let needed: Vec<_> = rounds
            .iter()
            .flat_map(|(at, round)| {
                round
                    .certain
                    .clone()
                    .chain(round.draws.iter().copied())
                    .filter(|&index| samples[*at].evaluated[index].is_none())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|index| (*at, index))
                    .collect::<Vec<_>>()
            })
            .collect();
        for chunk in needed.chunks(PASS_PAIRS) {
            let pairs: Vec<_> = chunk
                .iter()
                .map(|&(at, index)| Pair {
                    point: samples[at].point as u32,
                    candidate: samples[at].indices[index],
                    band_weight: 0.0,
                })
                .collect();
            let evaluated = match batch {
                Some(batch) => batch.pair_spectra(points, &pairs)?,
                None => pairs
                    .par_iter()
                    .map_init(Scratch::default, |scratch, pair| {
                        whole_energy(
                            square,
                            &points[pair.point as usize],
                            pair.candidate,
                            scratch,
                        )
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            };
            for (&(at, index), (energy, bands)) in chunk.iter().zip(evaluated) {
                let sample = &mut samples[at];
                sample.evaluated[index] = Some(sample.values.len());
                sample.values.push((energy.mixed, bands));
            }
        }
        for (at, round) in rounds {
            let sample = &mut samples[at];
            for index in round.certain.clone() {
                let energy = sample.energy(index);
                for (sum, value) in sample.certain_energy.iter_mut().zip(energy) {
                    *sum += value;
                }
            }
            let (estimate, variance) = round.estimate(&sample.orders, |index| sample.energy(index));
            let answer = std::array::from_fn(|period| {
                loud_energy[sample.point][sample.layer][period]
                    + sample.certain_energy[period]
                    + estimate[period]
            });
            if unheard(&variance) || sample.sample.converged(&round, variance, answer) {
                quiet[sample.point].0[sample.layer] =
                    std::array::from_fn(|period| sample.certain_energy[period] + estimate[period]);
                sample.finished = Some(round);
            } else {
                sample.sample.advance(&round);
            }
        }
    }
    for sample in &samples {
        let round = sample.finished.as_ref().unwrap();
        for (index, weight) in (0..round.certain.end)
            .map(|index| (index, 1.0))
            .chain(round.weights(&sample.orders))
        {
            let bands = &sample.values[sample.evaluated[index].unwrap()].1;
            for (total, bands) in quiet[sample.point].1[sample.layer].iter_mut().zip(bands) {
                for (total, band) in total.iter_mut().zip(bands) {
                    *total += weight * band;
                }
            }
        }
    }
    for (energy, layers) in &mut quiet {
        for (energy, bands) in energy.iter().zip(layers) {
            for (energy, bands) in energy.iter().zip(bands) {
                let sum: f64 = bands.iter().sum();
                let scale = if sum > 0.0 { energy / sum } else { 0.0 };
                for band in bands {
                    *band *= scale;
                }
            }
        }
    }
    Ok(quiet)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hum's budget is its Lden energy: two standard errors of 20 by day and in the evening
    /// and of 3 at night stay under 15 dB together, of 4 at night do not (a night's decibel
    /// counts ten times), though each period alone would pass a third of the budget's share.
    #[test]
    fn a_hums_errors_are_unheard_together_in_lden() {
        let of = |two_errors: [f64; PERIODS]| two_errors.map(|error| (error / 2.0).powi(2));
        assert!(unheard(&of([20.0, 20.0, 3.0])));
        assert!(!unheard(&of([20.0, 20.0, 4.0])));
    }
}
