//! Aircraft energy at block corners from distance-weighted lattices, with the popup's boxes and spectra.
//!
//! The flights at a square's block corners: the aircraft boxes as the popup reads them for a
//! click. Cruise boxes whose pieces and both average flight lines stay beyond 8 km use a 5 x 5
//! lattice only where screening does not apply and every corner reads the same boxes within
//! reach. The other far boxes use the hum's points and the near ones every second block corner,
//! each interpolated where it changes slowly; their linear energies and energies times the
//! flights' rate and passing duration are blended. Their spectrum is the loudest flights' at
//! every second block corner, blended between them.

use crate::exact::Point;
use crate::flight_boxes::FlightBoxes;
use crate::loudest::LoudestIndex;
use crate::paint::{Grid, bilinear};
use crate::square::{Square, gap_to_tile};
use physics::bands::{PERIODS, lden_energy};
use physics::doc29::atmosphere::{THIRD_OCTAVES as CLASS_THIRDS, place_rates_db_per_m};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver, tile_flight_sums};
use popup::aircraft::flights::{FlightTotals, heard_spectrum_energy};
use popup::aircraft::horizons::Horizons;
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

/// One aircraft tile a click reads: its place in the square's list, whether its fine boxes (else
/// its far ones), and those boxes.
pub(crate) struct HeardTile<'a> {
    pub number: usize,
    pub fine: bool,
    pub tile: TileId,
    pub boxes: Aircraft<'a>,
}

