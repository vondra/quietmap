//! Quiet ground source energies and spectra from shared importance samples on the hum lattice.

use crate::exact::{Batch, LAYERS, Pair, Point, PointBands, evaluate_all};
use crate::hum_samples::sampled_hum;
use crate::paint::bilinear;
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use rayon::prelude::*;
use tiles::geo::Mercator;

pub(crate) type HumPointSound = ([[f64; PERIODS]; LAYERS], PointBands);

/// The hum at points every `spacing` of the square's pixels, row-major from the north-west: per
/// layer its energy per period and its received bands at that energy.
pub struct Hum {
    side: usize,
    spacing: usize,
    quiet: Vec<HumPointSound>,
}

impl Hum {
    /// The hum of the square (`pixels` a side) at points every `spacing` pixels.
    pub fn new(
        square: &Square,
        pixels: usize,
        spacing: usize,
        batch: Option<&dyn Batch>,
    ) -> Result<Self, String> {
        let side = pixels / spacing + 1;
        let points: Vec<Point> = (0..side * side)
            .into_par_iter()
            .map(|at| {
                let (x, y) = ((at % side) * spacing, (at / side) * spacing);
                Point::at(
                    square,
                    square.frame.to_metres(Mercator {
                        x: f64::from(square.tile.x) + x as f64 / pixels as f64,
                        y: f64::from(square.tile.y) + y as f64 / pixels as f64,
                    }),
                )
            })
            .collect::<Result<_, String>>()?;
        let pairs: Vec<Pair> = (0..points.len())
            .into_par_iter()
            .map_init(Vec::new, |near, k| {
                let point = &points[k];
                square.index.reaching(point.position, point.position, near);
                (near.iter())
                    .filter(|&&index| square.loud(index, point.position, &point.bound))
                    .map(|&index| Pair::new(k as u32, index))
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect();
        let mut loud_energy = vec![[[0.0; PERIODS]; LAYERS]; points.len()];
        for (pair, energy) in pairs
            .iter()
            .zip(evaluate_all(square, &points, &pairs, batch)?.pairs)
        {
            let k = pair.point;
            let layer = square.candidates[pair.candidate as usize].layer as usize;
            for (total, value) in loud_energy[k as usize][layer].iter_mut().zip(energy.mixed) {
                *total += value;
            }
        }
        let seeds: Vec<u64> = (0..points.len())
            .map(|at| {
                // Shared world coordinates give neighbouring squares the same receiver sample.
                let x = u64::from(square.tile.x) * pixels as u64 + ((at % side) * spacing) as u64;
                let y = u64::from(square.tile.y) * pixels as u64 + ((at / side) * spacing) as u64;
                (x << 32) | y
            })
            .collect();
        let quiet = sampled_hum(square, &points, &loud_energy, &seeds, batch)?;
        Ok(Hum {
            side,
            spacing,
            quiet,
        })
    }

    /// The hum at the pixel `(x, y)` of the square, blended from the four points around its centre:
    /// per layer its energy per period and its bands.
    pub fn at(&self, x: usize, y: usize) -> ([[f64; PERIODS]; LAYERS], PointBands) {
        let last = self.side - 2;
        let (i, j) = ((x / self.spacing).min(last), (y / self.spacing).min(last));
        let fx = ((x - i * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let fy = ((y - j * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let corners = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.quiet[(j + dj) * self.side + i + di]);
        let energy = std::array::from_fn(|layer| {
            std::array::from_fn(|p| bilinear(corners.map(|corner| corner.0[layer][p]), fx, fy))
        });
        let bands = std::array::from_fn(|layer| {
            std::array::from_fn(|p| {
                std::array::from_fn(|b| {
                    bilinear(corners.map(|corner| corner.1[layer][p][b]), fx, fy)
                })
            })
        });
        (energy, bands)
    }

    /// Whether the hum stays under 0 dB at every point even with the largest reflection (3 dB):
    /// a bilinear blend is largest at a point.
    pub fn silent(&self) -> bool {
        (self.quiet.iter()).all(|(quiet, _)| 2.0 * quiet.iter().map(lden_energy).sum::<f64>() < 1.0)
    }
}
