//! Open blocks without crossing ground sources sampled on a loudness grid, checked by sixteen additional pixel receivers.
//!
//! These samples use the ordinary painter's source admission, propagation and sound model.
//! Interpolation replaces its remaining receivers only after checking each layer and total;
//! rejected blocks keep their ordinary pixels. The one-phon check is twice the map cell's step,
//! an empirical safeguard verified against the independent etalon, not a bound on unsampled rays.

use crate::exact::{LAYERS, Point};
use crate::paint::{PASS_BLOCKS, Painted, SILENCE, bilinear, cell, metres_at, runs_within};
use crate::points::PointPainter;
use rayon::prelude::*;

pub(crate) struct QuietSamples {
    axis: [usize; 3],
    values: Vec<Painted>,
}

impl QuietSamples {
    pub fn at(&self, x: usize, y: usize) -> Painted {
        let interval = |at: usize| {
            let low = self
                .axis
                .partition_point(|&sample| sample <= at)
                .saturating_sub(1)
                .min(1);
            (
                low,
                (at - self.axis[low]) as f64 / (self.axis[low + 1] - self.axis[low]) as f64,
            )
        };
        let ((i, fx), (j, fy)) = (interval(x), interval(y));
        let around = [
            j * 3 + i,
            j * 3 + i + 1,
            (j + 1) * 3 + i,
            (j + 1) * 3 + i + 1,
        ];
        (
            std::array::from_fn(|layer| {
                bilinear(around.map(|at| self.values[at].0[layer]), fx, fy)
            }),
            std::array::from_fn(|layer| {
                bilinear(around.map(|at| self.values[at].1[layer]), fx, fy)
            }),
        )
    }
}

/// Each accepted block's samples; rejected blocks retain the ordinary pixel path.
pub(crate) fn sample_open_blocks(
    painter: &PointPainter,
    block_ids: Vec<usize>,
) -> Result<Vec<(usize, QuietSamples)>, String> {
    let (square, grid) = (painter.square, painter.grid);
    let side = grid.pixels / grid.block;
    let b = grid.block;
    let (axis, positions) = sample_positions(b);
    let passes = (0..block_ids.len())
        .step_by(PASS_BLOCKS)
        .map(|start| start..(start + PASS_BLOCKS).min(block_ids.len()))
        .collect();
    // The existing scheduler overlaps sample assembly and card work, retaining its bounded chunks.
    let sampled = painter.in_passes(passes, |range| {
        let chunk = &block_ids[range];
        let mut accepted = Vec::new();
        let blocks: Vec<_> = chunk
            .par_iter()
            .map_init(Vec::new, |near, &at| painter.block(near, at))
            .collect();
        let bounds: Vec<_> = blocks
            .iter()
            .map(|block| positions.len() * block.exact.len())
            .collect();
        for run in runs_within(&bounds, PASS_BLOCKS) {
            let ids = &chunk[run.clone()];
            let places: Vec<_> = ids
                .iter()
                .enumerate()
                .flat_map(|(slot, &at)| {
                    let (x, y) = ((at % side) * b, (at / side) * b);
                    positions
                        .iter()
                        .map(move |&(dx, dy)| (slot, (y + dy) * grid.pixels + x + dx))
                })
                .collect();
            let points = places
                .par_iter()
                .map(|&(_, at)| {
                    Point::at(
                        square,
                        metres_at(
                            square,
                            grid.pixels,
                            (at % grid.pixels) as f64 + 0.5,
                            (at / grid.pixels) as f64 + 0.5,
                        ),
                    )
                })
                .collect::<Result<Vec<_>, String>>()?;
            let painted =
                painter.paint(ids, &blocks[run], &points, &places, &vec![true; ids.len()])?;
            for (slot, &at) in ids.iter().enumerate() {
                let heard: Option<Vec<_>> = painted
                    [slot * positions.len()..(slot + 1) * positions.len()]
                    .iter()
                    .copied()
                    .collect();
                let Some(heard) = heard else { continue };
                let sampled = QuietSamples {
                    axis,
                    values: heard[..9].to_vec(),
                };
                if positions[9..]
                    .iter()
                    .zip(&heard[9..])
                    .all(|(&(x, y), (_, ndens))| {
                        agrees_with_interpolation(ndens, &sampled.at(x, y).1)
                    })
                {
                    accepted.push((at, sampled));
                }
            }
        }
        Ok(accepted)
    })?;
    Ok(sampled
        .into_iter()
        .flat_map(|(_, accepted)| accepted)
        .collect())
}

fn sample_positions(b: usize) -> ([usize; 3], Vec<(usize, usize)>) {
    let axis = [0, b / 2, b - 1];
    let quarters = [0, b / 4, b / 2, 3 * b / 4, b - 1];
    let centres: [usize; 4] = std::array::from_fn(|at| (quarters[at] + quarters[at + 1]) / 2);
    let positions: Vec<_> = axis
        .into_iter()
        .flat_map(|y| axis.into_iter().map(move |x| (x, y)))
        .chain(
            centres
                .into_iter()
                .flat_map(|y| centres.into_iter().map(move |x| (x, y))),
        )
        .collect();
    (axis, positions)
}

/// Two half-phon cell steps in every layer and total; silence is already quantized.
fn agrees_with_interpolation(heard: &[f64; LAYERS + 1], blended: &[f64; LAYERS + 1]) -> bool {
    heard.iter().zip(blended).all(|(&heard, &blended)| {
        if !heard.is_finite() || !blended.is_finite() || heard < 0.0 || blended < 0.0 {
            return false;
        }
        if cell(heard) == SILENCE && cell(blended) == SILENCE {
            return true;
        }
        heard > 0.0 && blended > 0.0 && (10.0 * (heard / blended).log2()).abs() <= 1.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_grid_covers_every_pixel_without_extrapolation() {
        for b in [8, 16] {
            let (axis, positions) = sample_positions(b);
            assert_eq!(positions.len(), 25);
            assert!(
                positions[9..]
                    .iter()
                    .all(|&(x, y)| !axis.contains(&x) && !axis.contains(&y))
            );
            let mut unique = positions.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), positions.len());
            let values = axis
                .into_iter()
                .flat_map(|y| {
                    axis.into_iter().map(move |x| {
                        let value = 1.0 + x as f64 + 2.0 * y as f64;
                        ([value; LAYERS], [value; LAYERS + 1])
                    })
                })
                .collect();
            let samples = QuietSamples { axis, values };
            for y in 0..b {
                for x in 0..b {
                    assert!(
                        (samples.at(x, y).1[LAYERS] - (1.0 + x as f64 + 2.0 * y as f64)).abs()
                            < 1e-12
                    );
                }
            }
        }
    }

    #[test]
    fn a_probe_checks_each_layer_and_rejects_undefined_loudness() {
        let heard = [1.0; LAYERS + 1];
        let mut blended = heard;
        blended[0] *= 2.0f64.powf(1.1 / 10.0);
        assert!(!agrees_with_interpolation(&heard, &blended));
        blended[0] = 2.0f64.powf(0.9 / 10.0);
        assert!(agrees_with_interpolation(&heard, &blended));
        assert!(!agrees_with_interpolation(
            &[f64::NAN; LAYERS + 1],
            &[0.0; LAYERS + 1]
        ));
    }
}
