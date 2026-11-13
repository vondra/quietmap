//! Block pixels from exact loud sources, screened residuals, hum, aircraft and shared loudness physics.
//!
//! The points of a pass painted from their blocks: each one's block's exact sources loud there
//! evaluated exactly, here or on a card ([`crate::traced`]); what the block leaves out blended
//! from its corners, each layer's and direction's as far as the block's far sources of that
//! direction reach the point ([`crate::transfers`]); the hum from its points, the flights from
//! the corners; its loudness by the popup's model ([`crate::sone`]), or given by the caller.
//! Checked interpolation lives in [`crate::quiet`].

use crate::blocks::{Block, admission_counts, admit, meets};
use crate::corner_sources::Corner;
use crate::exact::{Batch, LAYERS, PIXELS_PER_LOUDNESS_BATCH, Point};
use crate::flights::{blend_flight_energy, blend_flights};
use crate::levels::Painted;
use crate::paint::{Corners, Grid, corner_weights, rectangle_at};
use crate::passes::paint_passes;
use crate::sone::{LeftOut, PixelSoundScratch, SoundBesideExactPairs, pixel_sound};
use crate::square::Square;
use crate::traced::{Traced, heard_among};
use crate::transfers::Reaching;
use physics::bands::{BANDS, PERIODS};
use popup::percentiles::Resolution;
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A point's loudness per layer and in total (sone) from its pixel (row-major) and its layers' Lden
/// energies, where it is known without composing the point's sound.
pub(crate) type KnownLoudness<'a> =
    dyn Fn(usize, &[f64; LAYERS]) -> Option<[f64; LAYERS + 1]> + Sync + 'a;

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
    /// The exact pairs of the points painted so far, the points, those that traced deeper, and
    /// those whose sound was composed for its loudness.
    pub pairs: AtomicUsize,
    pub points: AtomicUsize,
    pub deeper: AtomicUsize,
    pub sounds: AtomicUsize,
}

