//! A pass's points traced: each one's block's exact sources loud there, and its block's deeper list where the transfers change much of its sound.

use crate::blocks::{Block, admit_deeper};
use crate::exact::{Evaluated, Pair, PairEnergy, Point, PointBands, evaluate_all};
use crate::paint::corner_weights;
use crate::points::PointPainter;
use physics::bands::lden_energy;
use rayon::prelude::*;
use std::borrow::Borrow;
use std::sync::atomic::Ordering;

/// One round of tracing: each point's sources (sorted), where its pairs start among the round's,
/// the pairs' energies and the points' bands.
struct Round {
    lists: Vec<Vec<u32>>,
    starts: Vec<usize>,
    evaluated: Evaluated,
}

impl Round {
    /// Traces `lists[k]` at `points[k]`.
    fn trace(
        painter: &PointPainter,
        points: &[Point],
        lists: Vec<Vec<u32>>,
    ) -> Result<Self, String> {
        let pairs: Vec<Pair> = (lists.iter().enumerate())
            .flat_map(|(k, list)| list.iter().map(move |&index| Pair::new(k as u32, index)))
            .collect();
        painter.pairs.fetch_add(pairs.len(), Ordering::Relaxed);
        let evaluated = evaluate_all(painter.square, points, &pairs, painter.batch)?;
        let mut end = 0;
        let starts = (lists.iter())
            .map(|list| {
                end += list.len();
                end - list.len()
            })
            .collect();
        Ok(Self {
            lists,
            starts,
            evaluated,
        })
    }

    /// Point `k`'s sources, their energies and its bands.
    fn at(&self, k: usize) -> (&[u32], &[PairEnergy], &PointBands) {
        let pairs = self.starts[k]..self.starts[k] + self.lists[k].len();
        let evaluated = &self.evaluated;
        (&self.lists[k], &evaluated.pairs[pairs], &evaluated.bands[k])
    }
}

/// A point that traced deeper: its deeper block (among [`Traced::deeper_blocks`]) and its sources
/// of both rounds together (sorted), their energies and its bands.
struct Deeper {
    block: usize,
    sources: Vec<u32>,
    energies: Vec<PairEnergy>,
    bands: PointBands,
}

/// What a pass's points traced: `places[k]` is point `k`'s block's slot among `blocks` and its
/// pixel (row-major).
pub(crate) struct Traced<'a, B> {
    blocks: &'a [B],
    places: &'a [(usize, usize)],
    first: Round,
    deeper_blocks: Vec<Block>,
    deeper: Vec<Deeper>,
    /// Each point's place among `deeper`, `u32::MAX` for one that did not trace deeper.
    deeper_of: Vec<u32>,
}

/// A traced source's Lden energy among a point's `sources` (sorted) and their `energies`.
pub(crate) fn heard_among<'a>(
    sources: &'a [u32],
    energies: &'a [PairEnergy],
) -> impl Fn(u32) -> Option<f64> + 'a {
    move |index| {
        let at = sources.binary_search(&index).ok()?;
        Some(lden_energy(&energies[at].mixed))
    }
}

