//! Sub-block loudness lattices checked against pixel probes, retaining exact samples and screening edges.
//!
//! These samples use the ordinary painter's source admission, propagation and sound model.
//! A block first paints a coarse lattice (its edges and middle) and one probe a quadrant; where all
//! four agree and no source or screening edge crosses the block, the coarse lattice blends the whole
//! block. Elsewhere the fine lattice and its sixteen patch probes follow, the coarse samples among
//! them. Interpolation replaces a patch's remaining receivers only after checking loudness and Lden;
//! rejected patches reuse their exact lattice and probe results. Half-phon and half-dB checks match the stored steps,
//! an empirical safeguard verified against the independent etalon, not a bound on unsampled rays.

use crate::blocks::{Block, meets};
use crate::exact::{LAYERS, Point};
use crate::levels::{Painted, SILENCE, cell};
use crate::paint::{
    PASS_BLOCKS, bilinear, metres_at, rectangle_at, runs_within, shortest_pixel_side_metres,
};
use crate::points::PointPainter;
use rayon::prelude::*;

pub(crate) struct QuietSamples {
    axis: [usize; 5],
    values: Vec<Option<Painted>>,
    accepted: [bool; 16],
}

impl QuietSamples {
    fn patch_at(&self, x: usize, y: usize) -> [usize; 2] {
        let interval = |at: usize| {
            self.axis
                .partition_point(|&sample| sample <= at)
                .saturating_sub(1)
                .min(3)
        };
        [interval(x), interval(y)]
    }

    pub fn at(&self, x: usize, y: usize) -> Painted {
        let [i, j] = self.patch_at(x, y);
        let fraction =
            |at, low| (at - self.axis[low]) as f64 / (self.axis[low + 1] - self.axis[low]) as f64;
        let (fx, fy) = (fraction(x, i), fraction(y, j));
        let around = lattice_corners([i, j]);
        (
            std::array::from_fn(|layer| {
                bilinear(around.map(|at| self.values[at].unwrap().0[layer]), fx, fy)
            }),
            std::array::from_fn(|layer| {
                bilinear(around.map(|at| self.values[at].unwrap().1[layer]), fx, fy)
            }),
        )
    }

    pub fn accepts(&self, x: usize, y: usize) -> bool {
        let [i, j] = self.patch_at(x, y);
        self.accepted[j * 4 + i]
    }

    pub fn complete(&self) -> bool {
        self.accepted.iter().all(|&accepted| accepted)
    }

    pub fn interpolated_patches(&self) -> usize {
        self.accepted.iter().filter(|&&accepted| accepted).count()
    }

    fn agrees_with_probe(
        &self,
        [i, j]: [usize; 2],
        (x, y): (usize, usize),
        heard: &Painted,
    ) -> bool {
        // A one-pixel interval has no independent interior probe.
        if x <= self.axis[i]
            || x >= self.axis[i + 1]
            || y <= self.axis[j]
            || y >= self.axis[j + 1]
            || lattice_corners([i, j])
                .iter()
                .any(|&at| self.values[at].is_none())
        {
            return false;
        }
        let blended = self.at(x, y);
        agrees_with_interpolation(&heard.1, &blended.1)
            && energies_agree_within_half_decibel(&heard.0, &blended.0)
    }

    fn sample_index(&self, x: usize, y: usize) -> Option<usize> {
        if let (Some(i), Some(j)) = (
            self.axis.iter().position(|&at| at == x),
            self.axis.iter().position(|&at| at == y),
        ) {
            return Some(j * 5 + i);
        }
        let centres: [usize; 4] = std::array::from_fn(|at| (self.axis[at] + self.axis[at + 1]) / 2);
        if let (Some(i), Some(j)) = (
            centres.iter().position(|&at| at == x),
            centres.iter().position(|&at| at == y),
        ) {
            return Some(25 + j * 4 + i);
        }
        None
    }

    pub fn has_paint(&self, x: usize, y: usize) -> bool {
        self.sample_index(x, y)
            .map_or_else(|| self.accepts(x, y), |at| self.values[at].is_some())
    }

    pub fn known_at(&self, x: usize, y: usize) -> Option<Painted> {
        if let Some(at) = self.sample_index(x, y) {
            return self.values[at];
        }
        self.accepts(x, y).then(|| self.at(x, y))
    }
}

