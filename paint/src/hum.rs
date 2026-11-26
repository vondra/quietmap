//! Quiet ground source energies and spectra from shared importance samples on the hum lattice.
//!
//! The quiet hum of a square: at points every `spacing` pixels, per layer the energy per period of
//! the ground sources not loud there (`popup::candidates::loud`): left out under 0.1 dB of the
//! whole answer, else sampled (`popup::sampling`) to 0.05 dB of it or to what no visitor hears
//! (`crate::hum_samples`), over the loud ones evaluated exactly, and its bands. The selected quiet rays are batched through
//! `crate::hum_samples`; their importance weights apply to spectra, while unweighted energies
//! determine the sample's confidence. A pixel takes it blended from the four points around in
//! decibels; the painter and its etalon take the same.

use crate::exact::{Batch, LAYERS, Point, PointBands, evaluate_all, loud_pairs};
use crate::hum_samples::sampled_hum;
use crate::paint::{bilinear, lattice_points};
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};

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
fn blend_in_decibels_ln(ln_corners: [f64; 4], fx: f64, fy: f64) -> f64 {
    let level = bilinear(ln_corners, fx, fy);
    (level.exp() - HUM_BLEND_FLOOR).max(0.0)
}

#[cfg(test)]
fn blend_in_decibels(corners: [f64; 4], fx: f64, fy: f64) -> f64 {
    blend_in_decibels_ln(
        corners.map(|energy| (energy + HUM_BLEND_FLOOR).ln()),
        fx,
        fy,
    )
}

fn precompute_ln(quiet: &[HumPointSound]) -> Vec<[[f64; PERIODS]; LAYERS]> {
    quiet
        .iter()
        .map(|(energies, _)| {
            energies.map(|layer| layer.map(|energy| (energy + HUM_BLEND_FLOOR).ln()))
        })
        .collect()
}

