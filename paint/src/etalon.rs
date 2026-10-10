//! The painter's reference: every outdoor pixel answered as the popup answers a click there, every
//! ground source loud there evaluated exactly, the quiet hum blended from its lattice as the
//! painter takes it, and the flights; a painted map is scored against it cell by cell.

use crate::exact::{Batch, LAYERS, Point, evaluate};
use crate::flights::aircraft_energy;
use crate::lattice::Lattice;
use crate::paint::{Cells, NO_LEVEL, cells_of};
use crate::square::Square;
use popup::aircraft::boxes::AIRCRAFT_REACH_M;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// Pixel rows one batch takes: a city row's pairs are a few hundred megabytes.
const ETALON_PASS_ROWS: usize = 4;

/// Every outdoor pixel of the square (`pixels` a side) as the popup answers it: per layer the
/// ground sources loud there and the quiet hum (`lattice`) times the pixel's reflection, and the flights;
/// no level inside a building.
pub fn etalon(
    square: &Square,
    pixels: usize,
    lattice: &Lattice,
    batch: Option<&dyn Batch>,
) -> Result<Cells, String> {
    let n = pixels;
    let metres = |x: f64, y: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + x / n as f64,
            y: f64::from(square.tile.y) + y / n as f64,
        })
    };
    let mut cells = vec![[NO_LEVEL; LAYERS + 1]; n * n];
    for first_row in (0..n).step_by(ETALON_PASS_ROWS) {
        let rows = first_row..(first_row + ETALON_PASS_ROWS).min(n);
        let outdoor: Vec<(usize, Point)> = (rows.start * n..rows.end * n)
            .into_par_iter()
            .map(|at| {
                let point =
                    Point::at(square, metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5))?;
                Ok((point.own_footprint == 0).then_some((at, point)))
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect();
        let (pixels_at, points): (Vec<usize>, Vec<Point>) = outdoor.into_iter().unzip();
        let pairs: Vec<(u32, u32)> = (0..points.len())
            .into_par_iter()
            .map_init(Vec::new, |near, k| {
                let point = &points[k];
                square.index.reaching(point.position, point.position, near);
                near.iter()
                    .filter(|&&index| square.loud(index, point.position, &point.bound))
                    .map(|&index| (k as u32, index))
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect();
        let energies = evaluate(square, &points, &pairs, batch)?;
        let mut ground = vec![[0.0; LAYERS]; points.len()];
        for (&(k, index), energy) in pairs.iter().zip(energies) {
            ground[k as usize][square.candidates[index as usize].layer as usize] += energy;
        }
        let painted: Vec<[u8; LAYERS + 1]> = (0..points.len())
            .into_par_iter()
            .map(|k| {
                let point = &points[k];
                let quiet = lattice.quiet_at(pixels_at[k] % n, pixels_at[k] / n);
                let mut layers: [f64; LAYERS] =
                    std::array::from_fn(|l| (ground[k][l] + quiet[l]) * point.reflection());
                layers[Layer::Aircraft as usize] +=
                    aircraft_energy(square, point, &|_| 1.0, AIRCRAFT_REACH_M)?;
                Ok(cells_of(&layers))
            })
            .collect::<Result<_, String>>()?;
        for (at, out) in pixels_at.into_iter().zip(painted) {
            cells[at] = out;
        }
    }
    Ok(cells)
}