/// Each block's exact samples and checked patches; rejected patches retain their other pixels.
pub(crate) struct SampledBlocks {
    pub blocks: Vec<(usize, QuietSamples)>,
    pub admissions: Vec<(usize, Block)>,
}

pub(crate) fn sample_block_pixels(
    painter: &PointPainter,
    block_ids: Vec<usize>,
) -> Result<SampledBlocks, String> {
    let (square, grid) = (painter.square, painter.grid);
    let side = grid.pixels / grid.block;
    let b = grid.block;
    // The wall can be a whole pixel from the centre of the pixel in front of its façade receiver.
    let receiver_protection_pixels = 1.0
        + popup::obstacles::FACADE_RECEIVER_OFFSET_M
            / shortest_pixel_side_metres(square, grid.pixels);
    let (axis, positions) = sample_positions(b);
    let passes = (0..block_ids.len())
        .step_by(PASS_BLOCKS)
        .map(|start| start..(start + PASS_BLOCKS).min(block_ids.len()))
        .collect();
    // The existing scheduler overlaps sample assembly and card work, retaining its bounded chunks.
    let sampled = painter.in_passes(passes, |range| {
        let chunk = &block_ids[range];
        let mut accepted = Vec::new();
        let mut retain = vec![true; chunk.len()];
        let blocks: Vec<_> = chunk
            .par_iter()
            .map_init(Vec::new, |near, &at| painter.block(near, at))
            .collect();
        let bounds: Vec<_> = blocks
            .iter()
            .map(|block| positions.len() * block.exact.len())
            .collect();
        let rectangle_of = |at: usize, low: [usize; 2], high: [usize; 2]| {
            let (base_x, base_y) = ((at % side) * b, (at / side) * b);
            rectangle_at(
                square,
                grid.pixels,
                (base_x + low[0]) as f64 + 0.5 - receiver_protection_pixels,
                (base_y + low[1]) as f64 + 0.5 - receiver_protection_pixels,
                (base_x + high[0]) as f64 + 0.5 + receiver_protection_pixels,
                (base_y + high[1]) as f64 + 0.5 + receiver_protection_pixels,
            )
        };
        // Whether block `slot` of the chunk has no exact source and no screening edge across a
        // rectangle of its pixels (with the façade margin).
        let clear = |slot: usize, at: usize, low: [usize; 2], high: [usize; 2]| {
            let (low, high) = rectangle_of(at, low, high);
            let near = blocks[slot].exact.iter().any(|&source| {
                let [a, b] = square.candidates[source as usize].ends_m;
                meets(a, b, low, high)
            });
            !near && square.obstacles.has_no_screening_edges(low, high)
        };
        for run in runs_within(&bounds, PASS_BLOCKS) {
            let ids = &chunk[run.clone()];
            // First the coarse lattice and its quadrants' probes; then the rest where they disagree.
            let coarse = paint_positions(painter, &blocks[run.clone()], ids, &COARSE_FIRST)?;
            let mut values: Vec<Vec<Option<Painted>>> =
                vec![vec![None; positions.len()]; ids.len()];
            let mut finer = Vec::new();
            for (slot, &at) in ids.iter().enumerate() {
                for (&k, value) in COARSE_FIRST.iter().zip(&coarse[slot]) {
                    values[slot][k] = *value;
                }
                // The quadrants' rectangles with their margins cover the block's with its margin.
                match coarse_lattice(axis, &values[slot]) {
                    Some(lattice) if clear(run.start + slot, at, [0, 0], [axis[4], axis[4]]) => {
                        values[slot] = lattice;
                    }
                    _ => finer.push(slot),
                }
            }
            let rest: Vec<usize> = (0..positions.len())
                .filter(|k| !COARSE_FIRST.contains(k))
                .collect();
            let finer_ids: Vec<usize> = finer.iter().map(|&slot| ids[slot]).collect();
            let finer_blocks: Vec<&Block> = finer
                .iter()
                .map(|&slot| &blocks[run.start + slot])
                .collect();
            let painted = paint_positions(painter, &finer_blocks, &finer_ids, &rest)?;
            for (&slot, heard) in finer.iter().zip(painted) {
                for (&k, value) in rest.iter().zip(heard) {
                    values[slot][k] = value;
                }
            }
            for (slot, &at) in ids.iter().enumerate() {
                let complete_coarse = !finer.contains(&slot);
                let mut sampled = QuietSamples {
                    axis,
                    values: std::mem::take(&mut values[slot]),
                    accepted: [complete_coarse; 16],
                };
                if complete_coarse {
                    retain[run.start + slot] = false;
                    accepted.push((at, sampled));
                    continue;
                }
                let heard = sampled.values.clone();
                for (cell, (&(x, y), probe)) in positions[25..].iter().zip(&heard[25..]).enumerate()
                {
                    let Some(heard) = probe else {
                        continue;
                    };
                    let (i, j) = (cell % 4, cell / 4);
                    if !sampled.agrees_with_probe([i, j], (x, y), heard) {
                        continue;
                    }
                    sampled.accepted[cell] = clear(
                        run.start + slot,
                        at,
                        [axis[i], axis[j]],
                        [axis[i + 1], axis[j + 1]],
                    );
                }
                retain[run.start + slot] = !sampled.complete();
                if sampled.values.iter().any(Option::is_some) {
                    accepted.push((at, sampled));
                }
            }
        }
        let admissions = chunk
            .iter()
            .copied()
            .zip(blocks)
            .zip(retain)
            .filter_map(|(block, retain)| retain.then_some(block))
            .collect::<Vec<_>>();
        Ok((accepted, admissions))
    })?;
    let mut blocks = Vec::new();
    let mut admissions = Vec::new();
    for (_, (accepted, retained)) in sampled {
        admissions.extend(retained);
        blocks.extend(accepted);
    }
    Ok(SampledBlocks { blocks, admissions })
}

