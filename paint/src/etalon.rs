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
use popup::selection::SplitMix;
use rayon::prelude::*;
use tiles::geo::{Mercator, TileId};
use tiles::sources::Layer;

/// Answered pixel rows one batch takes: a city row's pairs are a few hundred megabytes.
const ETALON_PASS_ROWS: usize = 4;

/// Every outdoor pixel of the square (`pixels` a side) as the popup answers it, or with
/// `sample_step` K above 1 one of every K x K cell (4: a sixteenth, the drift's sample), drawn per
/// cell from the square and the cell, so the sample meets every place of the painter's blocks and
/// cells alike (multiples of K would hit all of their corners and none of their insides):
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
    let cells = n.div_ceil(sample_step);
    for first_row in (0..cells).step_by(ETALON_PASS_ROWS) {
        let (outdoor, inside): (Vec<(usize, Point)>, Vec<_>) = (first_row
            ..(first_row + ETALON_PASS_ROWS).min(cells))
            .flat_map(|row| (0..cells).map(move |column| (row, column)))
            .map(|(row, column)| sampled_pixel(square.tile, n, sample_step, row, column))
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

/// The pixel (row-major index) the sample answers in cell (`row`, `column`) of `step` pixels a side:
/// the whole square at step 1, else one drawn from the square's tile and the cell, inside the cell
/// where the square's edge cuts it.
fn sampled_pixel(tile: TileId, pixels: usize, step: usize, row: usize, column: usize) -> usize {
    let mut draw = SplitMix(
        (u64::from(tile.x) << 44) ^ (u64::from(tile.y) << 24) ^ (row as u64) << 12 ^ column as u64,
    );
    let mut within =
        |start: usize| start + (draw.next_u64() % (pixels - start).min(step) as u64) as usize;
    let (y, x) = (within(row * step), within(column * step));
    y * pixels + x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At step 1 every pixel in order; at step 4 one pixel inside each cell, the cells' places
    /// spread over the cell (not all at its corner), the same at every run; a square's last cells
    /// cut by its edge keep their pixel inside it.
    #[test]
    fn the_sample_takes_one_pixel_of_every_cell_anywhere_in_it() {
        let tile = TileId { x: 2207, y: 1393 };
        let all: Vec<usize> = (0..8)
            .flat_map(|row| (0..8).map(move |column| sampled_pixel(tile, 8, 1, row, column)))
            .collect();
        assert_eq!(all, (0..64).collect::<Vec<_>>());
        let (pixels, step): (usize, usize) = (1026, 4);
        let cells = pixels.div_ceil(step);
        let mut places = [0usize; 16];
        for row in 0..cells {
            for column in 0..cells {
                let at = sampled_pixel(tile, pixels, step, row, column);
                let (y, x) = (at / pixels, at % pixels);
                assert_eq!((y / step, x / step), (row, column));
                assert!(y < pixels && x < pixels);
                assert_eq!(at, sampled_pixel(tile, pixels, step, row, column));
                places[(y % step) * step + x % step] += 1;
            }
        }
        // Each of the 16 places in a cell is drawn about a sixteenth of the time.
        let expected = (cells * cells) as f64 / 16.0;
        assert!(
            places
                .iter()
                .all(|&n| (n as f64 - expected).abs() < 0.1 * expected),
            "{places:?}"
        );
    }
}
