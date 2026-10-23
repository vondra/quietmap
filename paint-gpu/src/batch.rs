//! Exact source energies and weighted receiver spectra on the card, with CPU fallback for refused pairs.
//!
//! The card as the painter's batch (`paint::exact::Batch`): a batch's pairs evaluated on the GPU in
//! order of source, so a warp takes one source at neighbouring receivers; a pair the card cannot
//! take (a buffer too small, a tile it would refuse) is evaluated on the CPU.
//! Sampled quiet pairs carry importance weights for their bands; their energies stay unweighted.

use crate::aircraft::DeviceFlightBoxes;
use crate::device::{DeviceSquare, Gpu};
use crate::loudness::LOUDNESS_FLOATS;
use paint::exact::{Batch, Evaluated, LAYERS, Pair, PairEnergy, Point, add_bands, whole_energy};
use paint::flight_boxes::{FlightBoxes, FlightRequest};
use paint::flights::FlightSums;
use paint::square::Square;
use physics::bands::{BANDS, PERIODS};
use popup::evaluate::Scratch;
use rayon::prelude::*;
use std::sync::OnceLock;

/// Pairs one launch takes: 172 bytes per pair, 24 per ray and 576 per reached point.
const PAIRS_PER_LAUNCH: usize = 2_000_000;

pub struct GpuBatch<'a> {
    pub gpu: &'a Gpu,
    pub device_square: &'a DeviceSquare,
    pub square: &'a Square<'a>,
    /// The square's aircraft boxes, uploaded at the first flights asked.
    flight_boxes: OnceLock<DeviceFlightBoxes>,
}

impl<'a> GpuBatch<'a> {
    pub fn new(gpu: &'a Gpu, device_square: &'a DeviceSquare, square: &'a Square<'a>) -> Self {
        GpuBatch {
            gpu,
            device_square,
            square,
            flight_boxes: OnceLock::new(),
        }
    }
}

/// The pairs' indices by candidate, each candidate's by point as [`Batch::evaluate`] takes them: a
/// counting sort, where sorting the indices by candidate and point read a pass's millions of pairs at
/// random again and again.
fn by_candidate(pairs: &[Pair], candidates: usize) -> Vec<u32> {
    let mut starts = vec![0u32; candidates + 1];
    for pair in pairs {
        starts[pair.candidate as usize + 1] += 1;
    }
    for at in 0..candidates {
        starts[at + 1] += starts[at];
    }
    let mut order = vec![0u32; pairs.len()];
    for (k, pair) in pairs.iter().enumerate() {
        let slot = &mut starts[pair.candidate as usize];
        order[*slot as usize] = k as u32;
        *slot += 1;
    }
    order
}

impl Batch for GpuBatch<'_> {
    fn evaluate(&self, points: &[Point], pairs: &[Pair]) -> Result<Evaluated, String> {
        let order = by_candidate(pairs, self.square.candidates.len());
        let mut energies = vec![PairEnergy::default(); pairs.len()];
        let mut on_cpu = Vec::new();
        // The card sums bands for the points the pairs reach alone, each at its slot: a pass of a
        // few pairs read back the bands of all its 65,536 points.
        let mut slot_of = vec![u32::MAX; points.len()];
        let mut reached: Vec<usize> = Vec::new();
        for pair in pairs {
            let at = pair.point;
            if slot_of[at as usize] == u32::MAX {
                slot_of[at as usize] = reached.len() as u32;
                reached.push(at as usize);
            }
        }
        let mut device_bands = self.gpu.bands(reached.len())?;
        for launch in order.chunks(PAIRS_PER_LAUNCH) {
            let point = |k: u32| &points[pairs[k as usize].point as usize];
            let receivers: Vec<[f64; 2]> = launch.iter().map(|&k| point(k).position).collect();
            let own: Vec<u64> = launch.iter().map(|&k| point(k).own_footprint).collect();
            let candidates: Vec<u32> = launch
                .iter()
                .map(|&k| pairs[k as usize].candidate)
                .collect();
            let at: Vec<u32> = (launch.iter())
                .map(|&k| slot_of[pairs[k as usize].point as usize])
                .collect();
            let weights: Vec<f32> = launch
                .iter()
                .map(|&k| pairs[k as usize].band_weight as f32)
                .collect();
            let (sums, failed) = self.gpu.evaluate_pairs(
                self.device_square,
                (&receivers, &own),
                (&candidates, &at, &weights),
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
        let mut bands = vec![[[[0.0; BANDS]; PERIODS]; LAYERS]; points.len()];
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
            add_bands(
                self.square,
                &mut bands[at as usize],
                index,
                &pair_bands,
                pairs[k].band_weight,
            );
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
        self.gpu.pixel_loudness(input, offsets)
    }

    fn flight_sums(
        &self,
        square: &Square,
        boxes: &FlightBoxes,
        requests: &[FlightRequest],
    ) -> Result<Vec<[FlightSums; 2]>, String> {
        let device_boxes = match self.flight_boxes.get() {
            Some(device_boxes) => device_boxes,
            None => {
                let uploaded = DeviceFlightBoxes::upload(self.gpu, square, boxes)?;
                self.flight_boxes.get_or_init(|| uploaded)
            }
        };
        self.gpu.flight_sums(square, boxes, device_boxes, requests)
    }
}
