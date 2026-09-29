//! Per-layer selection: candidates are evaluated from the loudest bound until the bounds of
//! everything left out stay below (10^(0.1/10) - 1) times the energy evaluated, per period and
//! across all rings. Never a per-piece threshold (dev4's false-quiet bug 62e7daa2).
//!
//! EXPERIMENT (branch r051-sampling, not decided): when that proven rule would need more than
//! `PROVEN_ROUND_LIMIT` further evaluations, the rest of the layer is estimated instead: pieces
//! whose bound is large against the rest are evaluated with certainty, the others sampled with
//! probability proportional to their bound (Hansen-Hurwitz), doubling the sample until two
//! standard errors fall under `SAMPLING_TOLERANCE` of the answer.

use crate::candidates::{Attributes, Candidate};
use crate::evaluate::{Receiver, Scratch, received_energy};
use crate::listing::EvaluatedPiece;
use crate::update::Contributor;
use physics::bands::PERIODS;
use rayon::prelude::*;
use std::collections::HashMap;
use tiles::sources::Layer;

/// The omitted energy may reach this fraction of the evaluated energy: 0.1 dB.
pub const OMITTED_ENERGY_FRACTION: f64 = 0.023_292_992_280_754_13;
/// The fewest candidates one round evaluates per layer, so the pool stays busy.
const MINIMUM_BATCH: usize = 16;
/// When the proven rule would take a layer's evaluations in one call past this, the rest of the
/// layer is sampled.
const PROVEN_ROUND_LIMIT: usize = 1_024;
/// The first sample and the largest one.
const SAMPLE_START: usize = 512;
const SAMPLE_MAX: usize = 32_768;
/// Two standard errors of the estimate may reach this fraction of the answer: 0.05 dB.
const SAMPLING_TOLERANCE: f64 = 0.011_579_454_376_269_092;

/// One layer's account across the rings.
pub struct LayerSelection {
    pub layer: Layer,
    pub pending: Vec<Candidate>,
    /// Energy of the pieces evaluated with certainty.
    pub energy: [f64; PERIODS],
    /// Estimated energy of the sampled rest, and the variance of that estimate.
    pub estimate: [f64; PERIODS],
    pub variance: [f64; PERIODS],
    /// Pieces put through the full physics (certain and sampled).
    pub evaluated: usize,
    /// Pieces no longer pending: evaluated with certainty or covered by an estimate.
    pub covered: usize,
    pub contributors: HashMap<u64, Contributor>,
    /// Every evaluated piece, kept only when the benchmark lists pieces.
    pub pieces: Vec<EvaluatedPiece>,
}

impl LayerSelection {
    pub fn new(layer: Layer) -> Self {
        LayerSelection {
            layer,
            pending: Vec::new(),
            energy: [0.0; PERIODS],
            estimate: [0.0; PERIODS],
            variance: [0.0; PERIODS],
            evaluated: 0,
            covered: 0,
            contributors: HashMap::new(),
            pieces: Vec::new(),
        }
    }

    /// The layer's answer: the certain energy and the estimate of the sampled rest.
    pub fn answer_energy(&self) -> [f64; PERIODS] {
        std::array::from_fn(|period| self.energy[period] + self.estimate[period])
    }

    /// What the answer may lack: the bounds still pending and two standard errors.
    pub fn uncertainty(&self) -> [f64; PERIODS] {
        let pending = self.pending_bound();
        std::array::from_fn(|period| pending[period] + 2.0 * self.variance[period].sqrt())
    }

    pub fn pending_bound(&self) -> [f64; PERIODS] {
        let mut sum = [0.0; PERIODS];
        for candidate in &self.pending {
            for (total, bound) in sum.iter_mut().zip(candidate.bound) {
                *total += bound;
            }
        }
        sum
    }

    /// The fewest loudest candidates whose evaluation could satisfy the rule: as if each
    /// delivered its whole bound, the most any can.
    fn fewest_to_satisfy(&self) -> usize {
        let mut remaining = self.pending_bound();
        let mut evaluated = self.energy;
        for (taken, candidate) in self.pending.iter().rev().enumerate() {
            if (0..PERIODS).all(|p| remaining[p] <= OMITTED_ENERGY_FRACTION * evaluated[p]) {
                return taken;
            }
            for period in 0..PERIODS {
                remaining[period] -= candidate.bound[period];
                evaluated[period] += candidate.bound[period];
            }
        }
        self.pending.len()
    }

