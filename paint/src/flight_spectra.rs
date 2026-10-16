//! Aircraft spectral shares sampled only where they stay smooth, with exact fine-grid centres and borders.
//!
//! The popup's loudest flight pieces give normalized unweighted third-octave energy in the
//! receiver's air. Every second block corner stays a spectral node. Alternating nodes and cell
//! centres are evaluated first; other nodes blend only where those five spectra vary little and
//! the centre agrees. Undefined spectra and square borders retain their exact computation.
//! The stored lattice and its pixel interpolation are unchanged; the etalon shares these spectra,
//! so it does not test their sampling.

use crate::exact::Point;
use crate::loudest::LoudestIndex;
use crate::paint::bilinear;
use crate::square::Square;
use physics::doc29::atmosphere::{THIRD_OCTAVES as CLASS_THIRDS, place_rates_db_per_m};
use popup::aircraft::flights::{FlightTotals, heard_spectrum_energy};
use rayon::prelude::*;

/// The flights' spectrum over a square: at points every `spacing` pixels (every second block
/// corner, where the near flights are summed: a low light aircraft shows within a few hundred
/// metres) the loudest flights' spectral classes at their slants in the place's air by their
/// exposure, as a click there takes them (`popup::aircraft::flights::heard_spectrum_energy`), each as
/// shares of its unweighted third-octave energy.
pub struct FlightSpectra {
    side: usize,
    spacing: usize,
    shares: Vec<[f64; CLASS_THIRDS]>,
}

impl FlightSpectra {
    pub fn new(square: &Square, pixels: usize, spacing: usize) -> Result<Self, String> {
        let side = pixels / spacing + 1;
        if !square.has_aircraft_boxes() {
            return Ok(Self {
                side,
                spacing,
                shares: vec![[f64::NAN; CLASS_THIRDS]; side * side],
            });
        }
        let points: Vec<Point> = (0..side * side)
            .into_par_iter()
            .map(|at| {
                let (x, y) = ((at % side) * spacing, (at / side) * spacing);
                let position = square.frame.to_metres(tiles::geo::Mercator {
                    x: f64::from(square.tile.x) + x as f64 / pixels as f64,
                    y: f64::from(square.tile.y) + y as f64 / pixels as f64,
                });
                Point::at(square, position)
            })
            .collect::<Result<_, String>>()?;
        let loudest = LoudestIndex::new(square, &points);
        let spectrum_of = |point: &Point| -> Result<[f64; CLASS_THIRDS], String> {
            let weather = point.weather(square);
            let mut flights = FlightTotals::default().in_atmosphere(&weather.alpha_db_per_km);
            loudest.search(square, point, &mut flights)?;
            let energy = heard_spectrum_energy(
                &flights.loudest(),
                &place_rates_db_per_m(&weather.alpha_db_per_km),
            );
            let sum: f64 = energy.iter().sum();
            Ok(energy.map(|value| value / sum))
        };
        let coarse = points
            .par_iter()
            .enumerate()
            .map(|(at, point)| {
                if at % side % 2 == at / side % 2 {
                    spectrum_of(point)
                } else {
                    Ok([f64::NAN; CLASS_THIRDS])
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let shares = points
            .par_iter()
            .enumerate()
            .map(|(at, point)| {
                let (x, y) = (at % side, at / side);
                if x % 2 == y % 2 {
                    return Ok(coarse[at]);
                }
                if x == 0 || y == 0 || x + 1 == side || y + 1 == side {
                    return spectrum_of(point);
                }
                let (i, j) = ((x / 2) * 2, (y / 2) * 2);
                let around = [(0, 0), (2, 0), (0, 2), (2, 2)]
                    .map(|(dx, dy)| coarse[(j + dy) * side + i + dx]);
                let centre = coarse[(j + 1) * side + i + 1];
                let finite = around
                    .iter()
                    .chain(std::iter::once(&centre))
                    .all(|spectrum| spectrum.iter().all(|value| value.is_finite()));
                let spread: f64 = (0..CLASS_THIRDS)
                    .map(|band| {
                        let (low, high) = around
                            .iter()
                            .chain(std::iter::once(&centre))
                            .map(|spectrum| spectrum[band])
                            .fold((f64::INFINITY, 0.0f64), |(low, high), value| {
                                (low.min(value), high.max(value))
                            });
                        high - low
                    })
                    .sum();
                let curvature: f64 = (0..CLASS_THIRDS)
                    .map(|band| {
                        (centre[band] - bilinear(around.map(|spectrum| spectrum[band]), 0.5, 0.5))
                            .abs()
                    })
                    .sum();
                // The centre check catches changes missed by the corners. These share tolerances
                // retain the aircraft layer's drift against the independent fine-grid reference.
                let smooth = finite && spread <= 0.02 && curvature <= 0.005;
                if !smooth {
                    return spectrum_of(point);
                }
                Ok(std::array::from_fn(|band| {
                    bilinear(
                        around.map(|spectrum| spectrum[band]),
                        (x - i) as f64 / 2.0,
                        (y - j) as f64 / 2.0,
                    )
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(FlightSpectra {
            side,
            spacing,
            shares,
        })
    }

    /// The spectrum (dB up to a constant) at the pixel `(x, y)`, blended from the four points
    /// around its centre.
    pub fn at(&self, x: usize, y: usize) -> [f64; CLASS_THIRDS] {
        let last = self.side - 2;
        let (i, j) = ((x / self.spacing).min(last), (y / self.spacing).min(last));
        let fx = ((x - i * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let fy = ((y - j * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let corners = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.shares[(j + dj) * self.side + i + di]);
        std::array::from_fn(|n| 10.0 * bilinear(corners.map(|corner| corner[n]), fx, fy).log10())
    }
}
