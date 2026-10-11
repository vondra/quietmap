//! The painter's reference: every outdoor pixel answered as the popup answers a click there, every
//! ground source loud there evaluated exactly, the quiet hum blended from its lattice as the
//! painter takes it, and the flights, a building's pixels at its loudest outdoor neighbour as the
//! painter takes them; or, sampled, every K-th outdoor pixel each way; a painted map is scored
//! against it cell by cell.

use crate::buildings::{fill_buildings, own_reflection_db};
use crate::exact::{Batch, LAYERS, Point, evaluate};
use crate::flights::aircraft_energy;
use crate::lattice::Lattice;
use crate::paint::{Cells, NO_LEVEL, cells_of};
use crate::square::Square;
use popup::aircraft::boxes::AIRCRAFT_REACH_M;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// Answered pixel rows one batch takes: a city row's pairs are a few hundred megabytes.
const ETALON_PASS_ROWS: usize = 4;

/// Every outdoor pixel of the square (`pixels` a side) as the popup answers it, or with
/// `sample_step` K above 1 those at multiples of K both ways (4: a sixteenth, the drift's sample):
/// per layer the ground sources loud there and the quiet hum (`lattice`) times the pixel's
/// reflection, and the flights, the card's ground pairs beside the cores' flights; a building's
/// pixels at its loudest outdoor neighbour, which only the full etalon has (a sample leaves them
/// not assessed).
pub fn etalon(
    square: &Square,
    pixels: usize,
    lattice: &Lattice,
    batch: Option<&dyn Batch>,
    sample_step: usize,
) -> Result<Cells, String> {
    if sample_step == 0 {
        return Err("the etalon's sample step is at least 1".into());
    }
    let n = pixels;
    let metres = |x: f64, y: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + x / n as f64,
            y: f64::from(square.tile.y) + y / n as f64,
        })
    };
    let mut pixel_layers: Vec<Option<[f64; LAYERS]>> = vec![None; n * n];
    let mut indoor = Vec::new();
    let answered_rows = n.div_ceil(sample_step);
    for first_row in (0..answered_rows).step_by(ETALON_PASS_ROWS) {
        let (outdoor, inside): (Vec<(usize, Point)>, Vec<_>) = (first_row
            ..(first_row + ETALON_PASS_ROWS).min(answered_rows))
            .flat_map(|row| (0..answered_rows).map(move |column| (row, column)))
            .map(|(row, column)| row * sample_step * n + column * sample_step)
            .collect::<Vec<usize>>()
            .into_par_iter()
            .map(|at| {
                let point =
                    Point::at(square, metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5))?;
                Ok((at, point))
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .partition(|(_, point)| point.own_footprint == 0);
        indoor.extend(
            inside
                .into_iter()
                .map(|(at, point)| (at, point.own_footprint)),
        );
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
        let (energies, flights) = rayon::join(
            || evaluate(square, &points, &pairs, batch),
            || {
                (points.par_iter())
                    .map(|point| aircraft_energy(square, point, &|_| 1.0, AIRCRAFT_REACH_M))
                    .collect::<Result<Vec<f64>, String>>()
            },
        );
        let (energies, flights) = (energies?, flights?);
        let mut ground = vec![[0.0; LAYERS]; points.len()];
        for (&(k, index), energy) in pairs.iter().zip(energies) {
            ground[k as usize][square.candidates[index as usize].layer as usize] += energy;
        }
        let painted: Vec<[f64; LAYERS]> = (0..points.len())
            .into_par_iter()
            .map(|k| {
                let quiet = lattice.quiet_at(pixels_at[k] % n, pixels_at[k] / n);
                let mut layers: [f64; LAYERS] =
                    std::array::from_fn(|l| (ground[k][l] + quiet[l]) * points[k].reflection());
                layers[Layer::Aircraft as usize] += flights[k];
                layers
            })
            .collect();
        for (at, layers) in pixels_at.into_iter().zip(painted) {
            pixel_layers[at] = Some(layers);
        }
    }
    if sample_step == 1 {
        fill_buildings(&mut pixel_layers, n, &indoor, &|at, footprint| {
            own_reflection_db(
                square,
                metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5),
                footprint,
            )
        })?;
    }
    Ok((pixel_layers.par_iter())
        .map(|layers| layers.as_ref().map_or([NO_LEVEL; LAYERS + 1], cells_of))
        .collect())
}
