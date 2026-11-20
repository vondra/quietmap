//! Source blocks sampled: a block's corners, coarse lattice or fine lattice checked against pixel probes, and the patches that blend.
//!
//! These samples use the ordinary painter's source admission, propagation and sound model. A block
//! no screening edge crosses paints one probe a quadrant first: where all four agree with the
//! blend of the block's corners, the corners paint the block; else with its coarse lattice (its
//! edges and middle), that does. Elsewhere the fine lattice and its sixteen patch probes follow,
//! the coarse samples among them, and each patch blends where its probe agrees. A blend takes the
//! block's own sources by their free field at the pixel ([`crate::lattice_blend::blend`]): a road
//! across a block is no reason to compute its pixels, only what the blend cannot tell of it.

use crate::blocks::Block;
use crate::exact::{LAYERS, Point};
use crate::free_field::OwnSources;
use crate::lattice_blend::Anchor;
use crate::levels::Painted;
use crate::paint::{PASS_BLOCKS, metres_at, rectangle_at, runs_within, shortest_pixel_side_metres};
use crate::points::PointPainter;
use crate::quiet_samples::{Lattice, QuietSamples, sample_positions};
use rayon::prelude::*;

/// Each block's exact samples and checked patches; rejected patches retain their other pixels,
/// and every source block returns its deepest source count for pass and façade bounds.
pub(crate) struct SampledBlocks {
    pub blocks: Vec<(usize, QuietSamples)>,
    pub admissions: Vec<(usize, Block)>,
    pub most_source_counts: Vec<(usize, usize)>,
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
    let corner_lines = [-0.5, b as f64 - 0.5];
    let coarse_lines = [axis[0], axis[2], axis[4]].map(|at| at as f64);
    let passes = (0..block_ids.len())
        .step_by(PASS_BLOCKS)
        .map(|start| start..(start + PASS_BLOCKS).min(block_ids.len()))
        .collect();
    // Whether no screening edge crosses a rectangle of block `at`'s pixels (with the margin
    // protecting the pixels used to rank façades).
    let edge_free = |at: usize, low: [usize; 2], high: [usize; 2]| {
        let (base_x, base_y) = ((at % side) * b, (at / side) * b);
        let (low, high) = rectangle_at(
            square,
            grid.pixels,
            (base_x + low[0]) as f64 + 0.5 - receiver_protection_pixels,
            (base_y + low[1]) as f64 + 0.5 - receiver_protection_pixels,
            (base_x + high[0]) as f64 + 0.5 + receiver_protection_pixels,
            (base_y + high[1]) as f64 + 0.5 + receiver_protection_pixels,
        );
        square.obstacles.has_no_screening_edges(low, high)
    };
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
        let own: Vec<OwnSources> = (blocks.par_iter())
            .map(|block| OwnSources::of(square, &block.own))
            .collect::<Result<_, String>>()?;
        for run in runs_within(&bounds, PASS_BLOCKS) {
            let ids = &chunk[run.clone()];
            let block_of = |slot: usize| &blocks[run.start + slot];
            let own_of = |slot: usize| &own[run.start + slot];
            let mut samples: Vec<Vec<Option<Anchor>>> =
                vec![vec![None; positions.len()]; ids.len()];
            // Paints the `wanted` samples of the blocks in `slots`.
            let sample = |samples: &mut [Vec<Option<Anchor>>],
                          slots: &[usize],
                          wanted: &[usize]|
             -> Result<(), String> {
                let (blocks, ids): (Vec<&Block>, Vec<usize>) =
                    (slots.iter().map(|&slot| (block_of(slot), ids[slot]))).unzip();
                let own: Vec<&OwnSources> = slots.iter().map(|&slot| own_of(slot)).collect();
                let painted =
                    paint_positions(painter, (&blocks, &own), &ids, (wanted, &positions))?;
                for (&slot, heard) in slots.iter().zip(painted) {
                    for (&k, anchor) in wanted.iter().zip(heard) {
                        samples[slot][k] = anchor;
                    }
                }
                Ok(())
            };
            // A block no screening edge crosses may blend whole where each quadrant's probe agrees:
            // first from its corners, then from the coarse lattice (its edges and middle).
            let open: Vec<usize> = (0..ids.len())
                .filter(|&slot| edge_free(ids[slot], [0, 0], [axis[4], axis[4]]))
                .collect();
            sample(&mut samples, &open, &QUADRANT_PROBES)?;
            let corners: Vec<[Option<Anchor>; 4]> = (open.par_iter())
                .map(|&slot| {
                    let own = (block_of(slot), own_of(slot));
                    corner_anchors(painter, corner_paint, own, ids[slot])
                })
                .collect();
            let corners_of =
                |slot: usize| &corners[open.binary_search(&slot).expect("an open block")];
            let coarse_rest = &COARSE_FIRST[..9];
            let coarser: Vec<usize> = (open.iter().copied())
                .filter(|&slot| {
                    let lattice = Lattice::new(&corner_lines, corners_of(slot));
                    !probes_agree(&lattice, &samples[slot], &positions)
                })
                .collect();
            sample(&mut samples, &coarser, coarse_rest)?;
            let coarse_of = |samples: &[Option<Anchor>]| -> Vec<Option<Anchor>> {
                coarse_rest.iter().map(|&k| samples[k]).collect()
            };
            let mut finer: Vec<usize> =
                (0..ids.len()).filter(|slot| !open.contains(slot)).collect();
            finer.extend((coarser.iter().copied()).filter(|&slot| {
                let coarse = coarse_of(&samples[slot]);
                !probes_agree(
                    &Lattice::new(&coarse_lines, &coarse),
                    &samples[slot],
                    &positions,
                )
            }));
            finer.sort_unstable();
            // What a finer block has not painted yet: all of a block not open, else the fine
            // lattice beside the coarse one.
            let (whole, fine): (Vec<usize>, Vec<usize>) =
                finer.iter().partition(|slot| !open.contains(slot));
            let all: Vec<usize> = (0..positions.len()).collect();
            let rest: Vec<usize> = (all.iter().copied())
                .filter(|k| !COARSE_FIRST.contains(k))
                .collect();
            sample(&mut samples, &whole, &all)?;
            sample(&mut samples, &fine, &rest)?;
            let settled: Vec<QuietSamples> = (0..ids.len())
                .into_par_iter()
                .map(|slot| {
                    let at = ids[slot];
                    let (base_x, base_y) = ((at % side) * b, (at / side) * b);
                    let free = |x: usize, y: usize| {
                        let pixel = (base_y + y) * grid.pixels + base_x + x;
                        own_of(slot).free_field_at_pixel(square, grid.pixels, pixel)
                    };
                    if finer.binary_search(&slot).is_ok() {
                        let patch_edge_free = |cell: usize| {
                            let (i, j) = (cell % 4, cell / 4);
                            edge_free(at, [axis[i], axis[j]], [axis[i + 1], axis[j + 1]])
                        };
                        QuietSamples::patched(b, &samples[slot], &patch_edge_free, &free)
                    } else if coarser.binary_search(&slot).is_ok() {
                        let coarse = coarse_of(&samples[slot]);
                        QuietSamples::blended_whole(b, &Lattice::new(&coarse_lines, &coarse), &free)
                    } else {
                        let lattice = Lattice::new(&corner_lines, corners_of(slot));
                        QuietSamples::blended_whole(b, &lattice, &free)
                    }
                })
                .collect();
            for ((slot, &at), sampled) in ids.iter().enumerate().zip(settled) {
                retain[run.start + slot] = !sampled.complete();
                if sampled.paints_a_pixel() {
                    accepted.push((at, sampled));
                }
            }
        }
        let most_source_counts: Vec<(usize, usize)> = chunk
            .iter()
            .copied()
            .zip(&blocks)
            .map(|(at, block)| (at, block.most))
            .collect();
        let admissions = chunk
            .iter()
            .copied()
            .zip(blocks)
            .zip(retain)
            .filter_map(|(block, retain)| retain.then_some(block))
            .collect::<Vec<_>>();
        Ok((accepted, admissions, most_source_counts))
    })?;
    let mut blocks = Vec::new();
    let mut admissions = Vec::new();
    let mut most_source_counts = Vec::new();
    for (_, (accepted, retained, counts)) in sampled {
        admissions.extend(retained);
        blocks.extend(accepted);
        most_source_counts.extend(counts);
    }
    Ok(SampledBlocks {
        blocks,
        admissions,
        most_source_counts,
    })
}

