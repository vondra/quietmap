//! The card as the painter's batch (`paint::exact::Batch`): a batch's pairs evaluated on the GPU in
//! order of source, so a warp takes one source at neighbouring receivers; a pair the card cannot
//! take (a buffer too small, a tile it would refuse) is evaluated on the CPU.

use crate::device::{DeviceSquare, Gpu};
use paint::exact::{Batch, Point, whole_energy};
use paint::square::Square;
use physics::bands::PERIODS;
use popup::evaluate::Scratch;
use rayon::prelude::*;

/// Pairs one launch takes (about 70 MB of buffers on the card).
const PAIRS_PER_LAUNCH: usize = 2_000_000;

pub struct GpuBatch<'a> {
    pub gpu: &'a Gpu,
    pub device_square: &'a DeviceSquare,
    pub square: &'a Square<'a>,
}

impl Batch for GpuBatch<'_> {
    fn evaluate(
        &self,
        points: &[Point],
        pairs: &[(u32, u32)],
    ) -> Result<Vec<[f64; PERIODS]>, String> {
        let mut order: Vec<u32> = (0..pairs.len() as u32).collect();
        order.par_sort_unstable_by_key(|&k| (pairs[k as usize].1, pairs[k as usize].0));
        let mut energies = vec![[0.0; PERIODS]; pairs.len()];
        let mut on_cpu = Vec::new();
        for launch in order.chunks(PAIRS_PER_LAUNCH) {
            let point = |k: u32| &points[pairs[k as usize].0 as usize];
            let receivers: Vec<[f64; 2]> = launch.iter().map(|&k| point(k).position).collect();
            let own: Vec<u64> = launch.iter().map(|&k| point(k).own_footprint).collect();
            let candidates: Vec<u32> = launch.iter().map(|&k| pairs[k as usize].1).collect();
            let (periods, failed) =
                self.gpu
                    .evaluate_pairs(self.device_square, &receivers, &own, &candidates)?;
            for (slot, &k) in launch.iter().enumerate() {
                if failed[slot] == 0 {
                    energies[k as usize] = periods[slot];
                } else {
                    on_cpu.push(k);
                }
            }
        }
        let cpu: Vec<[f64; PERIODS]> = on_cpu
            .par_iter()
            .map_init(Scratch::default, |scratch, &k| {
                let (at, index) = pairs[k as usize];
                whole_energy(self.square, &points[at as usize], index, scratch)
            })
            .collect::<Result<_, String>>()?;
        for (&k, energy) in on_cpu.iter().zip(cpu) {
            energies[k as usize] = energy;
        }
        Ok(energies)
    }
}
