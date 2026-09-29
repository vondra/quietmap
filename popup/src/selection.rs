//! Per-layer selection: candidates are evaluated from the loudest bound until the bounds of
//! everything left out stay below (10^(0.1/10) - 1) times the energy evaluated, per period and
//! across all rings. Never a per-piece threshold (dev4's false-quiet bug 62e7daa2).

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

/// One layer's account across the rings.
pub struct LayerSelection {
    pub layer: Layer,
    pub pending: Vec<Candidate>,
    pub energy: [f64; PERIODS],
    pub evaluated: usize,
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
            evaluated: 0,
            contributors: HashMap::new(),
            pieces: Vec::new(),
        }
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
        if exact {
            return self.pending.is_empty();
        }
        let omitted = self.pending_bound();
        (0..PERIODS).all(|period| omitted[period] <= OMITTED_ENERGY_FRACTION * self.energy[period])
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
/// until every layer's omitted-energy account allows it to stop (`exact`: until none is left).
/// `keep_pieces` keeps every evaluated piece for the benchmark's listing.
pub fn select(
    selections: &mut [LayerSelection],
    receiver: &Receiver,
    attributes: &Attributes,
    exact: bool,
    keep_pieces: bool,
) -> Result<(), String> {
    for selection in selections.iter_mut() {
        selection.pending.par_sort_unstable_by(evaluation_order);
    }
    loop {
        let mut work: Vec<(usize, Candidate)> = Vec::new();
        for (layer, selection) in selections.iter_mut().enumerate() {
            if !selection.satisfied(exact) {
                let take = if exact {
                    selection.pending.len()
                } else {
                    selection.fewest_to_satisfy().max(MINIMUM_BATCH)
                };
                let start = selection.pending.len() - take.min(selection.pending.len());
                work.extend(
                    selection
                        .pending
                        .drain(start..)
                        .map(|candidate| (layer, candidate)),
                );
            }
        }
        if work.is_empty() {
            return Ok(());
        }
        let energies: Vec<Result<[f64; PERIODS], String>> = work
            .par_iter()
            .map_init(Scratch::default, |scratch, (_, candidate)| {
                received_energy(
                    receiver,
                    candidate,
                    &attributes[candidate.attribute],
                    scratch,
                )
            })
            .collect();
        for ((layer, candidate), energy) in work.into_iter().zip(energies) {
            let energy = energy?;
            let selection = &mut selections[layer];
            selection.evaluated += 1;
            for (total, value) in selection.energy.iter_mut().zip(energy) {
                *total += value;
            }
            let contributor = selection
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
            if keep_pieces {
                selection
                    .pieces
                    .push(EvaluatedPiece::of(&candidate, attributes, energy));
            }
        }
    }
}
