//! Rank a building's sources, stopping when the remaining ceilings cannot enter its top hundred.

use super::{FACADE_SOURCE_LIMIT, Facade};
use crate::candidates::{Attributes, Candidate};
use physics::bands::lden_energy;
use physics::bound::{ReceiverBound, received_lden_ceiling};
use rayon::prelude::*;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

/// A bound and its candidate's place in the ranking's order: the greater bound first, of equal
/// bounds the earlier place.
#[derive(Clone, Copy)]
struct Ranked(f64, usize);

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(self.1.cmp(&other.1))
    }
}

/// The sources every façade sums: of `candidates` the [`FACADE_SOURCE_LIMIT`] of greatest bound at
/// any façade, which falls with distance, so at the nearest one, under `gain`; each with that
/// bound's Lden energy and its place in `candidates`, greatest first, equal bounds in their order
/// there. No bound exceeds its ceiling at the gap between the candidate's box and the façades'
/// (`physics::bound::received_lden_ceiling`): candidates are bounded in their ceilings' order until
/// the next ceiling falls below the limit's last bound, as no later one can rank.
pub(super) fn facade_sources(
    facades: &[Facade],
    candidates: &[&Candidate],
    attributes: &Attributes,
    gain: &ReceiverBound,
) -> Vec<(f64, usize)> {
    let bound = |index: usize| {
        let candidate = candidates[index];
        let nearest = (facades.iter())
            .map(|facade| candidate.distance_from(facade.position))
            .fold(f64::INFINITY, f64::min);
        let source = &attributes[candidate.attribute];
        Ranked(
            lden_energy(&candidate.bound_at_distance(nearest, source, gain)),
            index,
        )
    };
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for facade in facades {
        for axis in 0..2 {
            low[axis] = low[axis].min(facade.position[axis]);
            high[axis] = high[axis].max(facade.position[axis]);
        }
    }
    let largest_gain = gain.gain.iter().copied().fold(0.0, f64::max);
    let mut by_ceiling: BinaryHeap<Reverse<Ranked>> = (candidates.par_iter().enumerate())
        .map(|(index, candidate)| {
            let [a, b] = candidate.ends_m;
            let gap = |axis: usize| {
                (low[axis] - a[axis].max(b[axis]))
                    .max(a[axis].min(b[axis]) - high[axis])
                    .max(0.0)
            };
            let power = attributes[candidate.attribute].lden_power;
            let distance = gap(0).hypot(gap(1));
            let ceiling = received_lden_ceiling(power, candidate.spread(), distance, largest_gain);
            Reverse(Ranked(ceiling, index))
        })
        .collect();
    // The ranked so far, the one ranking last on top.
    let mut kept: BinaryHeap<Ranked> = BinaryHeap::with_capacity(FACADE_SOURCE_LIMIT + 1);
    while let Some(Reverse(Ranked(ceiling, index))) = by_ceiling.pop() {
        if kept.len() == FACADE_SOURCE_LIMIT && kept.peek().is_some_and(|last| ceiling < last.0) {
            break;
        }
        kept.push(bound(index));
        if kept.len() > FACADE_SOURCE_LIMIT {
            kept.pop();
        }
    }
    (kept.into_sorted_vec().into_iter())
        .map(|Ranked(bound, index)| (bound, index))
        .collect()
}

#[cfg(test)]
#[path = "facade_sources_tests.rs"]
mod tests;
