//! Background blocks and checked sub-block fields expanded from shared loudness and exact pixel probes.
//!
//! The obstacle proof precedes pixel receivers, as dev1's quiet-region shortcut precedes rasters.
//! Background blocks reuse the existing corner blend, loudness only. Source blocks check actual
//! pixel receivers against sampled loudness; only patches away from sources and walls that agree
//! omit work.
//! Candidate source lists live in bounded chunks, while the retained lattice holds only sound.

use crate::exact::LAYERS;
use crate::paint::{Cells, Painted, cells_of, corner_weights, rectangle_at};
use crate::points::PointPainter;
use crate::quiet_corners::loudness_at_corners;
use crate::quiet_samples::{QuietSamples, sample_block_pixels};
use rayon::prelude::*;

pub(crate) struct QuietBlocks {
    pub blocks: Vec<bool>,
    pub exact_source_counts: Vec<usize>,
    loudness: Vec<[f64; LAYERS + 1]>,
    samples: Vec<Option<QuietSamples>>,
}

impl QuietBlocks {
    pub fn new(painter: &PointPainter) -> Result<Self, String> {
        let (square, grid) = (painter.square, painter.grid);
        let side = grid.pixels / grid.block;
        let candidates = (0..side * side)
            .into_par_iter()
            .map_init(Vec::new, |near, at| {
                let (exact, leaves_out) = painter.block_admission(near, at);
                let (x, y) = ((at % side) * grid.block, (at / side) * grid.block);
                let (low, high) = rectangle_at(
                    square,
                    grid.pixels,
                    x as f64,
                    y as f64,
                    (x + grid.block) as f64,
                    (y + grid.block) as f64,
                );
                (
                    exact == 0 && !leaves_out && square.obstacles.is_open_rectangle(low, high),
                    exact,
                )
            })
            .collect::<Vec<_>>();
        let corner_side = side + 1;
        let mut needed = vec![false; corner_side * corner_side];
        for (at, &(background, _)) in candidates.iter().enumerate() {
            if background {
                let first = (at / side) * corner_side + at % side;
                for corner in [
                    first,
                    first + 1,
                    first + corner_side,
                    first + corner_side + 1,
                ] {
                    needed[corner] = true;
                }
            }
        }
        let loudness = loudness_at_corners(painter, needed)?;
        let mut quiet = Self {
            blocks: vec![false; candidates.len()],
            exact_source_counts: candidates.iter().map(|&(_, count)| count).collect(),
            loudness,
            samples: (0..candidates.len()).map(|_| None).collect(),
        };
        let mut block_ids = Vec::new();
        for (at, (background, _)) in candidates.into_iter().enumerate() {
            if background {
                quiet.blocks[at] = true;
            } else {
                block_ids.push(at);
            }
        }
        let background = quiet.blocks.iter().filter(|&&quiet| quiet).count();
        let candidate_count = block_ids.len();
        let sampled = sample_block_pixels(painter, block_ids)?;
        let sampled_count = sampled.len();
        let interpolated_patches: usize = sampled
            .iter()
            .map(|(_, samples)| samples.interpolated_patches())
            .sum();
        for (at, samples) in sampled {
            quiet.blocks[at] = samples.complete();
            quiet.samples[at] = Some(samples);
        }
        eprintln!(
            "qm-paint: {background} open background blocks, {sampled_count}/{candidate_count} source blocks sampled, {interpolated_patches} checked patches interpolated"
        );
        Ok(quiet)
    }

    pub fn has_paint(&self, painter: &PointPainter, at: usize) -> bool {
        let grid = painter.grid;
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        self.blocks[block]
            || self.samples[block]
                .as_ref()
                .is_some_and(|samples| samples.has_paint(x % grid.block, y % grid.block))
    }

    /// A pixel's loudness where it `has_paint`: its block's samples, else the background corners'
    /// blend, without the energies a façade ranks by (the open proof keeps façades out of it).
    pub fn loudness_at(&self, painter: &PointPainter, at: usize) -> [f64; LAYERS + 1] {
        let grid = painter.grid;
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        if let Some(samples) = &self.samples[block] {
            return samples.known_at(x % grid.block, y % grid.block).unwrap().1;
        }
        let side = grid.pixels / grid.block + 1;
        let first = (y / grid.block) * side + x / grid.block;
        let around = [first, first + 1, first + side, first + side + 1];
        let fx = ((x % grid.block) as f64 + 0.5) / grid.block as f64;
        let fy = ((y % grid.block) as f64 + 0.5) / grid.block as f64;
        let weights = corner_weights(fx, fy);
        std::array::from_fn(|layer| {
            (0..4)
                .map(|corner| weights[corner] * self.loudness[around[corner]][layer])
                .sum()
        })
    }

    pub fn cells(&self, painter: &PointPainter) -> Cells {
        (0..painter.grid.pixels * painter.grid.pixels)
            .into_par_iter()
            .map(|at| cells_of(&self.loudness_at(painter, at)))
            .collect()
    }

    /// The sampled blocks' pixels with their energies, which a façade in front of one ranks by.
    pub fn fill(&self, painter: &PointPainter, pixels: &mut [Option<Painted>]) {
        let grid = painter.grid;
        pixels.par_iter_mut().enumerate().for_each(|(at, pixel)| {
            let (x, y) = (at % grid.pixels, at / grid.pixels);
            let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
            if let Some(paint) = (self.samples[block].as_ref())
                .and_then(|samples| samples.known_at(x % grid.block, y % grid.block))
            {
                *pixel = Some(paint);
            }
        });
    }
}
