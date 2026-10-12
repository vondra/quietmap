//! The quiet hum of a square: at points every `spacing` pixels, per layer the energy of the ground
//! sources not loud there (`popup::candidates::loud`) as the popup estimates it at a click
//! (`popup::selection::select`: left out under 0.1 dB of the whole answer, else sampled to
//! 0.05 dB), over the loud ones evaluated exactly. It changes slowly and a pixel takes it blended
//! from the four points around; the painter and its etalon take the same.

use crate::exact::{Batch, LAYERS, Point, evaluate_periods};
use crate::paint::bilinear;
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use popup::candidates::{GROUND_REACH_M, loud};
use popup::selection::{LayerSelection, select};
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// The hum at points every `spacing` of the square's pixels, row-major from the north-west.
pub struct Hum {
    side: usize,
    spacing: usize,
    quiet: Vec<[f64; LAYERS]>,
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
        let points: Vec<Point> = (0..side * side)
            .into_par_iter()
            .map(|at| {
                let (x, y) = ((at % side) * spacing, (at / side) * spacing);
                Point::at(
                    square,
                    square.frame.to_metres(Mercator {
                        x: f64::from(square.tile.x) + x as f64 / pixels as f64,
                        y: f64::from(square.tile.y) + y as f64 / pixels as f64,
                    }),
                )
            })
            .collect::<Result<_, String>>()?;
        let pairs: Vec<(u32, u32)> = (0..points.len())
            .into_par_iter()
            .map_init(Vec::new, |near, k| {
                let point = &points[k];
                square.index.reaching(point.position, point.position, near);
                (near.iter())
                    .filter(|&&index| square.loud(index, point.position, &point.bound))
                    .map(|&index| (k as u32, index))
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect();
        let mut loud_energy = vec![[[0.0; PERIODS]; LAYERS]; points.len()];
        for (&(k, index), energy) in pairs
            .iter()
            .zip(evaluate_periods(square, &points, &pairs, batch)?)
        {
            let layer = square.candidates[index as usize].layer as usize;
            for (total, value) in loud_energy[k as usize][layer].iter_mut().zip(energy) {
                *total += value;
            }
        }
        let quiet = (0..points.len())
            .into_par_iter()
            .map(|k| {
                // The point's pixel in the world, so a point two squares share samples alike.
                let x = u64::from(square.tile.x) * pixels as u64 + ((k % side) * spacing) as u64;
                let y = u64::from(square.tile.y) * pixels as u64 + ((k / side) * spacing) as u64;
                hum_at(square, &points[k], &loud_energy[k], (x << 32) | y)
            })
            .collect::<Result<_, String>>()?;
        Ok(Hum {
            side,
            spacing,
            quiet,
        })
    }

    /// The hum at the pixel `(x, y)` of the square, blended from the four points around its centre.
    pub fn at(&self, x: usize, y: usize) -> [f64; LAYERS] {
        let last = self.side - 2;
        let (i, j) = ((x / self.spacing).min(last), (y / self.spacing).min(last));
        let fx = ((x - i * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let fy = ((y - j * self.spacing) as f64 + 0.5) / self.spacing as f64;
        let corners = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.quiet[(j + dj) * self.side + i + di]);
        std::array::from_fn(|layer| bilinear(corners.map(|corner| corner[layer]), fx, fy))
    }

    /// Whether the hum stays under 0 dB at every point even with the largest reflection (3 dB):
    /// a bilinear blend is largest at a point.
    pub fn silent(&self) -> bool {
        (self.quiet.iter()).all(|quiet| 2.0 * quiet.iter().sum::<f64>() < 1.0)
    }
}

/// The hum at `point`, without its reflection: the quiet sources the popup's selection leaves out
/// or samples there over the loud energy `loud_energy` (per layer and period), `seed` (the point's
/// place in the world) seeding the sample.
fn hum_at(
    square: &Square,
    point: &Point,
    loud_energy: &[[f64; PERIODS]; LAYERS],
    seed: u64,
) -> Result<[f64; LAYERS], String> {
    let mut selections: Vec<LayerSelection> = Layer::ALL
        .iter()
        .zip(loud_energy)
        .map(|(&layer, energy)| LayerSelection {
            energy: *energy,
            ..LayerSelection::new(layer)
        })
        .collect();
    let mut near = Vec::new();
    square
        .index
        .within(point.position, point.position, GROUND_REACH_M, &mut near);
    for &index in &near {
        let mut candidate = square.candidates[index as usize].clone();
        let source = &square.attributes[candidate.attribute];
        if candidate.bound_at(point.position, source, &point.bound) && !loud(&candidate.bound) {
            selections[candidate.layer as usize].pending.push(candidate);
        }
    }
    let receiver = point.receiver(square);
    select(&mut selections, &receiver, &square.attributes, false, seed)?;
    Ok(std::array::from_fn(|layer| {
        (lden_energy(&selections[layer].answer_energy()) - lden_energy(&loud_energy[layer]))
            .max(0.0)
    }))
}
