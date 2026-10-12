//! The flights at a square's block corners: the aircraft boxes as the popup reads them for a
//! click, the far ones at the hum's points and the near ones every second block corner, each
//! interpolated where it changes slowly; and their spectrum, the loudest flight's at the hum's
//! points, blended between them.

use crate::exact::Point;
use crate::paint::{Grid, bilinear};
use crate::square::{Square, gap_to_tile};
use physics::bands::{PERIODS, lden_energy};
use physics::doc29::atmosphere::{THIRD_OCTAVES as CLASS_THIRDS, place_rates_db_per_m};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver, tile_flight_sums};
use popup::aircraft::flights::{FlightTotals, heard_spectrum};
use popup::aircraft::horizons::Horizons;
use popup::aircraft::loudest_flights;
use rayon::prelude::*;
use tiles::aircraft::Aircraft;
use tiles::geo::TileId;

/// The flights heard at a point: their energy and their energy times Kurze's lambda per period
/// (both add up over boxes, and blend between points; their ratio is the flights' line's lambda).
pub type FlightSums = [[f64; PERIODS]; 2];

/// Blends four points' flights (north-west, north-east, south-west, south-east) bilinearly.
pub(crate) fn blend_flights(corners: [FlightSums; 4], fx: f64, fy: f64) -> FlightSums {
    std::array::from_fn(|kind| {
        std::array::from_fn(|p| bilinear(corners.map(|c| c[kind][p]), fx, fy))
    })
}

/// The aircraft's near share is evaluated every second block corner, and at a block corner
/// between them where the four around differ by more than this ratio (1 dB).
const AIR_SMOOTH: f64 = 1.258_925_411_794_167_2;

/// The aircraft tiles a click at `position` reads within `within_m`: fine boxes from tiles within
/// their distance of it, else the far ones.
fn tiles_heard<'a>(
    square: &Square<'a>,
    position: [f64; 2],
    within_m: f64,
) -> Vec<(TileId, Aircraft<'a>)> {
    (square.aircraft.iter())
        .filter_map(|tile| {
            let gap = gap_to_tile(&square.frame, tile.tile, position);
            if gap > within_m {
                return None;
            }
            let boxes = if gap <= FINE_BOXES_WITHIN_M {
                tile.fine.as_ref().or(tile.far.as_ref())
            } else {
                tile.far.as_ref()
            };
            boxes.map(|boxes| (tile.tile, *boxes))
        })
        .collect()
}

/// The flights' spectrum over a square: at points every `spacing` pixels (every second block
/// corner, where the near flights are summed: a low light aircraft shows within a few hundred
/// metres) the loudest flights' spectral classes at their slants in the place's air by their
/// exposure, as a click there takes them (`popup::aircraft::flights::heard_spectrum`), each as
/// shares of its unweighted third-octave energy.
pub struct FlightSpectra {
    side: usize,
    spacing: usize,
    shares: Vec<[f64; CLASS_THIRDS]>,
}