/// The sample positions painted first (indices of `sample_positions`): the lattice at every second
/// axis line (the block's edges and middle) and the four lattice points between them, each the
/// probe of one quadrant.
const COARSE_FIRST: [usize; 13] = [0, 2, 4, 10, 12, 14, 20, 22, 24, 6, 8, 16, 18];

/// A block's 41 samples blended from its coarse lattice (`values` at [`COARSE_FIRST`], the rest
/// unknown) when each quadrant's probe agrees with the blend there, as a patch's probe must; `None`
/// otherwise. The coarse lattice keeps its exact values; every other sample, the probes too, takes
/// the blend, so that the fine lattice interpolates the coarse one exactly.
fn coarse_lattice(axis: [usize; 5], values: &[Option<Painted>]) -> Option<Vec<Option<Painted>>> {
    let coarse = |i: usize, j: usize| values[j * 5 + i];
    let blend = |x: usize, y: usize| -> Option<Painted> {
        let i = if x < axis[2] { 0 } else { 2 };
        let j = if y < axis[2] { 0 } else { 2 };
        let fraction =
            |at: usize, low: usize| (at - axis[low]) as f64 / (axis[low + 2] - axis[low]) as f64;
        let (fx, fy) = (fraction(x, i), fraction(y, j));
        let around = [
            coarse(i, j)?,
            coarse(i + 2, j)?,
            coarse(i, j + 2)?,
            coarse(i + 2, j + 2)?,
        ];
        Some((
            std::array::from_fn(|layer| bilinear(around.map(|value| value.0[layer]), fx, fy)),
            std::array::from_fn(|layer| bilinear(around.map(|value| value.1[layer]), fx, fy)),
        ))
    };
    for (i, j) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
        let (probe, blended) = (values[j * 5 + i]?, blend(axis[i], axis[j])?);
        if !(agrees_with_interpolation(&probe.1, &blended.1)
            && energies_agree_within_half_decibel(&probe.0, &blended.0))
        {
            return None;
        }
    }
    let (_, positions) = sample_positions(axis[4] + 1);
    positions
        .iter()
        .enumerate()
        .map(|(k, &(x, y))| match COARSE_FIRST[..9].contains(&k) {
            true => values[k].map(Some),
            false => blend(x, y).map(Some),
        })
        .collect()
}