    fn satisfied(&self, exact: bool) -> bool {
        if exact || self.pending.is_empty() {
            return self.pending.is_empty();
        }
        let omitted = self.pending_bound();
        let answer = self.answer_energy();
        (0..PERIODS).all(|period| omitted[period] <= OMITTED_ENERGY_FRACTION * answer[period])
    }

    /// Adds one evaluated piece to the certain energy and its contributor group.
    fn add(
        &mut self,
        candidate: &Candidate,
        energy: [f64; PERIODS],
        attributes: &Attributes,
        keep_pieces: bool,
    ) {
        self.evaluated += 1;
        self.covered += 1;
        for (total, value) in self.energy.iter_mut().zip(energy) {
            *total += value;
        }
        self.add_contributor(candidate, energy);
        if keep_pieces {
            self.pieces
                .push(EvaluatedPiece::of(candidate, attributes, energy));
        }
    }

    fn add_contributor(&mut self, candidate: &Candidate, energy: [f64; PERIODS]) {
        let contributor = self
            .contributors
            .entry(candidate.group_key)
            .or_insert_with(|| Contributor {
                group_key: candidate.group_key,
                layer: candidate.layer,
                energy: [0.0; PERIODS],
                distance_m: candidate.distance_m,
                display: candidate.display,
            });
        for (total, value) in contributor.energy.iter_mut().zip(energy) {
            *total += value;
        }
        contributor.distance_m = contributor.distance_m.min(candidate.distance_m);
    }
}

/// Ascending bound, ties broken by the source itself so every run evaluates in the same order.
fn evaluation_order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    a.order
        .total_cmp(&b.order)
        .then(a.attribute.list.cmp(&b.attribute.list))
        .then(a.attribute.index.cmp(&b.attribute.index))
        .then(a.ends_m[0][0].total_cmp(&b.ends_m[0][0]))
        .then(a.ends_m[0][1].total_cmp(&b.ends_m[0][1]))
}

