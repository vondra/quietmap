//! Quiet ground source energies and spectra from shared importance samples on the hum lattice.
//!
//! The quiet hum of a square: at points every `spacing` pixels, per layer the energy per period of
//! the ground sources not loud there (`popup::candidates::loud`), with the click's error budget:
//! left out under 0.1 dB of the whole answer, else sampled to 0.05 dB (`popup::sampling`), over the
//! loud ones evaluated exactly, and its bands. The selected quiet rays are batched through
//! `crate::hum_samples`; their importance weights apply to spectra, while unweighted energies
//! determine the sample's confidence. A pixel takes it blended from the four points around in
//! decibels; the painter and its etalon take the same.

use crate::exact::{Batch, LAYERS, Pair, Point, PointBands, evaluate_all};
use crate::hum_samples::sampled_hum;
use crate::paint::bilinear;
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use rayon::prelude::*;
use tiles::geo::Mercator;

pub(crate) type HumPointSound = ([[f64; PERIODS]; LAYERS], PointBands);

/// The energy (0 dB) under which the hum's blend turns linear, so a silent point blends: a hum under
/// it is not heard (`Hum::silent`).
const HUM_BLEND_FLOOR: f64 = 1.0;

/// The hum between its points blended in decibels: bilinear in the energy's logarithm, above
/// [`HUM_BLEND_FLOOR`]. It falls off steeply from the quiet sources near one point, and a linear
/// blend read a quiet pixel by that point's level: at 474 random pixels of 40 Czech squares the
/// linear blend exceeded the hum computed at the pixel by +0.42 dB on average for the roads (+0.59
/// to +0.98 for railways, industry and buildings), this blend by +0.02 (+0.39 to +0.70, much of it
/// under the floor); against the popup, places under 2 sone +0.58 -> +0.49 phon (2026-10-14).
fn blend_in_decibels(corners: [f64; 4], fx: f64, fy: f64) -> f64 {
    let level = bilinear(
        corners.map(|energy| (energy + HUM_BLEND_FLOOR).ln()),
        fx,
        fy,
    );
    (level.exp() - HUM_BLEND_FLOOR).max(0.0)
}

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
        if square.candidates.is_empty() {
            return Ok(Self {
                side,
                spacing,
                quiet: vec![([[0.0; PERIODS]; LAYERS], PointBands::default()); side * side],
            });
        }
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
    /// per layer its energy per period in decibels ([`blend_in_decibels`]) and its bands, blended
    /// linearly in shape and scaled to that energy.
    pub fn at(&self, x: usize, y: usize) -> ([[f64; PERIODS]; LAYERS], PointBands) {
        let (corners, fx, fy) = self.neighbours(x, y);
        let blended: [[(f64, f64); PERIODS]; LAYERS] = std::array::from_fn(|layer| {
            std::array::from_fn(|p| {
                let energies = corners.map(|corner| corner.0[layer][p]);
                (
                    blend_in_decibels(energies, fx, fy),
                    bilinear(energies, fx, fy),
                )
            })
        });
        let energy = blended.map(|layer| layer.map(|(decibels, _)| decibels));
        let bands = std::array::from_fn(|layer| {
            std::array::from_fn(|p| {
                let (decibels, linear) = blended[layer][p];
                let scale = if linear > 0.0 { decibels / linear } else { 0.0 };
                std::array::from_fn(|b| {
                    scale * bilinear(corners.map(|corner| corner.1[layer][p][b]), fx, fy)
                })
            })
        });
        (energy, bands)
    }

    fn neighbours(&self, x: usize, y: usize) -> ([&HumPointSound; 4], f64, f64) {
        let last = self.side - 2;
        let (i, j) = ((x / self.spacing).min(last), (y / self.spacing).min(last));
        let fx = ((x - i * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let fy = ((y - j * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let corners = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.quiet[(j + dj) * self.side + i + di]);
        (corners, fx, fy)
    }

    /// Whether the hum stays under 0 dB at every point even with the largest reflection (3 dB):
    /// a bilinear blend is largest at a point.
    pub fn silent(&self) -> bool {
        (self.quiet.iter()).all(|(quiet, _)| 2.0 * quiet.iter().map(lden_energy).sum::<f64>() < 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point near quiet sources does not lend a pixel between it and silent points its level: the
    /// blend is exact at the points, keeps an even hum, and amid one point of 40 dB and three silent
    /// ones reads 9.5 dB, where a linear blend read 34.
    #[test]
    fn the_hum_blends_in_decibels_not_in_energy() {
        let loud = 10_000.0;
        assert!((blend_in_decibels([loud, 1.0, 1.0, 1.0], 0.0, 0.0) - loud).abs() < 1e-6);
        assert!((blend_in_decibels([50.0; 4], 0.3, 0.7) - 50.0).abs() < 1e-9);
        let halfway = blend_in_decibels([loud, 0.0, 0.0, 0.0], 0.5, 0.5);
        assert!((10.0 * halfway.log10() - 9.54).abs() < 0.01, "{halfway}");
        assert!(10.0 * bilinear([loud, 0.0, 0.0, 0.0], 0.5, 0.5).log10() > 33.9);
        assert_eq!(blend_in_decibels([0.0; 4], 0.5, 0.5), 0.0);
    }
}
