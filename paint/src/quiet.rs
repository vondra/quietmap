//! Open background and source blocks expanded from shared loudness after interior accuracy probes.
//!
//! The obstacle proof precedes pixel receivers, as dev1's quiet-region shortcut precedes rasters.
//! Background blocks reuse the existing corner blend. Open blocks without a crossing source
//! first check actual pixel receivers against sampled loudness; only those that agree omit work.
//! Candidate source lists live in bounded chunks, while the retained lattice holds only sound.

use crate::blocks::meets;
use crate::exact::LAYERS;
use crate::flights::blend_flights;
use crate::paint::{Cells, Painted, cells_of, corner_weights, rectangle_at};
use crate::points::PointPainter;
use crate::quiet_corners::loudness_at_corners;
use crate::quiet_samples::{QuietSamples, sample_open_blocks};
use physics::bands::lden_energy;
use rayon::prelude::*;
use tiles::sources::Layer;

pub(crate) struct QuietBlocks {
    pub blocks: Vec<bool>,
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
                let block = painter.block(near, at);
                let (x, y) = ((at % side) * grid.block, (at / side) * grid.block);
                let (low, high) = rectangle_at(
                    square,
                    grid.pixels,
                    x as f64,
                    y as f64,
                    (x + grid.block) as f64,
                    (y + grid.block) as f64,
                );
                if block.exact.iter().any(|&index| {
                    let [a, b] = square.candidates[index as usize].ends_m;
                    meets(a, b, low, high)
                }) {
                    return None;
                }
                square
                    .obstacles
                    .is_open_rectangle(low, high)
                    .then_some(block.exact.is_empty() && block.left_groups.is_empty())
            })
            .collect::<Vec<_>>();
        let corner_side = side + 1;
        let mut needed = vec![false; corner_side * corner_side];
        for (at, candidate) in candidates.iter().enumerate() {
            if *candidate == Some(true) {
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
            loudness,
            samples: (0..candidates.len()).map(|_| None).collect(),
        };
        let mut block_ids = Vec::new();
        for (at, candidate) in candidates.into_iter().enumerate() {
            match candidate {
                Some(true) => quiet.blocks[at] = true,
                Some(false) => block_ids.push(at),
                None => {}
            }
        }
        let background = quiet.blocks.iter().filter(|&&quiet| quiet).count();
        let candidate_count = block_ids.len();
        let sampled = sample_open_blocks(painter, block_ids)?;
        let interpolated = sampled.len();
        for (at, samples) in sampled {
            quiet.blocks[at] = true;
            quiet.samples[at] = Some(samples);
        }
        eprintln!(
            "qm-paint: {background} open background blocks, {interpolated}/{candidate_count} blocks with ground sources interpolated"
        );
        Ok(quiet)
    }

    pub fn at(&self, painter: &PointPainter, at: usize) -> Painted {
        let (grid, corners) = (painter.grid, &painter.corners);
        let (x, y) = (at % grid.pixels, at / grid.pixels);
        let block = (y / grid.block) * (grid.pixels / grid.block) + x / grid.block;
        if let Some(samples) = &self.samples[block] {
            return samples.at(x % grid.block, y % grid.block);
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
        let mut energies = corners
            .hum
            .energies_at(x, y)
            .map(|energy| lden_energy(&energy));
        energies[Layer::Aircraft as usize] +=
            lden_energy(&blend_flights(around.map(|corner| corners.flights[corner]), fx, fy)[0]);
        (energies, ndens)
    }

    pub fn cells(&self, painter: &PointPainter) -> Cells {
        (0..painter.grid.pixels * painter.grid.pixels)
            .into_par_iter()
            .map(|at| cells_of(&self.at(painter, at).1))
            .collect()
    }

    pub fn fill(&self, painter: &PointPainter, pixels: &mut [Option<Painted>]) {
        let grid = painter.grid;
        let side = grid.pixels / grid.block;
        pixels.par_iter_mut().enumerate().for_each(|(at, pixel)| {
            let block = (at / grid.pixels / grid.block) * side + (at % grid.pixels) / grid.block;
            if self.blocks[block] {
                *pixel = Some(self.at(painter, at));
            }
        });
    }
}
