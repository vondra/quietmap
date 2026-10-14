//! Outdoor-pixel and facade etalons with every retained loud ground source and aircraft box computed exactly.
//!
//! The painter's reference: every outdoor pixel's loudness as the painter's model takes it with
//! every ground source loud there evaluated exactly, the quiet hum blended from its points as the
//! painter takes it, and the flights at the pixel; or, sampled, one outdoor pixel drawn in each K x K
//! cell; a painted map is scored against it cell by cell. A building's pixels are not assessed: the
//! popup's answers inside buildings score the map there.

use crate::exact::{Batch, LAYERS, Pair, Point, add_bands, evaluate_all, whole_energy};
use crate::flight_spectra::FlightSpectra;
use crate::flights::aircraft_sums;
use crate::hum::Hum;
use crate::paint::{Cells, Grid, NO_LEVEL, Painted, cells_of};
use crate::sone::{Displays, PixelSound, PixelSoundScratch, Rest, pixel_ndens, pixel_sound};
use crate::square::Square;
use physics::bands::lden_energy;
use popup::evaluate::Scratch;
use popup::percentiles::Resolution;
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
/// per layer the ground sources loud there and the quiet hum (at the points of `grid`, as the
/// painter takes it) times the pixel's reflection, and the flights, the card's ground pairs beside
/// the cores' flights; its loudness by the painter's model (the flights' spectrum as the painter
/// blends it).
pub fn etalon(
    square: &Square,
    grid: Grid,
    batch: Option<&dyn Batch>,
    sample_step: usize,
) -> Result<Cells, String> {
    if sample_step == 0 {
        return Err("the etalon's sample step is at least 1".into());
    }
    let pixels = grid.pixels;
    let (hum, flight_spectra) = rayon::join(
        || Hum::new(square, pixels, grid.hum, batch),
        || FlightSpectra::new(square, pixels, 2 * grid.block),
    );
    let (hum, flight_spectra) = (hum?, flight_spectra?);
    let n = pixels;
    let metres = |x: f64, y: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + x / n as f64,
            y: f64::from(square.tile.y) + y / n as f64,
        })
    };
    let displays = Displays::of(square)?;
    let mut pixel_layers: Vec<Option<Painted>> = vec![None; n * n];
    let cells = n.div_ceil(sample_step);
    for first_row in (0..cells).step_by(ETALON_PASS_ROWS) {
        let outdoor: Vec<(usize, Point)> = (first_row..(first_row + ETALON_PASS_ROWS).min(cells))
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
            .filter(|(_, point)| point.own_footprint == 0)
            .collect();
        let (pixels_at, points): (Vec<usize>, Vec<Point>) = outdoor.into_iter().unzip();
        let pairs: Vec<Pair> = (0..points.len())
            .into_par_iter()
            .map_init(Vec::new, |near, k| {
                let point = &points[k];
                square.index.reaching(point.position, point.position, near);
                near.iter()
                    .filter(|&&index| square.loud(index, point.position, &point.bound))
                    .map(|&index| Pair::new(k as u32, index))
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect();
        let (evaluated, flights) = rayon::join(
            || evaluate_all(square, &points, &pairs, batch),
            || {
                (points.par_iter())
                    .map(|point| aircraft_sums(square, point))
                    .collect::<Result<Vec<_>, String>>()
            },
        );
        let (evaluated, flights) = (evaluated?, flights?);
        // The pairs come point by point: each point's run of them.
        let mut starts = vec![0usize; points.len() + 1];
        for pair in &pairs {
            let k = pair.point;
            starts[k as usize + 1] += 1;
        }
        for k in 0..points.len() {
            starts[k + 1] += starts[k];
        }
        let candidates: Vec<u32> = pairs.iter().map(|pair| pair.candidate).collect();
        let painted: Vec<Painted> = (0..points.len())
            .into_par_iter()
            .map(|k| -> Result<_, String> {
                let (steady, bands) = hum.at(pixels_at[k] % n, pixels_at[k] / n);
                let rest = Rest {
                    steady,
                    bands,
                    left_groups: (&[], [0.0; 4]),
                    left_reach: 1.0,
                    flights: flights[k],
                };
                let range = starts[k]..starts[k + 1];
                let reflection = points[k].reflection();
                let mut layers: [f64; LAYERS] =
                    std::array::from_fn(|layer| lden_energy(&rest.steady[layer]));
                for (&index, energy) in candidates[range.clone()]
                    .iter()
                    .zip(&evaluated.pairs[range.clone()])
                {
                    layers[square.candidates[index as usize].layer as usize] +=
                        lden_energy(&energy.mixed);
                }
                let mut layers = layers.map(|energy| reflection * energy);
                layers[Layer::Aircraft as usize] += lden_energy(&rest.flights[0]);
                let ndens = pixel_ndens(
                    square,
                    &displays,
                    (points[k].position, reflection),
                    (
                        &candidates[range.clone()],
                        &evaluated.pairs[range],
                        &evaluated.bands[k],
                    ),
                    (
                        &rest,
                        &flight_spectra.at(pixels_at[k] % n, pixels_at[k] / n),
                    ),
                    Resolution::MAP,
                )?;
                Ok((layers, ndens))
            })
            .collect::<Result<_, String>>()?;
        for (at, layers) in pixels_at.into_iter().zip(painted) {
            pixel_layers[at] = Some(layers);
        }
    }
    Ok((pixel_layers.par_iter())
        .map(|pixel| {
            pixel
                .as_ref()
                .map_or([NO_LEVEL; LAYERS + 1], |(_, ndens)| cells_of(ndens))
        })
        .collect())
}

/// What `count` outdoor pixels of the square drawn from `seed` hear (the etalon's way: every
/// ground source loud there evaluated exactly on the cores, the hum of `grid`'s points, the flights
/// there): the inputs a port of the pixels' loudness is held equal to the cores on.
pub fn sounds_at(
    square: &Square,
    grid: Grid,
    count: usize,
    seed: u64,
) -> Result<Vec<PixelSound>, String> {
    let n = grid.pixels;
    let (hum, flight_spectra) = rayon::join(
        || Hum::new(square, n, grid.hum, None),
        || FlightSpectra::new(square, n, 2 * grid.block),
    );
    let (hum, flight_spectra) = (hum?, flight_spectra?);
    let displays = Displays::of(square)?;
    let mut draw = SplitMix(seed);
    let metres = |at: usize| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + ((at % n) as f64 + 0.5) / n as f64,
            y: f64::from(square.tile.y) + ((at / n) as f64 + 0.5) / n as f64,
        })
    };
    let mut points = Vec::new();
    while points.len() < count {
        let at = (draw.next_u64() % (n * n) as u64) as usize;
        let point = Point::at(square, metres(at))?;
        if point.own_footprint == 0 {
            points.push((at, point));
        }
    }
    (points.par_iter())
        .map_init(Scratch::default, |scratch, (at, point)| {
            let mut near = Vec::new();
            square
                .index
                .reaching(point.position, point.position, &mut near);
            let loud: Vec<u32> = (near.into_iter())
                .filter(|&index| square.loud(index, point.position, &point.bound))
                .collect();
            let mut bands = [[[0.0; physics::bands::BANDS]; physics::bands::PERIODS]; LAYERS];
            let mut energies = Vec::with_capacity(loud.len());
            for &index in &loud {
                let (energy, pair) = whole_energy(square, point, index, scratch)?;
                add_bands(square, &mut bands, index, &pair, 1.0);
                energies.push(energy);
            }
            let (steady, hum_bands) = hum.at(at % n, at / n);
            let rest = Rest {
                steady,
                bands: hum_bands,
                left_groups: (&[], [0.0; 4]),
                left_reach: 1.0,
                flights: aircraft_sums(square, point)?,
            };
            pixel_sound(
                square,
                &displays,
                &mut PixelSoundScratch::default(),
                (point.position, point.reflection()),
                (&loud, &energies, &bands),
                (&rest, &flight_spectra.at(at % n, at / n)),
            )
        })
        .collect()
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