/// Each block's paint at `positions` (indices of `sample_positions`), `ids` the blocks' places.
fn paint_positions(
    painter: &PointPainter,
    blocks: &[impl std::borrow::Borrow<Block> + Sync],
    ids: &[usize],
    positions: &[usize],
) -> Result<Vec<Vec<Option<Painted>>>, String> {
    let (square, grid) = (painter.square, painter.grid);
    let (side, b) = (grid.pixels / grid.block, grid.block);
    let (_, all) = sample_positions(b);
    let places: Vec<_> = ids
        .iter()
        .enumerate()
        .flat_map(|(slot, &at)| {
            let (x, y) = ((at % side) * b, (at / side) * b);
            let all = &all;
            positions.iter().map(move |&k| {
                let (dx, dy) = all[k];
                (slot, (y + dy) * grid.pixels + x + dx)
            })
        })
        .collect();
    if places.is_empty() {
        return Ok(Vec::new());
    }
    let points = places
        .par_iter()
        .map(|&(_, at)| {
            Point::at(
                square,
                metres_at(
                    square,
                    grid.pixels,
                    (at % grid.pixels) as f64 + 0.5,
                    (at / grid.pixels) as f64 + 0.5,
                ),
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let painted = painter.paint(blocks, &points, &places)?;
    Ok(painted.chunks(positions.len()).map(<[_]>::to_vec).collect())
}

fn lattice_corners([i, j]: [usize; 2]) -> [usize; 4] {
    let first = j * 5 + i;
    [first, first + 1, first + 5, first + 6]
}

fn sample_positions(b: usize) -> ([usize; 5], Vec<(usize, usize)>) {
    let axis = [0, b / 4, b / 2, 3 * b / 4, b - 1];
    let centres: [usize; 4] = std::array::from_fn(|at| (axis[at] + axis[at + 1]) / 2);
    let positions: Vec<_> = axis
        .into_iter()
        .flat_map(|y| axis.into_iter().map(move |x| (x, y)))
        .chain(
            centres
                .into_iter()
                .flat_map(|y| centres.into_iter().map(move |x| (x, y))),
        )
        .collect();
    (axis, positions)
}

/// A layer quieter than this at the probe and in the blend needs only [`QUIET_LAYER_SONE`] there
/// (sone): the map shows it clear under 1 sone and the etalon scores a layer from 1 sone, while the
/// total keeps the half phon. In Dobris such layers alone rejected a sixth of the probed patches of
/// its busiest square, 2207/1394 (2026-10-14).
const QUIET_LAYER_UNDER_SONE: f64 = 0.5;
/// How far a quiet layer may differ from its blend (sone): what the map's hover reads stays within
/// a tenth of a sone, never a quiet place's silence read as a few tenths.
const QUIET_LAYER_SONE: f64 = 0.1;

/// A quiet intermittent source may pass the loudness check and still change Lden substantially.
/// Below-zero layers need no level, but their sum can cross the stored floor.
fn energy_agrees(heard: f64, blended: f64) -> bool {
    heard.is_finite()
        && blended.is_finite()
        && heard >= 0.0
        && blended >= 0.0
        && (heard.max(blended) < 1.0
            || (heard > 0.0 && blended > 0.0 && (10.0 * (heard / blended).log10()).abs() <= 0.5))
}

fn energies_agree_within_half_decibel(heard: &[f64; LAYERS], blended: &[f64; LAYERS]) -> bool {
    (0..LAYERS).all(|layer| energy_agrees(heard[layer], blended[layer]))
        && energy_agrees(heard.iter().sum(), blended.iter().sum())
}

/// One half-phon cell step in the total and in every layer not quieter than
/// [`QUIET_LAYER_UNDER_SONE`], a tenth of a sone in the quieter; silence is already quantized.
fn loudness_agrees(layer: usize, heard: f64, blended: f64) -> bool {
    if !heard.is_finite() || !blended.is_finite() || heard < 0.0 || blended < 0.0 {
        return false;
    }
    if layer < LAYERS && heard.max(blended) < QUIET_LAYER_UNDER_SONE {
        return (heard - blended).abs() <= QUIET_LAYER_SONE;
    }
    if cell(heard) == SILENCE && cell(blended) == SILENCE {
        return true;
    }
    heard > 0.0 && blended > 0.0 && (10.0 * (heard / blended).log2()).abs() <= 0.5
}

fn agrees_with_interpolation(heard: &[f64; LAYERS + 1], blended: &[f64; LAYERS + 1]) -> bool {
    (0..=LAYERS).all(|layer| loudness_agrees(layer, heard[layer], blended[layer]))
}

#[cfg(test)]
#[path = "quiet_samples_tests.rs"]
mod tests;
