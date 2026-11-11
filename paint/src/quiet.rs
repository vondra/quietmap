//! Background blocks and checked sub-block fields expanded from shared loudness and exact pixel probes.
//!
//! The obstacle proof precedes pixel receivers, as dev1's quiet-region shortcut precedes rasters.
//! Background blocks blend loudness at corners and evaluate their original energy fields. Source blocks check actual
//! pixel receivers against sampled loudness; only patches away from sources and walls that agree
//! omit work.
//! Source blocks retain their admissions; sampled pixel fields are evaluated in bounded chunks.

use crate::blocks::Block;
use crate::exact::LAYERS;
use crate::flights::blend_flights;
use crate::levels::Painted;
use crate::levels::{PaintedCells, layer_energy};
use crate::paint::{corner_weights, rectangle_at};
use crate::points::PointPainter;
use crate::quiet_corners::loudness_at_corners;
use crate::quiet_samples::{QuietSamples, sample_block_pixels};
use rayon::prelude::*;
use std::borrow::Cow;

pub(crate) struct QuietBlocks {
    pub known_blocks: Vec<bool>,
    admissions: Vec<Option<Block>>,
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
            known_blocks: vec![false; candidates.len()],
            admissions: (0..candidates.len()).map(|_| None).collect(),
            exact_source_counts: candidates.iter().map(|&(_, count)| count).collect(),
            loudness,
            samples: (0..candidates.len()).map(|_| None).collect(),
        };
        let mut block_ids = Vec::new();
        for (at, (background, _)) in candidates.into_iter().enumerate() {
            if background {
                quiet.known_blocks[at] = true;
            } else {
                block_ids.push(at);
            }
        }
        let background = quiet.known_blocks.iter().filter(|&&quiet| quiet).count();
        let candidate_count = block_ids.len();
        let sampled = sample_block_pixels(painter, block_ids)?;
        for (at, block) in sampled.admissions {
            quiet.admissions[at] = Some(block);
        }
        let sampled = sampled.blocks;
        let sampled_count = sampled.len();
        let interpolated_patches: usize = sampled
            .iter()
            .map(|(_, samples)| samples.interpolated_patches())
            .sum();
        for (at, samples) in sampled {
            quiet.known_blocks[at] = samples.complete();
            quiet.samples[at] = Some(samples);
        }
        eprintln!(
            "qm-paint: {background} open background blocks, {sampled_count}/{candidate_count} source blocks sampled, {interpolated_patches} checked patches interpolated"
        );
        Ok(quiet)
    }

    /// Rejected patches and façade passes share their already computed admissions.
    pub fn block<'a>(
        &'a self,
        painter: &PointPainter,
        near: &mut Vec<u32>,
        at: usize,
    ) -> Cow<'a, Block> {
        self.admissions[at]
            .as_ref()
            .map_or_else(|| Cow::Owned(painter.block(near, at)), Cow::Borrowed)
    }

    pub fn has_paint(&self, painter: &PointPainter, at: usize) -> bool {
        let grid = painter.grid;
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        self.known_blocks[block]
            || self.samples[block]
                .as_ref()
                .is_some_and(|samples| samples.has_paint(x % grid.block, y % grid.block))
    }

    /// A known pixel: sampled source fields, or background Nden corners and the original energy grids.
    pub fn paint_at(&self, painter: &PointPainter, at: usize) -> Painted {
        let grid = painter.grid;
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        if let Some(samples) = &self.samples[block] {
            return samples.known_at(x % grid.block, y % grid.block).unwrap();
        }
        let side = grid.pixels / grid.block + 1;
        let first = (y / grid.block) * side + x / grid.block;
        let around = [first, first + 1, first + side, first + side + 1];
        let fx = ((x % grid.block) as f64 + 0.5) / grid.block as f64;
        let fy = ((y % grid.block) as f64 + 0.5) / grid.block as f64;
        let weights = corner_weights(fx, fy);
        let ndens = std::array::from_fn(|layer| {
            (0..4)
                .map(|corner| weights[corner] * self.loudness[around[corner]][layer])
                .sum()
        });
        // The open-rectangle proof excludes reflection throughout the receiver windows.
        let steady = painter.corners.hum.energies_at(x, y);
        let flights = blend_flights(around.map(|at| painter.corners.flights[at]), fx, fy);
        (
            layer_energy(painter.square, &steady, &flights.energy, 1.0, &[], &[]),
            ndens,
        )
    }

    pub fn cells(&self, painter: &PointPainter) -> Result<PaintedCells, String> {
        PaintedCells::from_painted(
            (0..painter.grid.pixels * painter.grid.pixels)
                .into_par_iter()
                .map(|at| Some(self.paint_at(painter, at))),
        )
    }
}