impl<'a, B: Borrow<Block> + Sync> Traced<'a, B> {
    /// Traces at every outdoor point its block's exact sources loud there; then, where the
    /// transfers change much of the point's ground sound (`Reaching::asks_deeper`), what its
    /// block's deeper list adds.
    pub fn trace(
        painter: &PointPainter,
        blocks: &'a [B],
        points: &[Point],
        places: &'a [(usize, usize)],
    ) -> Result<Self, String> {
        let square = painter.square;
        let loud = |k: usize, index: u32| square.loud(index, points[k].position, &points[k].bound);
        let lists: Vec<Vec<u32>> = (0..points.len())
            .into_par_iter()
            .map(|k| {
                if points[k].own_footprint != 0 {
                    return Vec::new();
                }
                let exact = &blocks[places[k].0].borrow().exact;
                exact
                    .iter()
                    .copied()
                    .filter(|&index| loud(k, index))
                    .collect()
            })
            .collect();
        let first = Round::trace(painter, points, lists)?;
        let asks: Vec<bool> = (0..points.len())
            .into_par_iter()
            .map(|k| {
                let block: &Block = blocks[places[k].0].borrow();
                if points[k].own_footprint != 0 || block.left_groups.is_empty() {
                    return false;
                }
                let (sources, energies, _) = first.at(k);
                let (_, _, (fx, fy)) = painter.place_of(places[k].1);
                let reaching =
                    block.reaching(corner_weights(fx, fy), heard_among(sources, energies));
                let traced = energies
                    .iter()
                    .map(|energy| lden_energy(&energy.mixed))
                    .sum();
                reaching.asks_deeper(traced)
            })
            .collect();
        // The asking points' blocks, each once, with a point of it (its corners' place).
        let mut slots: Vec<(usize, usize)> = (0..points.len())
            .filter(|&k| asks[k])
            .map(|k| (places[k].0, k))
            .collect();
        slots.sort_unstable();
        slots.dedup_by_key(|&mut (slot, _)| slot);
        let deeper_blocks: Vec<Block> = (slots.par_iter())
            .map(|&(slot, k)| {
                let (_, around, _) = painter.place_of(places[k].1);
                let corners = around.map(|at| &painter.corners.corners[at]);
                admit_deeper(blocks[slot].borrow(), corners)
            })
            .collect();
        let deeper_block = |k: usize| {
            (slots.binary_search_by_key(&places[k].0, |&(slot, _)| slot)).expect("an asking block")
        };
        let more: Vec<Vec<u32>> = (0..points.len())
            .into_par_iter()
            .map(|k| {
                if !asks[k] {
                    return Vec::new();
                }
                let exact = &deeper_blocks[deeper_block(k)].exact;
                (exact.iter().copied())
                    .filter(|index| first.lists[k].binary_search(index).is_err())
                    .filter(|&index| loud(k, index))
                    .collect()
            })
            .collect();
        let second = match more.iter().all(Vec::is_empty) {
            true => None,
            false => Some(Round::trace(painter, points, more)?),
        };
        let asking: Vec<usize> = (0..points.len()).filter(|&k| asks[k]).collect();
        let deeper: Vec<Deeper> = (asking.par_iter())
            .map(|&k| {
                let (sources, energies, bands) = first.at(k);
                let mut both: Vec<(u32, PairEnergy)> =
                    (sources.iter().copied().zip(energies.iter().copied())).collect();
                let mut bands = *bands;
                if let Some(second) = &second {
                    let (sources, energies, more) = second.at(k);
                    both.extend(sources.iter().copied().zip(energies.iter().copied()));
                    for (sum, value) in (bands.as_flattened_mut().as_flattened_mut().iter_mut())
                        .zip(more.as_flattened().as_flattened())
                    {
                        *sum += value;
                    }
                }
                // Two sorted runs: the stable sort merges them.
                both.sort_by_key(|&(index, _)| index);
                let (sources, energies) = both.into_iter().unzip();
                Deeper {
                    block: deeper_block(k),
                    sources,
                    energies,
                    bands,
                }
            })
            .collect();
        let mut deeper_of = vec![u32::MAX; points.len()];
        for (place, &k) in asking.iter().enumerate() {
            deeper_of[k] = place as u32;
        }
        painter.points.fetch_add(points.len(), Ordering::Relaxed);
        painter.deeper.fetch_add(deeper.len(), Ordering::Relaxed);
        Ok(Self {
            blocks,
            places,
            first,
            deeper_blocks,
            deeper,
            deeper_of,
        })
    }

    /// Point `k`'s block as it traced it, its sources (sorted), their energies and its bands.
    pub fn at(&self, k: usize) -> (&Block, &[u32], &[PairEnergy], &PointBands) {
        match self.deeper.get(self.deeper_of[k] as usize) {
            Some(deeper) => (
                &self.deeper_blocks[deeper.block],
                &deeper.sources,
                &deeper.energies,
                &deeper.bands,
            ),
            None => {
                let (sources, energies, bands) = self.first.at(k);
                (
                    self.blocks[self.places[k].0].borrow(),
                    sources,
                    energies,
                    bands,
                )
            }
        }
    }
}
