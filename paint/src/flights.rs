//! The flights at a square's block corners: the aircraft boxes as the popup reads them for a
//! click, the far ones on the coarse lattice and the near ones every second block corner, each
//! interpolated where it changes slowly.

use crate::exact::Point;
use crate::paint::{Grid, bilinear};
use crate::square::{Square, gap_to_tile};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver, tile_lden_energy};
use popup::aircraft::horizons::Horizons;
use rayon::prelude::*;

/// The aircraft's near share is evaluated every second block corner, and at a block corner
/// between them where the four around differ by more than this ratio (1 dB).
const AIR_SMOOTH: f64 = 1.258_925_411_794_167_2;

/// The aircraft's Lden energy at `point`, each box's share `weight` of its distance: every box
/// within `within_m`, fine boxes from tiles within their distance of the point, as the popup
/// reads them for a click there.
pub(crate) fn aircraft_energy(
    square: &Square,
    point: &Point,
    weight: &dyn Fn(f64) -> f64,
    within_m: f64,
) -> Result<f64, String> {
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
    let mut total = 0.0;
    for tile in &square.aircraft {
        let gap = gap_to_tile(&square.frame, tile.tile, point.position);
        if gap > within_m {
            continue;
        }
        let boxes = if gap <= FINE_BOXES_WITHIN_M {
            tile.fine.as_ref().or(tile.far.as_ref())
        } else {
            tile.far.as_ref()
        };
        if let Some(boxes) = boxes {
            total += tile_lden_energy(
                boxes,
                tile.tile,
                &square.frame,
                receiver,
                &horizons,
                (weight, within_m),
            );
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
) -> Result<Vec<f64>, String> {
    if (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none()) {
        return Ok(vec![0.0; corners.len()]);
    }
    let coarse_m = square.frame.east_m_per_unit / grid.pixels as f64 * grid.coarse as f64;
    let far = |distance_m: f64| {
        let t = ((distance_m - 1.5 * coarse_m) / (1.5 * coarse_m)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let near_within_m = 3.0 * coarse_m;
    let lattice = |spacing: usize, far_part: bool| -> Result<(usize, Vec<f64>), String> {
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
                    aircraft_energy(square, &point, &far, AIRCRAFT_REACH_M)
                } else {
                    aircraft_energy(square, &point, &|d| 1.0 - far(d), near_within_m)
                }
            })
            .collect::<Result<_, String>>()?;
        Ok((side, values))
    };
    let (coarse_side, coarse) = lattice(grid.coarse, true)?;
    let (air_side, air) = lattice(2 * grid.block, false)?;
    let corner_side = grid.pixels / grid.block + 1;
    let ratio = grid.coarse / grid.block;
    let blend = |values: &[f64], side: usize, step: usize, i: usize, j: usize| {
        let (ci, cj) = ((i / step).min(side - 2), (j / step).min(side - 2));
        let around =
            [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| values[(cj + dj) * side + ci + di]);
        let (fx, fy) = (
            (i - ci * step) as f64 / step as f64,
            (j - cj * step) as f64 / step as f64,
        );
        (bilinear(around, fx, fy), around)
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
                    .fold((f64::INFINITY, 0.0f64), |(low, high), &v| {
                        (low.min(v), high.max(v))
                    });
                if high > AIR_SMOOTH * low {
                    aircraft_energy(square, point, &|d| 1.0 - far(d), near_within_m)?
                } else {
                    blended
                }
            };
            Ok(far_energy + near_energy)
        })
        .collect()
}
