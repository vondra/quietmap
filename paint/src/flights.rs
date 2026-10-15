//! Aircraft energy at block corners from distance-weighted lattices, with the popup's boxes and spectra.
//!
//! The flights at a square's block corners: the aircraft boxes as the popup reads them for a
//! click. Cruise boxes whose pieces and both average flight lines stay beyond 8 km use a 5 x 5
//! lattice only where screening does not apply and every corner reads the same boxes within
//! reach. The other far boxes use the hum's points and the near ones every second block corner,
//! each interpolated where it changes slowly; their linear energies and energies times the
//! flights' rate and passing duration are blended. Their spectral shares keep the finer grid
//! at every second block corner, with checked interpolation ([`crate::flight_spectra`]).

use crate::exact::Batch;
use crate::exact::Point;
use crate::flight_boxes::{FlightBoxes, FlightRequest, FlightWeight, flight_sums};
use crate::paint::{Grid, bilinear};
use crate::square::{Square, gap_to_tile};
use physics::bands::{PERIODS, lden_energy};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver, tile_flight_sums};
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
pub struct HeardTile<'a> {
    pub number: usize,
    pub fine: bool,
    pub tile: TileId,
    pub boxes: Aircraft<'a>,
}

/// The aircraft tiles a click at `position` reads within `within_m`: fine boxes from tiles within
/// their distance of it, else the far ones.
pub fn tiles_heard<'a>(
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

/// The lattices' requests, all block corners: at the hum's points (every `hum / block`th corner)
/// the far boxes and at every fourth of them the distant ones, then at every second corner the
/// near boxes.
pub fn lattice_requests<'a>(
    grid: Grid,
    corners: &'a [Point],
    boxes: &FlightBoxes,
) -> Vec<FlightRequest<'a>> {
    let corner_side = grid.pixels / grid.block + 1;
    let ratio = grid.hum / grid.block;
    let coarse_side = grid.pixels / grid.hum + 1;
    let air_side = grid.pixels / (2 * grid.block) + 1;
    let coarse = (0..coarse_side * coarse_side).map(|at| {
        let (column, row) = (at % coarse_side, at / coarse_side);
        FlightRequest {
            point: &corners[(row * ratio) * corner_side + column * ratio],
            weights: [
                boxes.occupied[0].then_some(FlightWeight::Far),
                (boxes.occupied[1] && column.is_multiple_of(4) && row.is_multiple_of(4))
                    .then_some(FlightWeight::One),
            ],
            within_m: AIRCRAFT_REACH_M,
        }
    });
    let air = (0..air_side * air_side).map(|at| {
        near_request(
            &corners[(at / air_side) * 2 * corner_side + (at % air_side) * 2],
            boxes,
        )
    });
    coarse.chain(air).collect()
}

/// A point's near boxes, heard within three of the hum's spacings.
fn near_request<'a>(point: &'a Point, boxes: &FlightBoxes) -> FlightRequest<'a> {
    FlightRequest {
        point,
        weights: [boxes.occupied[0].then_some(FlightWeight::Near), None],
        within_m: 3.0 * boxes.coarse_m,
    }
}

/// Flights beyond a minimum slant of 8 km use 5² nodes; the closer boxes keep the near/far split.
/// The lattices' sums are asked of the card when there is one (`batch`), else of the cores.
pub(crate) fn flights_at_corners(
    square: &Square,
    grid: Grid,
    corners: &[Point],
    batch: Option<&dyn Batch>,
) -> Result<Vec<FlightSums>, String> {
    let boxes = FlightBoxes::new(square, grid, corners);
    if boxes.occupied.iter().all(|&occupied| !occupied) {
        return Ok(vec![[[0.0; PERIODS]; 2]; corners.len()]);
    }
    let corner_side = grid.pixels / grid.block + 1;
    let ratio = grid.hum / grid.block;
    let coarse_side = grid.pixels / grid.hum + 1;
    let air_side = grid.pixels / (2 * grid.block) + 1;
    let sums = flight_sums(
        square,
        &boxes,
        &lattice_requests(grid, corners, &boxes),
        batch,
    )?;
    let (coarse, air) = sums.split_at(coarse_side * coarse_side);
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
    // Every other corner's near flights blended from the four around, or summed exactly where they
    // differ by more than 1 dB.
    let near_energy: Vec<Option<FlightSums>> = (0..corners.len())
        .into_par_iter()
        .map(|at| {
            let (column, row) = (at % corner_side, at / corner_side);
            if column.is_multiple_of(2) && row.is_multiple_of(2) {
                return Some(air[(row / 2) * air_side + column / 2][0]);
            }
            let (blended, around) =
                blend(column, row, 2, &|i, j| air[(j / 2) * air_side + i / 2][0]);
            let (low, high) = around
                .iter()
                .map(|sums| lden_energy(&sums[0]))
                .fold((f64::INFINITY, 0.0f64), |(low, high), value| {
                    (low.min(value), high.max(value))
                });
            (high <= 1.258_925_411_794_167_2 * low).then_some(blended)
        })
        .collect();
    let requests: Vec<FlightRequest> = (near_energy.iter().zip(corners))
        .filter(|(sums, _)| sums.is_none())
        .map(|(_, corner)| near_request(corner, &boxes))
        .collect();
    let mut exact_sums = flight_sums(square, &boxes, &requests, batch)?.into_iter();
    let near_energy: Vec<FlightSums> = near_energy
        .into_iter()
        .map(|sums| sums.unwrap_or_else(|| exact_sums.next().unwrap()[0]))
        .collect();
    Ok((0..corners.len())
        .into_par_iter()
        .map(|at| {
            let (column, row) = (at % corner_side, at / corner_side);
            let (far_energy, _) = blend(column, row, ratio, &|i, j| {
                coarse[(j / ratio) * coarse_side + i / ratio][0]
            });
            let (distant_energy, _) = blend(column, row, 4 * ratio, &|i, j| {
                coarse[(j / ratio) * coarse_side + i / ratio][1]
            });
            std::array::from_fn(|kind| {
                std::array::from_fn(|period| {
                    near_energy[at][kind][period]
                        + far_energy[kind][period]
                        + distant_energy[kind][period]
                })
            })
        })
        .collect())
}
