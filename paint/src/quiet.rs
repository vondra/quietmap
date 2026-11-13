//! Background blocks and checked sub-block fields expanded from shared loudness and exact pixel probes.
//!
//! The obstacle proof precedes pixel receivers, as dev1's quiet-region shortcut precedes rasters.
//! Background blocks blend loudness at corners and evaluate their original energy fields. Source
//! blocks check actual pixel receivers against their corners' paint, then against sampled
//! lattices; only blocks and patches away from sources and walls that agree omit work.
//! Source blocks retain their admissions; sampled pixel fields are evaluated in bounded chunks.

use crate::block_samples::sample_block_pixels;
use crate::blocks::Block;
use crate::exact::LAYERS;
use crate::flights::blend_flight_energy;
use crate::levels::Painted;
use crate::levels::{PaintedCells, layer_energy};
use crate::paint::{corner_weights, rectangle_at};
use crate::points::PointPainter;
use crate::quiet_corners::paint_corners;
use crate::quiet_samples::QuietSamples;
use rayon::prelude::*;
use std::borrow::Cow;

pub(crate) struct QuietBlocks {
    pub known_blocks: Vec<bool>,
    admissions: Vec<Option<Block>>,
    pub most_source_counts: Vec<usize>,
    corner_paint: Vec<Painted>,
    samples: Vec<Option<QuietSamples>>,
}

impl QuietBlocks {
    pub fn new(painter: &PointPainter) -> Result<Self, String> {
        let (square, grid) = (painter.square, painter.grid);
        let side = grid.pixels / grid.block;
        let candidates = (0..side * side)
            .into_par_iter()
            .map_init(Vec::new, |near, at| {
                let (most, leaves_out) = painter.block_admission(near, at);
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
                    most == 0 && !leaves_out && square.obstacles.is_open_rectangle(low, high),
                    most,
                )
            })
            .collect::<Vec<_>>();
        let corner_paint = paint_corners(painter)?;
        let mut quiet = Self {
            known_blocks: vec![false; candidates.len()],
            admissions: (0..candidates.len()).map(|_| None).collect(),
            most_source_counts: candidates.iter().map(|&(_, count)| count).collect(),
            corner_paint,
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
        let sampled = sample_block_pixels(painter, block_ids, &quiet.corner_paint)?;
        for (at, block) in sampled.admissions {
            quiet.admissions[at] = Some(block);
        }
        let (mut whole, mut blended, mut carried) = (0, 0, 0);
        for (at, samples) in sampled.blocks {
            let patches = samples.patches();
            whole += usize::from(samples.complete());
            blended += patches.0;
            carried += patches.1;
            quiet.known_blocks[at] = samples.complete();
            quiet.samples[at] = Some(samples);
        }
        let patches = 16 * (candidate_count - whole);
        eprintln!(
            "qm-paint: {background} open background blocks, {whole} of {candidate_count} source blocks blended whole; of the others' {patches} patches {} blended, {carried} with carried loudness",
            blended - 16 * whole
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

    /// The loudness the pixel `at` takes from its block's lattice and its own `energies`, where its
    /// patch carries it (`QuietSamples::loudness_from`).
    pub fn loudness_from(
        &self,
        painter: &PointPainter,
        at: usize,
        energies: &[f64; LAYERS],
    ) -> Option<[f64; LAYERS + 1]> {
        let grid = painter.grid;
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        (self.samples[block].as_ref())
            .and_then(|samples| samples.loudness_from(x % grid.block, y % grid.block, energies))
    }

    /// A known pixel: sampled source fields, or background Nden corners and the original energy grids.
    /// The open proof keeps facade ranking out of the background path.
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
                .map(|corner| weights[corner] * self.corner_paint[around[corner]].1[layer])
                .sum()
        });
        // The open-rectangle proof excludes reflection throughout the receiver windows.
        let steady = painter.corners.hum.energies_at(x, y);
        let flights = around.map(|at| &painter.corners.flights[at]);
        let flights = blend_flight_energy(flights, fx, fy);
        (
            layer_energy(painter.square, &steady, &flights, 1.0, &[], &[]),
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
