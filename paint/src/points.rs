//! Block pixels from exact loud sources, screened residuals, hum, aircraft and shared loudness physics.
//!
//! The points of a pass painted from their blocks: each one's block's exact sources loud there
//! evaluated exactly (here or on a card), what the block leaves out blended from its corners as
//! far as the point hears its far admitted sources, the hum from its points, the flights from the
//! corners; its loudness by the popup's model ([`crate::sone`]), a block hearing only the hum and
//! the flights blending its corners'.

use crate::blocks::{Block, admit, meets};
use crate::exact::{Batch, LAYERS, PIXELS_PER_LOUDNESS_BATCH, Pair, Point, evaluate_all};
use crate::flights::blend_flights;
use crate::paint::{Corners, Grid, Painted, corner_weights, rectangle_at};
use crate::passes::paint_passes;
use crate::sone::{PixelSoundScratch, Rest, pixel_sound};
use crate::square::Square;
use physics::bands::{BANDS, PERIODS, lden_energy};
use popup::percentiles::Resolution;
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use tiles::sources::Layer;

/// A point's loudness per layer and in total: known (sone), or its sound packed for the card.
enum Loudness {
    Known([f64; LAYERS + 1]),
    Packed,
}

/// What paints a square's points once its corners are known.
pub(crate) struct PointPainter<'a> {
    pub square: &'a Square<'a>,
    pub grid: Grid,
    pub batch: Option<&'a dyn Batch>,
    pub corners: Corners,
    /// The exact pairs of the points painted so far.
    pub pairs: AtomicUsize,
}