/// The hum at points every `spacing` of the square's pixels, row-major from the north-west: per
/// layer its energy per period and its received bands at that energy.
pub struct Hum {
    side: usize,
    spacing: usize,
    quiet: Vec<HumPointSound>,
    quiet_ln: Vec<[[f64; PERIODS]; LAYERS]>,
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
            let quiet = vec![([[0.0; PERIODS]; LAYERS], PointBands::default()); side * side];
            let quiet_ln = precompute_ln(&quiet);
            return Ok(Self {
                side,
                spacing,
                quiet,
                quiet_ln,
            });
        }
        let points = lattice_points(square, pixels, spacing)?;
        let pairs = loud_pairs(square, &points);
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
        Self::from_loud(square, pixels, spacing, &points, &loud_energy, batch)
    }

    /// The hum's points and loud energy already computed at the square's block corners.
    pub(crate) fn from_loud(
        square: &Square,
        pixels: usize,
        spacing: usize,
        points: &[Point],
        loud_energy: &[[[f64; PERIODS]; LAYERS]],
        batch: Option<&dyn Batch>,
    ) -> Result<Self, String> {
        let side = pixels / spacing + 1;
        let seeds: Vec<u64> = (0..points.len())
            .map(|at| {
                point_seed(
                    square.tile,
                    pixels,
                    (at % side) * spacing,
                    (at / side) * spacing,
                )
            })
            .collect::<Vec<_>>();
        let quiet = sampled_hum(square, points, loud_energy, &seeds, batch)?;
        let quiet_ln = precompute_ln(&quiet);
        Ok(Hum {
            side,
            spacing,
            quiet,
            quiet_ln,
        })
    }

    /// The hum at the pixel `(x, y)` of the square, blended from the four points around its centre:
    /// per layer its energy per period in decibels ([`blend_in_decibels_ln`]) and its bands, blended
    /// linearly in shape and scaled to that energy.
    pub fn at(&self, x: usize, y: usize) -> ([[f64; PERIODS]; LAYERS], PointBands) {
        let energy = self.energies_at(x, y);
        let bands = self.bands_at(x, y, &energy);
        (energy, bands)
    }

    /// The hum bands at `(x, y)` scaled to already-computed `decibels` energies from `energies_at`.
    pub fn bands_at(&self, x: usize, y: usize, decibels: &[[f64; PERIODS]; LAYERS]) -> PointBands {
        let (indices, fx, fy) = self.neighbour_indices(x, y);
        let corners = indices.map(|at| &self.quiet[at]);
        std::array::from_fn(|layer| {
            std::array::from_fn(|p| {
                let d = decibels[layer][p];
                if d <= 0.0 {
                    return [0.0; physics::bands::BANDS];
                }
                let linear = bilinear(corners.map(|corner| corner.0[layer][p]), fx, fy);
                let scale = if linear > 0.0 { d / linear } else { 0.0 };
                std::array::from_fn(|b| {
                    scale * bilinear(corners.map(|corner| corner.1[layer][p][b]), fx, fy)
                })
            })
        })
    }

    /// The same hum energies as `at`, without constructing the unused spectral bands.
    pub fn energies_at(&self, x: usize, y: usize) -> [[f64; PERIODS]; LAYERS] {
        let (indices, fx, fy) = self.neighbour_indices(x, y);
        std::array::from_fn(|layer| {
            std::array::from_fn(|period| {
                let energies = indices.map(|at| self.quiet[at].0[layer][period]);
                if energies == [0.0; 4] {
                    0.0
                } else {
                    blend_in_decibels_ln(indices.map(|at| self.quiet_ln[at][layer][period]), fx, fy)
                }
            })
        })
    }

    fn neighbour_indices(&self, x: usize, y: usize) -> ([usize; 4], f64, f64) {
        let last = self.side - 2;
        let (i, j) = ((x / self.spacing).min(last), (y / self.spacing).min(last));
        let fx = ((x - i * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let fy = ((y - j * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let indices =
            [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| (j + dj) * self.side + i + di);
        (indices, fx, fy)
    }

    /// Whether the hum stays under 0 dB at every point even with the largest reflection (3 dB):
    /// a bilinear blend is largest at a point.
    pub fn silent(&self) -> bool {
        (self.quiet.iter()).all(|(quiet, _)| 2.0 * quiet.iter().map(lden_energy).sum::<f64>() < 1.0)
    }
}

/// The sample's seed of the hum point at pixel (x, y) of the square `tile`, `pixels` a side: its
/// world coordinates, so that neighbouring squares draw the same sample at a point they share,
/// the two sides of the antimeridian too.
fn point_seed(tile: tiles::geo::TileId, pixels: usize, x: usize, y: usize) -> u64 {
    let world = pixels as u64 * u64::from(tiles::geo::TILES_PER_AXIS);
    let x = (u64::from(tile.x) * pixels as u64 + x as u64) % world;
    let y = u64::from(tile.y) * pixels as u64 + y as u64;
    (x << 32) | y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two squares draw the same sample at a hum point of their shared edge, across the
    /// antimeridian too (there the eastern square's seed was a world further).
    #[test]
    fn neighbouring_squares_seed_a_shared_hum_point_alike() {
        let tile = |x: u32| tiles::geo::TileId { x, y: 1387 };
        let seed = |x: u32, pixel: usize| point_seed(tile(x), 1024, pixel, 64);
        assert_eq!(seed(2212, 1024), seed(2213, 0));
        assert_eq!(seed(4095, 1024), seed(0, 0));
        assert_ne!(seed(2212, 0), seed(2212, 64));
    }

    #[test]
    fn energy_only_background_is_the_original_hum_at_every_pixel() {
        let quiet = (0..9)
            .map(|at| {
                let energies = std::array::from_fn(|layer| {
                    std::array::from_fn(|period| {
                        if at % 3 != 2 {
                            0.0
                        } else {
                            (at + layer + period + 1) as f64 * 123.45
                        }
                    })
                });
                (energies, PointBands::default())
            })
            .collect::<Vec<_>>();
        let quiet_ln = precompute_ln(&quiet);
        let hum = Hum {
            side: 3,
            spacing: 16,
            quiet,
            quiet_ln,
        };
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(hum.energies_at(x, y), hum.at(x, y).0);
            }
        }
    }

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
