//! Quiet source importance samples batched across hum receivers through the shared exact evaluator.

use crate::exact::{Batch, LAYERS, Pair, Point, PointBands, evaluate_all};
use crate::hum::HumPointSound;
use crate::paint::PASS_PAIRS;
use crate::square::Square;
use physics::bands::PERIODS;
use popup::candidates::{GROUND_REACH_M, loud};
use popup::sampling::{BoundSample, SampleRound};
use popup::selection::OMITTED_ENERGY_FRACTION;
use rayon::prelude::*;
use std::collections::BTreeSet;

struct QuietSourceSample {
    point: usize,
    layer: usize,
    indices: Vec<u32>,
    orders: Vec<f64>,
    evaluated: Vec<Option<[f64; PERIODS]>>,
    certain_energy: [f64; PERIODS],
    sample: BoundSample,
    finished: Option<SampleRound>,
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
            for index in near {
                let mut candidate = square.candidates[index as usize].clone();
                if candidate.bound_at(
                    point.position,
                    &square.attributes[candidate.attribute],
                    &point.bound,
                ) && !loud(&candidate.bound)
                {
                    let layer = candidate.layer as usize;
                    pending[layer].push((index, candidate.order));
                    for (sum, value) in bounds[layer].iter_mut().zip(candidate.bound) {
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
                        indices,
                        orders,
                        certain_energy: [0.0; PERIODS],
                        sample: BoundSample::new(
                            seeds[at] ^ (layer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
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
            let energies = evaluate_all(square, points, &pairs, batch)?.pairs;
            for (&(at, index), energy) in chunk.iter().zip(energies) {
                samples[at].evaluated[index] = Some(energy.mixed);
            }
        }
        for (at, round) in rounds {
            let sample = &mut samples[at];
            for index in round.certain.clone() {
                for (sum, value) in sample
                    .certain_energy
                    .iter_mut()
                    .zip(sample.evaluated[index].unwrap())
                {
                    *sum += value;
                }
            }
            let (estimate, variance) =
                round.estimate(&sample.orders, |index| sample.evaluated[index].unwrap());
            let answer = std::array::from_fn(|period| {
                loud_energy[sample.point][sample.layer][period]
                    + sample.certain_energy[period]
                    + estimate[period]
            });
            if sample.sample.converged(&round, variance, answer) {
                quiet[sample.point].0[sample.layer] =
                    std::array::from_fn(|period| sample.certain_energy[period] + estimate[period]);
                sample.finished = Some(round);
            } else {
                sample.sample.advance(&round);
            }
        }
    }
    // Re-evaluate final draws with their weights instead of retaining a spectrum for every trial draw.
    let pairs: Vec<_> = samples
        .iter()
        .flat_map(|sample| {
            let round = sample.finished.as_ref().unwrap();
            (0..round.certain.end)
                .map(|index| (index, 1.0))
                .chain(round.weights(&sample.orders))
                .map(|(index, band_weight)| Pair {
                    point: sample.point as u32,
                    candidate: sample.indices[index],
                    band_weight,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for chunk in pairs.chunks(PASS_PAIRS) {
        let evaluated = evaluate_all(square, points, chunk, batch)?;
        for ((_, total), bands) in quiet.iter_mut().zip(evaluated.bands) {
            for (total, bands) in total.iter_mut().zip(bands) {
                for (total, bands) in total.iter_mut().zip(bands) {
                    for (total, value) in total.iter_mut().zip(bands) {
                        *total += value;
                    }
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