impl PointPainter<'_> {
    /// `pass` over every run of `runs`, each run with its result: with a card
    /// [`crate::passes::PASSES_AT_ONCE`] at once, each taking the next run from a shared queue, the
    /// card launching at most two passes' pairs at once while the others' lists and loudness are on
    /// the cores; without one in order.
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
    /// around, and its corners' own.
    pub fn block(&self, near: &mut Vec<u32>, at: usize) -> Block {
        let (local, around) = self.block_sources(near, at);
        admit(local, around)
    }

    /// The most sources a pixel of block `at` may trace and whether its corners leave any out of
    /// its exact sources ([`admission_counts`]).
    pub fn block_admission(&self, near: &mut Vec<u32>, at: usize) -> (usize, bool) {
        let (local, around) = self.block_sources(near, at);
        admission_counts(local, around)
    }

    /// Block `at`'s own sources (crossing it or the eight around) and its four corners.
    fn block_sources(&self, near: &mut Vec<u32>, at: usize) -> (Vec<u32>, [&Corner; 4]) {
        let (square, grid) = (self.square, self.grid);
        let blocks = grid.pixels / grid.block;
        let (i, j) = (at % blocks, at / blocks);
        let b = grid.block as f64;
        let (x, y) = (i as f64 * b, j as f64 * b);
        let (low, high) = rectangle_at(square, grid.pixels, x - b, y - b, x + 2.0 * b, y + 2.0 * b);
        // The pieces that may lie within the rectangle, not every one whose reach covers it: the same
        // crossing pieces in the same order.
        square.index.within(low, high, 0.0, near);
        let local: Vec<u32> = (near.iter().copied())
            .filter(|&index| {
                let [a, b] = square.candidates[index as usize].ends_m;
                meets(a, b, low, high)
            })
            .collect();
        let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &self.corners.corners[(j + dj) * (blocks + 1) + i + di]);
        (local, around)
    }

    /// Each point's paint, `None` for one in a building: `places[k]` is its block's slot among
    /// `blocks` and the pixel (row-major) whose hum, flights
    /// and corner weights it takes. Every outdoor point reaching this stage computes its sound.
    pub fn paint(
        &self,
        blocks: &[impl std::borrow::Borrow<Block> + Sync],
        points: &[Point],
        places: &[(usize, usize)],
    ) -> Result<Vec<Option<Painted>>, String> {
        self.paint_beside(blocks, points, places, &|_, _| None)
    }

    /// The pixel `at` (row-major): its coordinates, its block's corners (row-major among the
    /// square's) and where it stands between them.
    pub fn place_of(&self, at: usize) -> ((usize, usize), [usize; 4], (f64, f64)) {
        let (n, block) = (self.grid.pixels, self.grid.block);
        let (x, y) = (at % n, at / n);
        let first = (y / block) * (n / block + 1) + x / block;
        let around = [
            first,
            first + 1,
            first + n / block + 1,
            first + n / block + 2,
        ];
        let within = |t: usize| ((t % block) as f64 + 0.5) / block as f64;
        ((x, y), around, (within(x), within(y)))
    }

    /// [`PointPainter::paint`] where `known` may give a point's loudness from its place (`places`'
    /// pixel) and its layers' Lden energies: such a point's sound is not composed.
    pub fn paint_beside(
        &self,
        blocks: &[impl std::borrow::Borrow<Block> + Sync],
        points: &[Point],
        places: &[(usize, usize)],
        known: &KnownLoudness,
    ) -> Result<Vec<Option<Painted>>, String> {
        let (square, grid) = (self.square, self.grid);
        let corners = &self.corners;
        let traced = Traced::trace(self, blocks, points, places)?;
        // Each outdoor point's sound packed for the card, or its loudness on the cores without one.
        let paint_point = |k: usize,
                           scratch: &mut PixelSoundScratch,
                           input: &mut Vec<f32>|
         -> Result<(usize, [f64; LAYERS], Loudness), String> {
            let (at, point) = (places[k].1, &points[k]);
            let ((x, y), around, (fx, fy)) = self.place_of(at);
            let (block, list, energies, heard_bands) = traced.at(k);
            let mut steady = corners.hum.energies_at(x, y);
            // How much of what each corner leaves out reaches the point, by layer and direction:
            // the block's far sources heard here against the corners' blend of them. A pixel
            // behind a wall hears a direction's far sources 10-20 dB under their blend, and the
            // left-out blended from the corners put Dobris's total 7 phon over the etalon there.
            let Reaching {
                transfers,
                energy: left,
                ..
            } = block.reaching(corner_weights(fx, fy), heard_among(list, energies));
            for layer in 0..LAYERS {
                for p in 0..PERIODS {
                    steady[layer][p] += left[layer][p].iter().sum::<f64>();
                }
            }
            let flights_around = around.map(|corner| &corners.flights[corner]);
            let reflection = point.reflection();
            let layers = crate::levels::layer_energy(
                square,
                &steady,
                &blend_flight_energy(flights_around, fx, fy),
                reflection,
                list,
                energies,
            );
            if let Some(ndens) = known(at, &layers) {
                return Ok((k, layers, Loudness::Known(ndens)));
            }
            // The left-out's bands: each corner's in the shape of all it hears in that layer.
            let (_, mut bands) = corners.hum.at(x, y);
            for layer in 0..LAYERS {
                for p in 0..PERIODS {
                    for (c, &corner) in around.iter().enumerate() {
                        let energy = left[layer][p][c];
                        let shape = &corners.corner_bands[corner][layer][p];
                        let sum = corners.corner_band_sums[corner][layer][p];
                        if energy > 0.0 && sum > 0.0 {
                            for b in 0..BANDS {
                                bands[layer][p][b] += energy * shape[b] / sum;
                            }
                        }
                    }
                }
            }
            let rest = SoundBesideExactPairs {
                steady,
                bands,
                left_out: LeftOut {
                    groups: &block.left_groups,
                    bins: &block.left_bins,
                    transfers: &transfers,
                },
                flights: blend_flights(flights_around, fx, fy),
            };
            self.sounds.fetch_add(1, Ordering::Relaxed);
            let sound = pixel_sound(
                square,
                &corners.displays,
                scratch,
                (point, reflection),
                (list, energies, heard_bands),
                (&rest, &corners.flight_spectra.at(x, y)),
            )?;
            Ok(match self.batch {
                Some(_) => {
                    sound.pack(input);
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