/// The sample positions painted first (indices of `sample_positions`): the lattice at every second
/// axis line (the block's edges and middle) and the four lattice points between them, each the
/// probe of one quadrant.
const COARSE_FIRST: [usize; 13] = [0, 2, 4, 10, 12, 14, 20, 22, 24, 6, 8, 16, 18];
/// The quadrants' probes among them, painted before all: the probes of the corners' blend too.
const QUADRANT_PROBES: [usize; 4] = [6, 8, 16, 18];

/// Whether a block's four quadrant probes among its `samples` (at `positions`) agree with `lattice`'s
/// blend there, as a patch's probe must: the lattice then paints the block. The corners hear every
/// loud source, and neighbouring blocks share them. (One probe in the middle let a road beside the
/// block bend its field unseen: of 554 popup clicks of three squares 16 instead of 8 were over
/// 1 phon.)
fn probes_agree(
    lattice: &Lattice,
    samples: &[Option<Anchor>],
    positions: &[(usize, usize)],
) -> bool {
    QUADRANT_PROBES.iter().all(|&k| {
        let (x, y) = positions[k];
        samples[k].is_some_and(|probe| lattice.agrees_with(x, y, &probe))
    })
}

/// Block `at`'s four corners (north-west, north-east, south-west, south-east) as the exact points
/// its pixels may blend from, `None` for one standing in a building: its paint, and of it what
/// `block`'s own sources deliver there, as the corner heard them.
fn corner_anchors(
    painter: &PointPainter,
    corner_paint: &[Painted],
    (block, own_sources): (&Block, &OwnSources),
    at: usize,
) -> [Option<Anchor>; 4] {
    let side = painter.grid.pixels / painter.grid.block;
    let first = (at / side) * (side + 1) + at % side;
    [first, first + 1, first + side + 1, first + side + 2].map(|corner| {
        let point = &painter.corners.corner_points[corner];
        (point.own_footprint == 0).then(|| {
            let ranked = &painter.corners.corners[corner].ranked;
            let own: [f64; LAYERS] = std::array::from_fn(|layer| {
                let heard: f64 = (ranked[layer].iter())
                    .filter(|source| block.own.binary_search(&source.index).is_ok())
                    .map(|source| source.lden)
                    .sum();
                heard * point.reflection()
            });
            Anchor {
                paint: corner_paint[corner],
                own,
                free: own_sources.free_field(painter.square, point.position, point.altitude_m),
            }
        })
    })
}

/// Each block's samples `wanted` (indices of `positions`, a block's `sample_positions`), `ids`
/// the blocks' places.
fn paint_positions(
    painter: &PointPainter,
    blocks: (&[&Block], &[&OwnSources]),
    ids: &[usize],
    (wanted, positions): (&[usize], &[(usize, usize)]),
) -> Result<Vec<Vec<Option<Anchor>>>, String> {
    let (square, grid) = (painter.square, painter.grid);
    let (side, b) = (grid.pixels / grid.block, grid.block);
    let places: Vec<_> = ids
        .iter()
        .enumerate()
        .flat_map(|(slot, &at)| {
            let (x, y) = ((at % side) * b, (at / side) * b);
            wanted.iter().map(move |&k| {
                let (dx, dy) = positions[k];
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
    let painted = painter.paint_anchors(blocks, &points, &places)?;
    Ok(painted.chunks(wanted.len()).map(<[_]>::to_vec).collect())
}

#[cfg(test)]
#[path = "block_samples_tests.rs"]
mod tests;