/// The aircraft tiles a click at `position` reads within `within_m`: fine boxes from tiles within
/// their distance of it, else the far ones.
pub(crate) fn tiles_heard<'a>(
    square: &Square<'a>,
    position: [f64; 2],
    within_m: f64,
) -> Vec<HeardTile<'a>> {
    (square.aircraft.iter().enumerate())
        .filter_map(|(number, tile)| {
            let gap = gap_to_tile(&square.frame, tile.tile, position);
            if gap > within_m {
                return None;
            }
            let (fine, boxes) = match (gap <= FINE_BOXES_WITHIN_M, tile.fine.as_ref()) {
                (true, Some(fine)) => (true, fine),
                _ => (false, tile.far.as_ref()?),
            };
            Some(HeardTile {
                number,
                fine,
                tile: tile.tile,
                boxes: *boxes,
            })
        })
        .collect()
}

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
        let shares = (points.par_iter())
            .map(|point| {
                let weather = point.weather(square);
                let mut flights = FlightTotals::default().in_atmosphere(&weather.alpha_db_per_km);
                loudest.search(square, point, &mut flights)?;
                let energy = heard_spectrum_energy(
                    &flights.loudest(),
                    &place_rates_db_per_m(&weather.alpha_db_per_km),
                );
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

/// All flights at `point` within the aircraft reach,
/// fine boxes from tiles within their distance of the point, as the popup reads them for a click
/// there.
pub(crate) fn aircraft_sums(square: &Square, point: &Point) -> Result<FlightSums, String> {
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
    for heard in tiles_heard(square, point.position, AIRCRAFT_REACH_M) {
        let sums = tile_flight_sums(
            &heard.boxes,
            heard.tile,
            &square.frame,
            receiver,
            &horizons,
            (0..heard.boxes.box_count(), &|_| 1.0, AIRCRAFT_REACH_M),
        );
        for (totals, values) in total.iter_mut().zip([sums.0, sums.1]) {
            for (sum, value) in totals.iter_mut().zip(values) {
                *sum += value;
            }
        }
    }
    Ok(total)
}

/// Flights beyond a minimum slant of 8 km use 5² nodes; the closer boxes keep the near/far split.
pub(crate) fn flights_at_corners(
    square: &Square,
    grid: Grid,
    metres: &(dyn Fn(f64, f64) -> [f64; 2] + Sync),
    corners: &[Point],
) -> Result<Vec<FlightSums>, String> {
    let boxes = FlightBoxes::new(square, corners);
    if boxes.occupied.iter().all(|&occupied| !occupied) {
        return Ok(vec![[[0.0; PERIODS]; 2]; corners.len()]);
    }
    let coarse_m = square.frame.east_m_per_unit / grid.pixels as f64 * grid.hum as f64;
    let far = |distance_m: f64| {
        let fraction = ((distance_m - 1.5 * coarse_m) / (1.5 * coarse_m)).clamp(0.0, 1.0);
        fraction * fraction * (3.0 - 2.0 * fraction)
    };
    let one = |_: f64| 1.0;
    let near = |distance| 1.0 - far(distance);
    let near_within_m = 3.0 * coarse_m;
    let coarse_side = grid.pixels / grid.hum + 1;
    let coarse = (0..coarse_side * coarse_side)
        .into_par_iter()
        .map(|at| {
            let (column, row) = (at % coarse_side, at / coarse_side);
            let point = Point::at(
                square,
                metres((column * grid.hum) as f64, (row * grid.hum) as f64),
            )?;
            boxes.at(
                square,
                &point,
                [
                    boxes.occupied[0],
                    boxes.occupied[1] && column.is_multiple_of(4) && row.is_multiple_of(4),
                ],
                [&far, &one],
                AIRCRAFT_REACH_M,
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let air_side = grid.pixels / (2 * grid.block) + 1;
    let air = (0..air_side * air_side)
        .into_par_iter()
        .map(|at| {
            let point = Point::at(
                square,
                metres(
                    ((at % air_side) * 2 * grid.block) as f64,
                    ((at / air_side) * 2 * grid.block) as f64,
                ),
            )?;
            Ok(boxes.at(
                square,
                &point,
                [boxes.occupied[0], false],
                [&near, &one],
                near_within_m,
            )?[0])
        })
        .collect::<Result<Vec<_>, String>>()?;
    let corner_side = grid.pixels / grid.block + 1;
    let ratio = grid.hum / grid.block;
    let blend = |i: usize, j: usize, step: usize, lookup: &dyn Fn(usize, usize) -> FlightSums| {
        let limit = (corner_side - 1) / step - 1;
        let (west, north) = ((i / step).min(limit) * step, (j / step).min(limit) * step);
        let around = [(0, 0), (step, 0), (0, step), (step, step)]
            .map(|(east, south)| lookup(west + east, north + south));
        (
            blend_flights(
                around,
                (i - west) as f64 / step as f64,
                (j - north) as f64 / step as f64,
            ),
            around,
        )
    };
    corners
        .par_iter()
        .enumerate()
        .map(|(at, point)| {
            let (column, row) = (at % corner_side, at / corner_side);
            let (far_energy, _) = blend(column, row, ratio, &|i, j| {
                coarse[(j / ratio) * coarse_side + i / ratio][0]
            });
            let (distant_energy, _) = blend(column, row, 4 * ratio, &|i, j| {
                coarse[(j / ratio) * coarse_side + i / ratio][1]
            });
            let near_energy = if column.is_multiple_of(2) && row.is_multiple_of(2) {
                air[(row / 2) * air_side + column / 2]
            } else {
                let (blended, around) =
                    blend(column, row, 2, &|i, j| air[(j / 2) * air_side + i / 2]);
                let (low, high) = around
                    .iter()
                    .map(|sums| lden_energy(&sums[0]))
                    .fold((f64::INFINITY, 0.0f64), |(low, high), value| {
                        (low.min(value), high.max(value))
                    });
                if high > 1.258_925_411_794_167_2 * low {
                    boxes.at(
                        square,
                        point,
                        [boxes.occupied[0], false],
                        [&near, &one],
                        near_within_m,
                    )?[0]
                } else {
                    blended
                }
            };
            Ok(std::array::from_fn(|kind| {
                std::array::from_fn(|period| {
                    near_energy[kind][period]
                        + far_energy[kind][period]
                        + distant_energy[kind][period]
                })
            }))
        })
        .collect()
}