impl PointPainter<'_> {
    /// `pass` over every run of `runs`, each run with its result: with a card
    /// [`crate::passes::PASSES_AT_ONCE`]
    /// at once, each taking the next run from a shared queue (one's lists and loudness on the
    /// cores while the others' pairs are on the card); without one in order.
    pub fn in_passes<T: Send>(
        &self,
        runs: Vec<Range<usize>>,
        pass: impl Fn(Range<usize>) -> Result<T, String> + Sync,
    ) -> Result<Vec<(Range<usize>, T)>, String> {
        let run_one = |run: &Range<usize>| pass(run.clone()).map(|done| (run.clone(), done));
        if self.batch.is_none() {
            return runs.iter().map(run_one).collect();
        }
        paint_passes(&runs, &pass)
    }

    /// Block `at` (row-major among the square's blocks): the loud sources crossing it or the eight
    /// around, and those its corners admit.
    pub fn block(&self, near: &mut Vec<u32>, at: usize) -> Block {
        let (square, grid) = (self.square, self.grid);
        let blocks = grid.pixels / grid.block;
        let (i, j) = (at % blocks, at / blocks);
        let b = grid.block as f64;
        let (x, y) = (i as f64 * b, j as f64 * b);
        let (low, high) = rectangle_at(square, grid.pixels, x - b, y - b, x + 2.0 * b, y + 2.0 * b);
        square.index.reaching(low, high, near);
        let local: Vec<u32> = (near.iter().copied())
            .filter(|&index| {
                let [a, b] = square.candidates[index as usize].ends_m;
                meets(a, b, low, high)
            })
            .collect();
        let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.corners.corners[(j + dj) * (blocks + 1) + i + di]);
        admit(local, around)
    }

    /// Each point's paint, `None` for one in a building: `places[k]` is its block's slot among
    /// `blocks` (the square's block `block_ids[slot]`) and the pixel (row-major) whose hum, flights
    /// and corner weights it takes; a block with `walls[slot]` never blends its corners' loudness.
    pub fn paint(
        &self,
        block_ids: &[usize],
        blocks: &[Block],
        points: &[Point],
        places: &[(usize, usize)],
        walls: &[bool],
    ) -> Result<Vec<Option<Painted>>, String> {
        let (square, grid, n) = (self.square, self.grid, self.grid.pixels);
        let blocks_per_side = n / grid.block;
        let corner_side = blocks_per_side + 1;
        let corners = &self.corners;
        let lists: Vec<Vec<u32>> = (points.par_iter().zip(places))
            .map(|(point, &(slot, _))| {
                if point.own_footprint != 0 {
                    return Vec::new();
                }
                (blocks[slot].exact.iter().copied())
                    .filter(|&index| square.loud(index, point.position, &point.bound))
                    .collect()
            })
            .collect();
        let pairs: Vec<Pair> = (lists.iter().enumerate())
            .flat_map(|(k, list)| list.iter().map(move |&index| Pair::new(k as u32, index)))
            .collect();
        self.pairs.fetch_add(pairs.len(), Ordering::Relaxed);
        let evaluated = evaluate_all(square, points, &pairs, self.batch)?;
        let mut offset = 0;
        let starts: Vec<usize> = (lists.iter())
            .map(|list| {
                offset += list.len();
                offset - list.len()
            })
            .collect();
        let within = |t: usize| ((t % grid.block) as f64 + 0.5) / grid.block as f64;
        // A block hearing no source of its own, left out or near, and no wall: only the hum and the
        // flights, both blended, so its loudness is blended from its corners'.
        let smooth: Vec<Option<[[f64; LAYERS + 1]; 4]>> = (0..blocks.len())
            .into_par_iter()
            .map(|slot| -> Result<_, String> {
                let block = &blocks[slot];
                if !block.exact.is_empty() || !block.left_groups.is_empty() || walls[slot] {
                    return Ok(None);
                }
                let block_at = block_ids[slot];
                let (i, j) = (block_at % blocks_per_side, block_at / blocks_per_side);
                let corner = |(di, dj): (usize, usize)| {
                    crate::quiet_corners::loudness_at_corner(self, (j + dj) * corner_side + i + di)
                };
                Ok(Some([
                    corner((0, 0))?,
                    corner((1, 0))?,
                    corner((0, 1))?,
                    corner((1, 1))?,
                ]))
            })
            .collect::<Result<_, String>>()?;
        // Each outdoor point's loudness: blended in a block hearing only the hum and the flights;
        // else its sound packed for the card, or its loudness on the cores without one.
        let paint_point = |k: usize,
                           scratch: &mut PixelSoundScratch,
                           input: &mut Vec<f32>|
         -> Result<(usize, [f64; LAYERS], Loudness), String> {
            let ((slot, at), point) = (places[k], &points[k]);
            let (x, y) = (at % n, at / n);
            let (fx, fy) = (within(x), within(y));
            let (i, j) = (x / grid.block, y / grid.block);
            let around =
                [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| (j + dj) * corner_side + i + di);
            let weights = corner_weights(fx, fy);
            let range = starts[k]..starts[k] + lists[k].len();
            let (mut steady, mut bands) = corners.hum.at(x, y);
            // How much of what the block leaves out reaches the point: its admitted far
            // sources' energy here against their corners' blend, at most 1. A pixel behind a
            // wall hears its far sources 10-20 dB under that blend, and the left-out blended
            // from the corners put Dobris's total 7 phon over the etalon there.
            let block = &blocks[slot];
            let admitted_here: f64 = (lists[k].iter().zip(&evaluated.pairs[range.clone()]))
                .filter(|(index, _)| block.admitted.binary_search(index).is_ok())
                .map(|(_, energy)| lden_energy(&energy.mixed))
                .sum();
            let admitted_blended: f64 = (0..4)
                .map(|c| weights[c] * block.admitted_at_corners[c])
                .sum();
            let left_reach = if admitted_blended > 0.0 {
                (admitted_here / admitted_blended).min(1.0)
            } else {
                1.0
            };
            // What the block leaves out beside the hum, each corner's in the shape of all it
            // hears in that layer; of it each group's own.
            for layer in 0..LAYERS {
                for p in 0..PERIODS {
                    for (c, &corner) in around.iter().enumerate() {
                        let energy = weights[c] * block.left_out[c][layer][p] * left_reach;
                        steady[layer][p] += energy;
                        let shape = &corners.corner_bands[corner][layer][p];
                        let sum: f64 = shape.iter().sum();
                        if energy > 0.0 && sum > 0.0 {
                            for b in 0..BANDS {
                                bands[layer][p][b] += energy * shape[b] / sum;
                            }
                        }
                    }
                }
            }
            let rest = Rest {
                steady,
                bands,
                left_groups: (&block.left_groups, weights),
                left_reach,
                flights: blend_flights(around.map(|corner| corners.flights[corner]), fx, fy),
            };
            let reflection = point.reflection();
            let mut layers: [f64; LAYERS] =
                std::array::from_fn(|layer| lden_energy(&rest.steady[layer]));
            for (&index, energy) in lists[k].iter().zip(&evaluated.pairs[range.clone()]) {
                layers[square.candidates[index as usize].layer as usize] +=
                    lden_energy(&energy.mixed);
            }
            let mut layers = layers.map(|energy| reflection * energy);
            layers[Layer::Aircraft as usize] += lden_energy(&rest.flights[0]);
            if let Some(at_corners) = &smooth[slot] {
                let ndens = std::array::from_fn(|l| {
                    (0..4).map(|c| weights[c] * at_corners[c][l]).sum::<f64>()
                });
                return Ok((k, layers, Loudness::Known(ndens)));
            }
            let sound = pixel_sound(
                square,
                &corners.displays,
                scratch,
                (point.position, reflection),
                (&lists[k][..], &evaluated.pairs[range], &evaluated.bands[k]),
                (&rest, &corners.flight_spectra.at(x, y)),
            )?;
            Ok(match self.batch {
                Some(_) => {
                    sound.pack(Resolution::MAP.weather_states, input);
                    (k, layers, Loudness::Packed)
                }
                None => (k, layers, Loudness::Known(sound.ndens(Resolution::MAP))),
            })
        };
        let prepare_chunk = |first: usize, end: usize| -> Result<_, String> {
            let area = grid.block * grid.block;
            let parts = points[first..end]
                .par_chunks(area)
                .enumerate()
                .map(|(part, chunk)| -> Result<_, String> {
                    let start = first + part * area;
                    let mut scratch = PixelSoundScratch::default();
                    let (mut input, mut offsets, mut heard) = (Vec::new(), Vec::new(), Vec::new());
                    for (offset, point) in chunk.iter().enumerate() {
                        if point.own_footprint != 0 {
                            continue;
                        }
                        let k = start + offset;
                        let offset = input.len() as u64;
                        let point = paint_point(k, &mut scratch, &mut input)?;
                        if matches!(point.2, Loudness::Packed) {
                            offsets.push(offset);
                        }
                        heard.push(point);
                    }
                    Ok((heard, input, offsets))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let floats = parts.iter().map(|(_, input, _)| input.len()).sum();
            let count = parts.iter().map(|(_, _, offsets)| offsets.len()).sum();
            let mut input = Vec::with_capacity(floats);
            let mut offsets = Vec::with_capacity(count);
            let mut heard = Vec::new();
            for (part, packed, packed_offsets) in parts {
                let base = input.len() as u64;
                offsets.extend(packed_offsets.into_iter().map(|offset| base + offset));
                input.extend_from_slice(&packed);
                heard.extend(part);
            }
            Ok((heard, input, offsets))
        };
        let mut painted = vec![None; points.len()];
        std::thread::scope(|scope| -> Result<(), String> {
            let (sender, receiver) = std::sync::mpsc::sync_channel(0);
            scope.spawn(move || {
                for first in (0..points.len()).step_by(PIXELS_PER_LOUDNESS_BATCH) {
                    let chunk =
                        prepare_chunk(first, (first + PIXELS_PER_LOUDNESS_BATCH).min(points.len()));
                    let failed = chunk.is_err();
                    if sender.send(chunk).is_err() || failed {
                        return;
                    }
                }
            });
            for chunk in receiver {
                let (heard, input, offsets) = chunk?;
                let mut card = match self.batch {
                    Some(batch) if !offsets.is_empty() => batch.loudness(&input, &offsets)?,
                    _ => Vec::new(),
                }
                .into_iter();
                for (k, layers, loudness) in heard {
                    let ndens = match loudness {
                        Loudness::Known(ndens) => ndens,
                        Loudness::Packed => card
                            .next()
                            .ok_or("a packed pixel has no card loudness")?
                            .map(f64::from),
                    };
                    painted[k] = Some((layers, ndens));
                }
                if card.next().is_some() {
                    return Err("a card loudness has no packed pixel".into());
                }
            }
            Ok(())
        })?;
        Ok(painted)
    }
}
