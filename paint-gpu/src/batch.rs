//! The card as the painter's batch (`paint::exact::Batch`): a batch's pairs evaluated on the GPU in
//! order of source, so a warp takes one source at neighbouring receivers; a pair the card cannot
//! take (a buffer too small, a tile it would refuse) is evaluated on the CPU.

use crate::device::{DeviceSquare, Gpu};
use paint::exact::{Batch, Evaluated, LAYERS, PairEnergy, Point, add_bands, whole_energy};
use paint::square::Square;
use physics::bands::{BANDS, PERIODS};
use popup::evaluate::Scratch;
use rayon::prelude::*;

/// Pairs one launch takes (about 110 MB of buffers on the card).
const PAIRS_PER_LAUNCH: usize = 2_000_000;

pub struct GpuBatch<'a> {
    pub gpu: &'a Gpu,
    pub device_square: &'a DeviceSquare,
    pub square: &'a Square<'a>,
}

impl Batch for GpuBatch<'_> {
    fn evaluate(&self, points: &[Point], pairs: &[(u32, u32)]) -> Result<Evaluated, String> {
        let mut order: Vec<u32> = (0..pairs.len() as u32).collect();
        order.par_sort_unstable_by_key(|&k| (pairs[k as usize].1, pairs[k as usize].0));
        let mut energies = vec![PairEnergy::default(); pairs.len()];
        let mut on_cpu = Vec::new();
        let mut device_bands = self.gpu.bands(points.len())?;
        for launch in order.chunks(PAIRS_PER_LAUNCH) {
            let point = |k: u32| &points[pairs[k as usize].0 as usize];
            let receivers: Vec<[f64; 2]> = launch.iter().map(|&k| point(k).position).collect();
            let own: Vec<u64> = launch.iter().map(|&k| point(k).own_footprint).collect();
            let candidates: Vec<u32> = launch.iter().map(|&k| pairs[k as usize].1).collect();
            let at: Vec<u32> = launch.iter().map(|&k| pairs[k as usize].0).collect();
            let (sums, failed) = self.gpu.evaluate_pairs(
                self.device_square,
                (&receivers, &own),
                (&candidates, &at),
                &mut device_bands,
            )?;
            for (slot, &k) in launch.iter().enumerate() {
                if failed[slot] != 0 {
                    on_cpu.push(k as usize);
                    continue;
                }
                let sum = |kind: usize| -> [f64; PERIODS] {
                    std::array::from_fn(|p| f64::from(sums[slot][kind * PERIODS + p]))
                };
                energies[k as usize] = PairEnergy {
                    mixed: sum(0),
                    states: [sum(1), sum(2)],
                };
            }
        }
        let flat = self.gpu.download(&device_bands)?;
        let mut bands: Vec<_> = flat
            .chunks_exact(LAYERS * PERIODS * BANDS)
            .take(points.len())
            .map(|point| {
                std::array::from_fn(|l| {
                    std::array::from_fn(|p| {
                        std::array::from_fn(|b| f64::from(point[(l * PERIODS + p) * BANDS + b]))
                    })
                })
            })
            .collect();
        let cpu: Vec<_> = on_cpu
            .par_iter()
            .map_init(Scratch::default, |scratch, &k| {
                let (at, index) = pairs[k];
                whole_energy(self.square, &points[at as usize], index, scratch)
            })
            .collect::<Result<_, String>>()?;
        for (&k, (energy, pair_bands)) in on_cpu.iter().zip(cpu) {
            let (at, index) = pairs[k];
            energies[k] = energy;
            add_bands(self.square, &mut bands[at as usize], index, &pair_bands);
        }
        Ok(Evaluated {
            pairs: energies,
            bands,
        })
    }
}