/// Evaluates candidates from the loudest bound, in parallel batches over all unsatisfied layers,
/// until every layer's omitted-energy account allows it to stop (`exact`: until none is left);
/// a layer whose proven round would exceed `PROVEN_ROUND_LIMIT` is sampled instead. `seed` makes
/// the sample of a click reproducible. `keep_pieces` keeps every evaluated piece for the
/// benchmark's listing.
pub fn select(
    selections: &mut [LayerSelection],
    receiver: &Receiver,
    attributes: &Attributes,
    exact: bool,
    keep_pieces: bool,
    seed: u64,
) -> Result<(), String> {
    for selection in selections.iter_mut() {
        selection.pending.par_sort_unstable_by(evaluation_order);
    }
    let mut sampled = vec![false; selections.len()];
    let mut taken = vec![0usize; selections.len()];
    loop {
        let mut work: Vec<(usize, Candidate)> = Vec::new();
        for (layer, selection) in selections.iter_mut().enumerate() {
            if sampled[layer] || selection.satisfied(exact) {
                continue;
            }
            let take = if exact {
                selection.pending.len()
            } else {
                selection.fewest_to_satisfy().max(MINIMUM_BATCH)
            };
            if !exact && taken[layer] + take.min(selection.pending.len()) > PROVEN_ROUND_LIMIT {
                sampled[layer] = true;
                continue;
            }
            taken[layer] += take.min(selection.pending.len());
            let start = selection.pending.len() - take.min(selection.pending.len());
            work.extend(
                selection
                    .pending
                    .drain(start..)
                    .map(|candidate| (layer, candidate)),
            );
        }
        if work.is_empty() {
            break;
        }
        let energies = evaluate_all(&work, receiver, attributes);
        for ((layer, candidate), energy) in work.into_iter().zip(energies) {
            selections[layer].add(&candidate, energy?, attributes, keep_pieces);
        }
    }
    for (layer, selection) in selections.iter_mut().enumerate() {
        if sampled[layer] {
            sample_rest(
                selection,
                receiver,
                attributes,
                keep_pieces,
                seed ^ (layer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            )?;
        }
    }
    Ok(())
}

fn evaluate_all(
    work: &[(usize, Candidate)],
    receiver: &Receiver,
    attributes: &Attributes,
) -> Vec<Result<[f64; PERIODS], String>> {
    work.par_iter()
        .map_init(Scratch::default, |scratch, (_, candidate)| {
            received_energy(
                receiver,
                candidate,
                &attributes[candidate.attribute],
                scratch,
            )
        })
        .collect()
}

/// SplitMix64: a small reproducible generator for the sample.
struct SplitMix(u64);

impl SplitMix {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Estimates the layer's pending pieces: those with a bound of at least the rest's sum over the
/// sample size are evaluated with certainty; the sample draws the others with probability
/// proportional to their bound (with replacement) and doubles until two standard errors of the
/// estimate fall under the tolerance, or the sample reaches its largest size.
fn sample_rest(
    selection: &mut LayerSelection,
    receiver: &Receiver,
    attributes: &Attributes,
    keep_pieces: bool,
    seed: u64,
) -> Result<(), String> {
    let mut rest = std::mem::take(&mut selection.pending);
    // Pending stays sorted ascending (only its loudest end is ever drained).
    rest.reverse();
    let mut evaluated: HashMap<usize, [f64; PERIODS]> = HashMap::new();
    let mut size = SAMPLE_START;
    let mut first = 0;
    loop {
        // Certainty pieces for this sample size.
        let mut rest_order: f64 = rest[first..].iter().map(|c| c.order).sum();
        let mut certain = first;
        while certain < rest.len() && rest[certain].order * size as f64 >= rest_order {
            rest_order -= rest[certain].order;
            certain += 1;
        }
        let population = rest.len() - certain;
        let exhaustive = population <= size;
        let mut random = SplitMix(seed ^ size as u64);
        let draws: Vec<usize> = if exhaustive {
            (certain..rest.len()).collect()
        } else {
            let cumulative: Vec<f64> = rest[certain..]
                .iter()
                .scan(0.0, |sum, c| {
                    *sum += c.order;
                    Some(*sum)
                })
                .collect();
            let total = *cumulative.last().expect("a non-empty rest");
            (0..size)
                .map(|_| {
                    let target = random.unit() * total;
                    certain
                        + cumulative
                            .partition_point(|&sum| sum <= target)
                            .min(population - 1)
                })
                .collect()
        };
        let needed: Vec<(usize, Candidate)> = (first..certain)
            .chain(draws.iter().copied())
            .filter(|index| !evaluated.contains_key(index))
            .collect::<std::collections::BTreeSet<usize>>()
            .into_iter()
            .map(|index| (index, rest[index].clone()))
            .collect();
        let energies = evaluate_all(&needed, receiver, attributes);
        for ((index, _), energy) in needed.iter().zip(energies) {
            evaluated.insert(*index, energy?);
        }
        for index in first..certain {
            selection.add(&rest[index], evaluated[&index], attributes, keep_pieces);
        }
        first = certain;
        let (estimate, variance) = if exhaustive {
            let mut sum = [0.0; PERIODS];
            for index in certain..rest.len() {
                for (total, value) in sum.iter_mut().zip(evaluated[&index]) {
                    *total += value;
                }
            }
            (sum, [0.0; PERIODS])
        } else {
            let n = draws.len() as f64;
            let mut mean = [0.0; PERIODS];
            let mut square = [0.0; PERIODS];
            for &index in &draws {
                let probability = rest[index].order / rest_order;
                for period in 0..PERIODS {
                    let y = evaluated[&index][period] / probability;
                    mean[period] += y / n;
                    square[period] += y * y / n;
                }
            }
            let variance = std::array::from_fn(|period| {
                (square[period] - mean[period] * mean[period]).max(0.0) * n / (n - 1.0) / n
            });
            (mean, variance)
        };
        let answer: [f64; PERIODS] =
            std::array::from_fn(|period| selection.answer_energy()[period] + estimate[period]);
        let precise = (0..PERIODS)
            .all(|period| 2.0 * variance[period].sqrt() <= SAMPLING_TOLERANCE * answer[period]);
        if exhaustive || precise || size >= SAMPLE_MAX {
            for period in 0..PERIODS {
                selection.estimate[period] += estimate[period];
                selection.variance[period] += variance[period];
            }
            if !exhaustive {
                selection.covered += population;
            }
            let mut shown = std::collections::BTreeSet::new();
            for &index in &draws {
                if shown.insert(index) {
                    let energy = evaluated[&index];
                    if exhaustive {
                        selection.add(&rest[index], energy, attributes, keep_pieces);
                        for (estimate, value) in selection.estimate.iter_mut().zip(energy) {
                            *estimate -= value;
                        }
                    } else {
                        selection.evaluated += 1;
                        selection.add_contributor(&rest[index], energy);
                    }
                }
            }
            return Ok(());
        }
        size *= 2;
    }
}
