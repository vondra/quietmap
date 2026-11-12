//! Source blocks sampled: a block's corners, coarse lattice or fine lattice checked against pixel probes, and the patches that blend.
//!
//! These samples use the ordinary painter's source admission, propagation and sound model. A block
//! no source or screening edge crosses paints one probe a quadrant first: where all four agree with
//! the blend of the block's corners, the corners paint the block; else with its coarse lattice (its
//! edges and middle), that does. Elsewhere the fine lattice and its sixteen patch probes follow,
//! the coarse samples among them, and each patch blends where its probe agrees.

use crate::blocks::{Block, meets};
use crate::exact::{LAYERS, Point};
use crate::lattice_blend::{blend, loudness_slopes};
use crate::levels::Painted;
use crate::paint::{PASS_BLOCKS, metres_at, rectangle_at, runs_within, shortest_pixel_side_metres};
use crate::points::PointPainter;
use crate::quiet_samples::{QuietSamples, sample_positions};
use crate::visible::visibly_agrees;
use rayon::prelude::*;

/// Each block's exact samples and checked patches; rejected patches retain their other pixels.
pub(crate) struct SampledBlocks {
    pub blocks: Vec<(usize, QuietSamples)>,
    pub admissions: Vec<(usize, Block)>,
}

pub(crate) fn sample_block_pixels(
    painter: &PointPainter,
    block_ids: Vec<usize>,
    corner_paint: &[Painted],
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
            .map(|block| positions.len() * block.most)
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
        // A block's four corners' paint, where all four stand outdoors.
        let corners_of = |at: usize| -> Option<[Painted; 4]> {
            let first = (at / side) * (side + 1) + at % side;
            let around = [first, first + 1, first + side + 1, first + side + 2];
            (around.iter())
                .all(|&corner| painter.corners.corner_points[corner].own_footprint == 0)
                .then(|| around.map(|corner| corner_paint[corner]))
        };
        for run in runs_within(&bounds, PASS_BLOCKS) {
            let ids = &chunk[run.clone()];
            let mut values: Vec<Vec<Option<Painted>>> =
                vec![vec![None; positions.len()]; ids.len()];
            // A block no source or screening edge crosses may blend where each quadrant's probe
            // agrees: first from its corners, then from the coarse lattice (its edges and middle);
            // then the rest where they disagree.
            let open: Vec<usize> = (0..ids.len())
                .filter(|&slot| clear(run.start + slot, ids[slot], [0, 0], [axis[4], axis[4]]))
                .collect();
            let subset = |slots: &[usize]| -> (Vec<&Block>, Vec<usize>) {
                (slots.iter())
                    .map(|&slot| (&blocks[run.start + slot], ids[slot]))
                    .unzip()
            };
            let (open_blocks, open_ids) = subset(&open);
            let probes = paint_positions(painter, &open_blocks, &open_ids, &QUADRANT_PROBES)?;
            let mut coarser = Vec::new();
            for (&slot, heard) in open.iter().zip(probes) {
                for (&k, value) in QUADRANT_PROBES.iter().zip(&heard) {
                    values[slot][k] = *value;
                }
                let blended =
                    (corners_of(ids[slot])).and_then(|corners| corner_lattice(b, corners, &heard));
                match blended {
                    Some(lattice) => values[slot] = lattice,
                    None => coarser.push(slot),
                }
            }
            let coarse_rest = &COARSE_FIRST[..9];
            let (coarser_blocks, coarser_ids) = subset(&coarser);
            let coarse = paint_positions(painter, &coarser_blocks, &coarser_ids, coarse_rest)?;
            let mut finer: Vec<usize> =
                (0..ids.len()).filter(|slot| !open.contains(slot)).collect();
            for (&slot, heard) in coarser.iter().zip(coarse) {
                for (&k, value) in coarse_rest.iter().zip(heard) {
                    values[slot][k] = value;
                }
                match coarse_lattice(axis, &values[slot]) {
                    Some(lattice) => values[slot] = lattice,
                    None => finer.push(slot),
                }
            }
            finer.sort_unstable();
            // What a finer block has not painted yet: all of a block not open, else the fine
            // lattice beside the coarse one.
            let (whole, fine): (Vec<usize>, Vec<usize>) =
                finer.iter().partition(|slot| !open.contains(slot));
            let all: Vec<usize> = (0..positions.len()).collect();
            let rest: Vec<usize> = (all.iter().copied())
                .filter(|k| !COARSE_FIRST.contains(k))
                .collect();
            for (slots, missing) in [(&whole, &all), (&fine, &rest)] {
                let (finer_blocks, finer_ids) = subset(slots);
                let painted = paint_positions(painter, &finer_blocks, &finer_ids, missing)?;
                for (&slot, heard) in slots.iter().zip(painted) {
                    for (&k, value) in missing.iter().zip(heard) {
                        values[slot][k] = value;
                    }
                }
            }
            for (slot, &at) in ids.iter().enumerate() {
                let complete_coarse = !finer.contains(&slot);
                let mut sampled = QuietSamples {
                    axis,
                    values: std::mem::take(&mut values[slot]),
                    accepted: [complete_coarse; 16],
                    carried: [false; 16],
                    slopes: [0.0; LAYERS + 1],
                };
                if complete_coarse {
                    retain[run.start + slot] = false;
                    accepted.push((at, sampled));
                    continue;
                }
                sampled.slopes = loudness_slopes(&sampled.values[..25]);
                for (cell, &(x, y)) in positions[25..].iter().enumerate() {
                    let Some(heard) = sampled.values[25 + cell] else {
                        continue;
                    };
                    let (i, j) = (cell % 4, cell / 4);
                    sampled.carried[cell] = sampled.carries_probe([i, j], (x, y), &heard);
                    if !sampled.agrees_with_probe([i, j], (x, y), &heard) {
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
/// The quadrants' probes among them, painted before all: the probes of the corners' blend too.
const QUADRANT_PROBES: [usize; 4] = [6, 8, 16, 18];

/// A block of `b` pixels' 41 samples blended from its four `corners`' paint (north-west, north-east,
/// south-west, south-east) when its quadrants' `probes` agree with the blend there, as a patch's
/// probe must; `None` otherwise. Every sample, the probes too, takes the blend, which the lattice
/// then interpolates exactly. The corners hear every loud source, and neighbouring blocks share
/// them. (One probe in the middle let a road beside the block bend its field unseen: of 554 popup
/// clicks of three squares 16 instead of 8 were over 1 phon.)
fn corner_lattice(
    b: usize,
    corners: [Painted; 4],
    probes: &[Option<Painted>],
) -> Option<Vec<Option<Painted>>> {
    let within = |at: usize| (at as f64 + 0.5) / b as f64;
    let (_, positions) = sample_positions(b);
    let blended = |k: usize| blend(corners, within(positions[k].0), within(positions[k].1));
    (QUADRANT_PROBES.iter().zip(probes))
        .all(|(&k, probe)| probe.is_some_and(|probe| visibly_agrees(&probe, &blended(k))))
        .then(|| (0..positions.len()).map(|k| Some(blended(k))).collect())
}

/// A block's 41 samples blended from its coarse lattice (`values` at [`COARSE_FIRST`], the rest
/// unknown) when each quadrant's probe agrees with the blend there, as a patch's probe must; `None`
/// otherwise. The coarse lattice keeps its exact values; every other sample, the probes too, takes
/// the blend, so that the fine lattice interpolates the coarse one exactly.
fn coarse_lattice(axis: [usize; 5], values: &[Option<Painted>]) -> Option<Vec<Option<Painted>>> {
    let coarse = |i: usize, j: usize| values[j * 5 + i];
    let blended = |x: usize, y: usize| -> Option<Painted> {
        let i = if x < axis[2] { 0 } else { 2 };
        let j = if y < axis[2] { 0 } else { 2 };
        let fraction =
            |at: usize, low: usize| (at - axis[low]) as f64 / (axis[low + 2] - axis[low]) as f64;
        let around = [
            coarse(i, j)?,
            coarse(i + 2, j)?,
            coarse(i, j + 2)?,
            coarse(i + 2, j + 2)?,
        ];
        Some(blend(around, fraction(x, i), fraction(y, j)))
    };
    for (i, j) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
        if !visibly_agrees(&values[j * 5 + i]?, &blended(axis[i], axis[j])?) {
            return None;
        }
    }
    let (_, positions) = sample_positions(axis[4] + 1);
    positions
        .iter()
        .enumerate()
        .map(|(k, &(x, y))| match COARSE_FIRST[..9].contains(&k) {
            true => values[k].map(Some),
            false => blended(x, y).map(Some),
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

#[cfg(test)]
#[path = "block_samples_tests.rs"]
mod tests;
