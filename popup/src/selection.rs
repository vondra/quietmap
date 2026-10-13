//! Source selection by bounded omission and shared importance samples, with contributor accounting.

use crate::candidates::{Attributes, Candidate, DisplayRef};
use crate::evaluate::{Bands, Path, Received, Receiver, Scratch, period_sums, received_bands};
use crate::listing::EvaluatedPiece;
use crate::sampling::BoundSample;
use crate::update::{CONTRIBUTOR_PIECES, Contributor};
use physics::bands::{BANDS, PERIODS};
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
    /// The pieces of the source whose sound path is asked for, every one evaluated.
    pub pieces: Vec<EvaluatedPiece>,
    /// The received band energies of every evaluated piece, certain and sampled alike: the
    /// spectrum's shape, not its level.
    pub spectrum: Bands,
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
            spectrum: [[0.0; BANDS]; PERIODS],
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
    fn add(&mut self, candidate: &Candidate, received: &Received) {
        let energy = period_sums(&received.bands);
        self.evaluated += 1;
        self.covered += 1;
        for (total, value) in self.energy.iter_mut().zip(energy) {
            *total += value;
        }
        self.add_spectrum(&received.bands);
        self.add_contributor(candidate, received);
    }

    /// Evaluates `candidates` (the pieces of the source whose sound path is asked for) with
    /// certainty, outside the stop rule and the sample, and keeps each for its listing.
    pub fn evaluate_kept(
        &mut self,
        candidates: Vec<Candidate>,
        receiver: &Receiver,
        attributes: &Attributes,
    ) -> Result<(), String> {
        let work: Vec<(usize, Candidate)> = candidates.into_iter().map(|c| (0, c)).collect();
        for ((_, candidate), received) in work.iter().zip(evaluate_all(&work, receiver, attributes))
        {
            let received = received?;
            self.add(candidate, &received);
            self.pieces
                .push(EvaluatedPiece::of(candidate, attributes, &received));
        }
        Ok(())
    }

    fn add_spectrum(&mut self, bands: &Bands) {
        for (total, period) in self.spectrum.iter_mut().zip(bands) {
            for (sum, value) in total.iter_mut().zip(period) {
                *sum += value;
            }
        }
    }

    /// Adds one evaluated piece to its contributor group.
    fn add_contributor(&mut self, candidate: &Candidate, received: &Received) {
        let energy = period_sums(&received.bands);
        let states = received.path.states;
        let contributor = self
            .contributors
            .entry(candidate.group_key)
            .or_insert_with(|| Contributor {
                group_key: candidate.group_key,
                layer: candidate.layer,
                energy: [0.0; PERIODS],
                weather: Default::default(),
                distance_m: candidate.distance_m,
                display: candidate.display,
                display_lden: f64::NEG_INFINITY,
                pieces: Vec::new(),
                lines: Vec::new(),
                heard: None,
                nden_sone: None,
                share: None,
                path: Path::default(),
            });
        for (total, value) in contributor.energy.iter_mut().zip(energy) {
            *total += value;
        }
        contributor.weather.add(&energy, &states);
        contributor.path.add(&received.path);
        contributor.distance_m = contributor.distance_m.min(candidate.distance_m);
        let lden = physics::bands::lden_energy(&energy);
        let order = |d: DisplayRef| (d.ring, d.tile, d.attribute);
        if lden > contributor.display_lden
            || (lden == contributor.display_lden
                && order(candidate.display) < order(contributor.display))
        {
            (contributor.display, contributor.display_lden) = (candidate.display, lden);
        }
        contributor.pieces.push((candidate.ends_m, lden));
        if contributor.pieces.len() >= 2 * CONTRIBUTOR_PIECES {
            loudest_pieces(&mut contributor.pieces);
        }
    }
}

/// Keeps the [`CONTRIBUTOR_PIECES`] loudest pieces, loudest first.
pub fn loudest_pieces(pieces: &mut Vec<([[f64; 2]; 2], f64)>) {
    pieces.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0[0][0].total_cmp(&b.0[0][0])));
    pieces.truncate(CONTRIBUTOR_PIECES);
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
/// the sample of a click reproducible.
pub fn select(
    selections: &mut [LayerSelection],
    receiver: &Receiver,
    attributes: &Attributes,
    exact: bool,
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
        let received = evaluate_all(&work, receiver, attributes);
        for ((layer, candidate), received) in work.into_iter().zip(received) {
            selections[layer].add(&candidate, &received?);
        }
    }
    for (layer, selection) in selections.iter_mut().enumerate() {
        if sampled[layer] {
            sample_rest(
                selection,
                receiver,
                attributes,
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
) -> Vec<Result<Received, String>> {
    work.par_iter()
        .map_init(Scratch::default, |scratch, (_, candidate)| {
            received_bands(
                receiver,
                candidate,
                &attributes[candidate.attribute],
                scratch,
            )
        })
        .collect()
}

/// SplitMix64: a small reproducible generator (the click's sample, the etalon's, the card's checks).
pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
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
    seed: u64,
) -> Result<(), String> {
    let mut rest = std::mem::take(&mut selection.pending);
    // Pending stays sorted ascending (only its loudest end is ever drained).
    rest.reverse();
    let mut evaluated: HashMap<usize, ([f64; PERIODS], Received)> = HashMap::new();
    let orders: Vec<f64> = rest.iter().map(|candidate| candidate.order).collect();
    let mut sample = BoundSample::new(seed);
    loop {
        let round = sample.round(&orders);
        let (first, certain) = (round.certain.start, round.certain.end);
        let draws = &round.draws;
        let exhaustive = round.exhaustive;
        let needed: Vec<(usize, Candidate)> = (first..certain)
            .chain(draws.iter().copied())
            .filter(|index| !evaluated.contains_key(index))
            .collect::<std::collections::BTreeSet<usize>>()
            .into_iter()
            .map(|index| (index, rest[index].clone()))
            .collect();
        let received = evaluate_all(&needed, receiver, attributes);
        for ((index, _), received) in needed.iter().zip(received) {
            let received = received?;
            evaluated.insert(*index, (period_sums(&received.bands), received));
        }
        for index in first..certain {
            selection.add(&rest[index], &evaluated[&index].1);
        }
        let (estimate, variance) = round.estimate(&orders, |index| evaluated[&index].0);
        let answer: [f64; PERIODS] =
            std::array::from_fn(|period| selection.answer_energy()[period] + estimate[period]);
        if sample.converged(&round, variance, answer) {
            for period in 0..PERIODS {
                selection.estimate[period] += estimate[period];
                selection.variance[period] += variance[period];
            }
            if !exhaustive {
                selection.covered += round.population;
            }
            // Each drawn piece stands for its share of the estimate: drawn m times out of n with
            // probability p, its weight is m / (n p), so the spectrum sums to the layer's estimate.
            // The estimate stays the layer's steady rest, no contributor's: weighted into its own
            // group a drawn piece would carry other groups' energy as its passes (+1.5 sone where
            // every piece evaluated read 2.55).
            for (index, weight) in round.weights(&orders) {
                let (energy, received) = &evaluated[&index];
                if exhaustive {
                    selection.add(&rest[index], received);
                    for (estimate, value) in selection.estimate.iter_mut().zip(energy) {
                        *estimate -= value;
                    }
                } else {
                    selection.evaluated += 1;
                    selection.add_spectrum(
                        &received
                            .bands
                            .map(|period| period.map(|band| band * weight)),
                    );
                }
            }
            return Ok(());
        }
        sample.advance(&round);
    }
}
