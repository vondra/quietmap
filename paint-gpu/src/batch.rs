//! Exact source energies and weighted receiver spectra on the card, with CPU fallback for refused pairs.
//!
//! The card as the painter's batch (`paint::exact::Batch`): a batch's pairs evaluated on the GPU in
//! order of source, so a warp takes one source at neighbouring receivers; a pair the card cannot
//! take (a buffer too small, a tile it would refuse) is evaluated on the CPU.
//! Sampled quiet pairs carry importance weights for their bands; their energies stay unweighted.

use crate::device::{DeviceSquare, Gpu, PAIR_SUMS};
use crate::loudness::LOUDNESS_FLOATS;
use paint::exact::{
    Batch, Evaluated, LAYERS, PIXELS_PER_LOUDNESS_BATCH, Pair, PairEnergy, Point, add_bands,
    whole_energy,
};
use paint::square::Square;
use physics::bands::{BANDS, PERIODS};
use popup::evaluate::Scratch;
use rayon::prelude::*;

/// Pairs one launch takes.
const PAIRS_PER_LAUNCH: usize = 4_000_000;

pub struct GpuBatch<'a> {
    pub gpu: &'a Gpu,
    pub device_square: &'a DeviceSquare,
    pub square: &'a Square<'a>,
}

impl Batch for GpuBatch<'_> {
    fn evaluate(&self, points: &[Point], pairs: &[Pair]) -> Result<Evaluated, String> {
        let mut counts = vec![0u32; self.square.candidates.len() + 1];
        let mut ordered_by_point = true;
        let mut last_point = 0u32;
        let mut any_non_one_weight = false;
        let mut any_non_zero_weight = false;
        // The card sums bands for the points the pairs reach alone, each at its slot: a pass of a
        // few pairs read back the bands of all its 65,536 points.
        let mut slot_of = vec![u32::MAX; points.len()];
        let mut reached: Vec<usize> = Vec::new();
        for pair in pairs {
            counts[pair.candidate as usize + 1] += 1;
            ordered_by_point &= pair.point >= last_point;
            last_point = pair.point;
            any_non_one_weight |= pair.band_weight != 1.0;
            any_non_zero_weight |= pair.band_weight != 0.0;
            let at = pair.point;
            if slot_of[at as usize] == u32::MAX {
                slot_of[at as usize] = reached.len() as u32;
                reached.push(at as usize);
            }
        }
        let mut order = vec![0u32; pairs.len()];
        let mut sorted_candidates = vec![0u32; pairs.len()];
        let mut sorted_at = vec![0u32; pairs.len()];
        if ordered_by_point {
            for i in 1..counts.len() {
                counts[i] += counts[i - 1];
            }
            for (k, pair) in pairs.iter().enumerate() {
                let slot = counts[pair.candidate as usize] as usize;
                counts[pair.candidate as usize] += 1;
                order[slot] = k as u32;
                sorted_candidates[slot] = pair.candidate;
                sorted_at[slot] = slot_of[pair.point as usize];
            }
        } else {
            for (k, slot) in order.iter_mut().enumerate() {
                *slot = k as u32;
            }
            order.par_sort_unstable_by_key(|&k| {
                (pairs[k as usize].candidate, pairs[k as usize].point)
            });
            for (slot, &k) in order.iter().enumerate() {
                let pair = &pairs[k as usize];
                sorted_candidates[slot] = pair.candidate;
                sorted_at[slot] = slot_of[pair.point as usize];
            }
        }
        let sorted_weights: Option<Vec<f32>> =
            (any_non_one_weight && any_non_zero_weight).then(|| {
                order
                    .iter()
                    .map(|&k| pairs[k as usize].band_weight as f32)
                    .collect()
            });
        let mut point_xy = Vec::with_capacity(2 * reached.len());
        let mut point_own = Vec::with_capacity(reached.len());
        for &at in &reached {
            let p = &points[at];
            point_xy.push(p.position[0] as f32);
            point_xy.push(p.position[1] as f32);
            point_own.push(p.own_footprint);
        }
        let device_points = self.gpu.upload_points(&point_xy, &point_own)?;
        let mut device_bands = self.gpu.bands(if any_non_zero_weight {
            reached.len()
        } else {
            0
        })?;
        let mut energies = vec![PairEnergy::default(); pairs.len()];
        let mut on_cpu = Vec::new();
        for start in (0..order.len()).step_by(PAIRS_PER_LAUNCH) {
            let end = (start + PAIRS_PER_LAUNCH).min(order.len());
            let launch = &order[start..end];
            let weights = sorted_weights.as_ref().map(|w| &w[start..end]);
            let (sums, failed) = self.gpu.evaluate_indexed_pairs(
                self.device_square,
                &device_points,
                (
                    &sorted_candidates[start..end],
                    &sorted_at[start..end],
                    weights,
                ),
                any_non_zero_weight,
                &mut device_bands,
            )?;
            for (slot, &k) in launch.iter().enumerate() {
                if failed[slot] != 0 {
                    on_cpu.push(k as usize);
                    continue;
                }
                let base = slot * PAIR_SUMS;
                let sum = |kind: usize| -> [f64; PERIODS] {
                    std::array::from_fn(|p| f64::from(sums[base + kind * PERIODS + p]))
                };
                energies[k as usize] = PairEnergy {
                    mixed: sum(0),
                    states: [sum(1), sum(2)],
                };
            }
        }
        let mut bands = if any_non_zero_weight {
            let mut bands = vec![[[[0.0; BANDS]; PERIODS]; LAYERS]; points.len()];
            let flat = self.gpu.download(&device_bands)?;
            for (&at, point) in reached
                .iter()
                .zip(flat.chunks_exact(LAYERS * PERIODS * BANDS))
            {
                bands[at] = std::array::from_fn(|l| {
                    std::array::from_fn(|p| {
                        std::array::from_fn(|b| f64::from(point[(l * PERIODS + p) * BANDS + b]))
                    })
                });
            }
            bands
        } else {
            Vec::new()
        };
        let cpu: Vec<_> = on_cpu
            .par_iter()
            .map_init(Scratch::default, |scratch, &k| {
                let Pair {
                    point: at,
                    candidate: index,
                    ..
                } = pairs[k];
                whole_energy(self.square, &points[at as usize], index, scratch)
            })
            .collect::<Result<_, String>>()?;
        for (&k, (energy, pair_bands)) in on_cpu.iter().zip(cpu) {
            let Pair {
                point: at,
                candidate: index,
                ..
            } = pairs[k];
            energies[k] = energy;
            if any_non_zero_weight {
                add_bands(
                    self.square,
                    &mut bands[at as usize],
                    index,
                    &pair_bands,
                    pairs[k].band_weight,
                );
            }
        }
        Ok(Evaluated {
            pairs: energies,
            bands,
        })
    }

    fn loudness(
        &self,
        input: &[f32],
        offsets: &[u64],
    ) -> Result<Vec<[f32; LOUDNESS_FLOATS]>, String> {
        let mut ndens = Vec::with_capacity(offsets.len());
        for (k, launch) in offsets.chunks(PIXELS_PER_LOUDNESS_BATCH).enumerate() {
            let start = launch[0] as usize;
            let end = offsets
                .get((k + 1) * PIXELS_PER_LOUDNESS_BATCH)
                .map_or(input.len(), |&end| end as usize);
            let local: Vec<u64> = launch.iter().map(|&offset| offset - start as u64).collect();
            ndens.extend(self.gpu.pixel_loudness(&input[start..end], &local)?);
        }
        Ok(ndens)
    }
}