impl FlightSpectra {
    pub fn new(square: &Square, pixels: usize, spacing: usize) -> Result<Self, String> {
        let side = pixels / spacing + 1;
        let shares = (0..side * side)
            .into_par_iter()
            .map(|at| {
                let (x, y) = ((at % side) * spacing, (at / side) * spacing);
                let position = square.frame.to_metres(tiles::geo::Mercator {
                    x: f64::from(square.tile.x) + x as f64 / pixels as f64,
                    y: f64::from(square.tile.y) + y as f64 / pixels as f64,
                });
                let point = Point::at(square, position)?;
                let weather = point.weather(square);
                let horizons = Horizons::build(
                    &square.ground,
                    &square.obstacles,
                    point.position,
                    point.altitude_m,
                    point.own_footprint,
                )?;
                let receiver = AircraftReceiver {
                    position: point.position,
                    altitude_m: point.altitude_m,
                };
                let mut flights = FlightTotals::default().in_atmosphere(&weather.alpha_db_per_km);
                let tiles = tiles_heard(square, point.position, AIRCRAFT_REACH_M);
                loudest_flights(&tiles, &square.frame, receiver, &horizons, &mut flights);
                let spectrum = heard_spectrum(
                    &flights.loudest(),
                    &place_rates_db_per_m(&weather.alpha_db_per_km),
                );
                let energy = spectrum.map(|db| 10f64.powf(db / 10.0));
                let sum: f64 = energy.iter().sum();
                Ok(energy.map(|value| value / sum))
            })
            .collect::<Result<_, String>>()?;
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

/// The flights at `point`, each box's share `weight` of its distance: every box within `within_m`,
/// fine boxes from tiles within their distance of the point, as the popup reads them for a click
/// there.
pub(crate) fn aircraft_sums(
    square: &Square,
    point: &Point,
    weight: &dyn Fn(f64) -> f64,
    within_m: f64,
) -> Result<FlightSums, String> {
    let horizons = Horizons::build(
        &square.ground,
        &square.obstacles,
        point.position,
        point.altitude_m,
        point.own_footprint,
    )?;
    let receiver = AircraftReceiver {
        position: point.position,
        altitude_m: point.altitude_m,
    };
    let mut total = [[0.0; PERIODS]; 2];
    for (tile, boxes) in tiles_heard(square, point.position, within_m) {
        let sums = tile_flight_sums(
            &boxes,
            tile,
            &square.frame,
            receiver,
            &horizons,
            (weight, within_m),
        );
        for (totals, values) in total.iter_mut().zip([sums.0, sums.1]) {
            for (sum, value) in totals.iter_mut().zip(values) {
                *sum += value;
            }
        }
    }
    Ok(total)
}

/// The flights at every block corner: the boxes beyond 1.5-3 coarse cells at the coarse corners and
/// the nearer ones every second block corner, each interpolated where it changes slowly.
pub(crate) fn flights_at_corners(
    square: &Square,
    grid: Grid,
    metres: &(dyn Fn(f64, f64) -> [f64; 2] + Sync),
    corners: &[Point],
) -> Result<Vec<FlightSums>, String> {
    if (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none()) {
        return Ok(vec![[[0.0; PERIODS]; 2]; corners.len()]);
    }
    let coarse_m = square.frame.east_m_per_unit / grid.pixels as f64 * grid.hum as f64;
    let far = |distance_m: f64| {
        let t = ((distance_m - 1.5 * coarse_m) / (1.5 * coarse_m)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let near_within_m = 3.0 * coarse_m;
    let lattice = |spacing: usize, far_part: bool| -> Result<(usize, Vec<FlightSums>), String> {
        let side = grid.pixels / spacing + 1;
        let values = (0..side * side)
            .into_par_iter()
            .map(|at| {
                let position = metres(
                    ((at % side) * spacing) as f64,
                    ((at / side) * spacing) as f64,
                );
                let point = Point::at(square, position)?;
                if far_part {
                    aircraft_sums(square, &point, &far, AIRCRAFT_REACH_M)
                } else {
                    aircraft_sums(square, &point, &|d| 1.0 - far(d), near_within_m)
                }
            })
            .collect::<Result<_, String>>()?;
        Ok((side, values))
    };
    let (coarse_side, coarse) = lattice(grid.hum, true)?;
    let (air_side, air) = lattice(2 * grid.block, false)?;
    let corner_side = grid.pixels / grid.block + 1;
    let ratio = grid.hum / grid.block;
    let blend = |values: &[FlightSums], side: usize, step: usize, i: usize, j: usize| {
        let (ci, cj) = ((i / step).min(side - 2), (j / step).min(side - 2));
        let around =
            [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| values[(cj + dj) * side + ci + di]);
        let (fx, fy) = (
            (i - ci * step) as f64 / step as f64,
            (j - cj * step) as f64 / step as f64,
        );
        (blend_flights(around, fx, fy), around)
    };
    (corners.par_iter().enumerate())
        .map(|(at, point)| {
            let (i, j) = (at % corner_side, at / corner_side);
            let (far_energy, _) = blend(&coarse, coarse_side, ratio, i, j);
            let near_energy = if i.is_multiple_of(2) && j.is_multiple_of(2) {
                air[(j / 2) * air_side + i / 2]
            } else {
                let (blended, around) = blend(&air, air_side, 2, i, j);
                let (low, high) = (around.iter())
                    .map(|sums| lden_energy(&sums[0]))
                    .fold((f64::INFINITY, 0.0f64), |(low, high), v| {
                        (low.min(v), high.max(v))
                    });
                if high > AIR_SMOOTH * low {
                    aircraft_sums(square, point, &|d| 1.0 - far(d), near_within_m)?
                } else {
                    blended
                }
            };
            Ok(std::array::from_fn(|kind| {
                std::array::from_fn(|p| far_energy[kind][p] + near_energy[kind][p])
            }))
        })
        .collect()
}
