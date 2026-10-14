//! Sub-block loudness lattices checked against pixel probes, retaining exact samples and screening edges.
//!
//! These samples use the ordinary painter's source admission, propagation and sound model.
//! Interpolation replaces a patch's remaining receivers only after checking each layer and total;
//! rejected patches reuse their exact lattice and probe results. The half-phon check matches the map cell's step,
//! an empirical safeguard verified against the independent etalon, not a bound on unsampled rays.

use crate::blocks::meets;
use crate::exact::{LAYERS, Point};
use crate::paint::{
    PASS_BLOCKS, Painted, SILENCE, bilinear, cell, metres_at, rectangle_at, runs_within,
    shortest_pixel_side_metres,
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
        heard: &[f64; LAYERS + 1],
    ) -> bool {
        // A one-pixel interval has no independent interior probe.
        x > self.axis[i]
            && x < self.axis[i + 1]
            && y > self.axis[j]
            && y < self.axis[j + 1]
            && lattice_corners([i, j])
                .iter()
                .all(|&at| self.values[at].is_some())
            && agrees_with_interpolation(heard, &self.at(x, y).1)
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
pub(crate) fn sample_block_pixels(
    painter: &PointPainter,
    block_ids: Vec<usize>,
) -> Result<Vec<(usize, QuietSamples)>, String> {
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
        let blocks: Vec<_> = chunk
            .par_iter()
            .map_init(Vec::new, |near, &at| painter.block(near, at))
            .collect();
        let bounds: Vec<_> = blocks
            .iter()
            .map(|block| positions.len() * block.exact.len())
            .collect();
        for run in runs_within(&bounds, PASS_BLOCKS) {
            let ids = &chunk[run.clone()];
            let places: Vec<_> = ids
                .iter()
                .enumerate()
                .flat_map(|(slot, &at)| {
                    let (x, y) = ((at % side) * b, (at / side) * b);
                    positions
                        .iter()
                        .map(move |&(dx, dy)| (slot, (y + dy) * grid.pixels + x + dx))
                })
                .collect();
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
            let painted = painter.paint(&blocks[run.clone()], &points, &places)?;
            for (slot, &at) in ids.iter().enumerate() {
                let heard = &painted[slot * positions.len()..(slot + 1) * positions.len()];
                let mut sampled = QuietSamples {
                    axis,
                    values: heard.to_vec(),
                    accepted: [false; 16],
                };
                for (cell, (&(x, y), probe)) in positions[25..].iter().zip(&heard[25..]).enumerate()
                {
                    let Some((_, ndens)) = probe else { continue };
                    let (i, j) = (cell % 4, cell / 4);
                    if !sampled.agrees_with_probe([i, j], (x, y), ndens) {
                        continue;
                    }
                    let (base_x, base_y) = ((at % side) * b, (at / side) * b);
                    let (low, high) = rectangle_at(
                        square,
                        grid.pixels,
                        (base_x + axis[i]) as f64 + 0.5 - receiver_protection_pixels,
                        (base_y + axis[j]) as f64 + 0.5 - receiver_protection_pixels,
                        (base_x + axis[i + 1]) as f64 + 0.5 + receiver_protection_pixels,
                        (base_y + axis[j + 1]) as f64 + 0.5 + receiver_protection_pixels,
                    );
                    let near = blocks[run.start + slot].exact.iter().any(|&source| {
                        let [a, b] = square.candidates[source as usize].ends_m;
                        meets(a, b, low, high)
                    });
                    sampled.accepted[cell] =
                        !near && square.obstacles.has_no_screening_edges(low, high);
                }
                if sampled.values.iter().any(Option::is_some) {
                    accepted.push((at, sampled));
                }
            }
        }
        Ok(accepted)
    })?;
    Ok(sampled
        .into_iter()
        .flat_map(|(_, accepted)| accepted)
        .collect())
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

/// One half-phon cell step in the total and in every layer not quieter than
/// [`QUIET_LAYER_UNDER_SONE`], a tenth of a sone in the quieter; silence is already quantized.
fn agrees_with_interpolation(heard: &[f64; LAYERS + 1], blended: &[f64; LAYERS + 1]) -> bool {
    (0..=LAYERS).all(|layer| {
        let (heard, blended) = (heard[layer], blended[layer]);
        if !heard.is_finite() || !blended.is_finite() || heard < 0.0 || blended < 0.0 {
            return false;
        }
        if cell(heard) == SILENCE && cell(blended) == SILENCE {
            return true;
        }
        if layer < LAYERS && heard.max(blended) < QUIET_LAYER_UNDER_SONE {
            return (heard - blended).abs() <= QUIET_LAYER_SONE;
        }
        heard > 0.0 && blended > 0.0 && (10.0 * (heard / blended).log2()).abs() <= 0.5
    })
}

#[cfg(test)]
#[path = "quiet_samples_tests.rs"]
mod tests;
