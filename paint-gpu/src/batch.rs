//! Exact source energies and weighted receiver spectra on the card, with CPU fallback for refused pairs.
//!
//! The card as the painter's batch (`paint::exact::Batch`): a batch's pairs evaluated on the GPU in
//! order of source, so a warp takes one source at neighbouring receivers; a pair the card cannot
//! take (a buffer too small, a tile it would refuse) is evaluated on the CPU.
//! Sampled quiet pairs carry importance weights for their bands; their energies stay unweighted.

use crate::aircraft::DeviceFlightBoxes;
use crate::device::{DeviceSquare, Gpu, PAIR_SUMS};
use crate::loudness::LOUDNESS_FLOATS;
use crate::turns::CardTurns;
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
    /// The card's turns among the square's evaluations, each taken for its launches alone.
    turns: CardTurns,
}

impl<'a> GpuBatch<'a> {
    pub fn new(gpu: &'a Gpu, device_square: &'a DeviceSquare, square: &'a Square<'a>) -> Self {
        GpuBatch {
            gpu,
            device_square,
            square,
            flight_boxes: OnceLock::new(),
            turns: CardTurns::default(),
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

/// One launch's inputs as the card takes them.
#[derive(Default)]
struct LaunchInputs {
    receivers: Vec<[f64; 2]>,
    own: Vec<u64>,
    candidates: Vec<u32>,
    at: Vec<u32>,
    weights: Vec<f32>,
}

impl LaunchInputs {
    /// The launch's pairs' inputs, each pair read once (`slot_of` each point's slot).
    fn gather(&mut self, launch: &[u32], (points, pairs): (&[Point], &[Pair]), slot_of: &[u32]) {
        self.receivers.clear();
        self.own.clear();
        self.candidates.clear();
        self.at.clear();
        self.weights.clear();
        self.receivers.reserve(launch.len());
        self.own.reserve(launch.len());
        self.candidates.reserve(launch.len());
        self.at.reserve(launch.len());
        self.weights.reserve(launch.len());
        for &k in launch {
            let pair = &pairs[k as usize];
            let point = &points[pair.point as usize];
            self.receivers.push(point.position);
            self.own.push(point.own_footprint);
            self.candidates.push(pair.candidate);
            self.at.push(slot_of[pair.point as usize]);
            self.weights.push(pair.band_weight as f32);
        }
    }
}

/// A launch's pairs' energies from its sums (a refused pair's replaced later by the CPU's).
fn unpack(launch: &[u32], sums: &[[f32; PAIR_SUMS]], energies: &mut [PairEnergy]) {
    for (&k, sums) in launch.iter().zip(sums) {
        let sum = |kind: usize| -> [f64; PERIODS] {
            std::array::from_fn(|p| f64::from(sums[kind * PERIODS + p]))
        };
        energies[k as usize] = PairEnergy {
            mixed: sum(0),
            states: [sum(1), sum(2)],
        };
    }
}

/// What a pass's launches leave.
struct Launched {
    /// The pairs' energies but the last launch's.
    energies: Vec<PairEnergy>,
    /// The pairs the card refused.
    refused: Vec<usize>,
    /// The reached points' bands.
    bands: Vec<f32>,
    /// The last launch's sums, to unpack.
    last_sums: Vec<[f32; PAIR_SUMS]>,
}

impl GpuBatch<'_> {
    /// The pairs launched in `order` on the card (`slot_of` each point's slot among the `reached`,
    /// `gathered` the first launch's inputs). While the card evaluates a launch, a thread beside it
    /// unpacks the one before and gathers the one after, the first making the pass's energies: on
    /// the pass's thread between its launches, the pairs read at random, they kept the card waiting
    /// while the other passes sounded on the cores; made before the turn, a waiting pass's energies
    /// filled memory. No rayon there: an evaluation on a rayon worker would wait for the thread
    /// while its work waited for the pool.
    fn launch(
        &self,
        (points, pairs): (&[Point], &[Pair]),
        order: &[u32],
        (slot_of, reached): (&[u32], usize),
        mut gathered: LaunchInputs,
    ) -> Result<Launched, String> {
        let mut device_bands = self.gpu.bands(reached)?;
        let launches: Vec<&[u32]> = order.chunks(PAIRS_PER_LAUNCH).collect();
        let mut gathering = LaunchInputs::default();
        let (mut energies, mut refused) = (Vec::new(), Vec::new());
        let mut evaluated: Option<(&[u32], Vec<[f32; PAIR_SUMS]>)> = None;
        for (at, &launch) in launches.iter().enumerate() {
            let (sums, failed) = std::thread::scope(|scope| {
                scope.spawn(|| {
                    if energies.is_empty() {
                        energies = vec![PairEnergy::default(); pairs.len()];
                    }
                    if let Some((launch, sums)) = evaluated.take() {
                        unpack(launch, &sums, &mut energies);
                    }
                    if let Some(next) = launches.get(at + 1) {
                        gathering.gather(next, (points, pairs), slot_of);
                    }
                });
                self.gpu.evaluate_pairs(
                    self.device_square,
                    (&gathered.receivers, &gathered.own),
                    (&gathered.candidates, &gathered.at, &gathered.weights),
                    &mut device_bands,
                )
            })?;
            refused.extend(
                (launch.iter().zip(&failed))
                    .filter(|&(_, &failed)| failed != 0)
                    .map(|(&k, _)| k as usize),
            );
            evaluated = Some((launch, sums));
            std::mem::swap(&mut gathered, &mut gathering);
        }
        Ok(Launched {
            energies,
            refused,
            bands: self.gpu.download(&device_bands)?,
            last_sums: evaluated.map(|(_, sums)| sums).unwrap_or_default(),
        })
    }
}

impl Batch for GpuBatch<'_> {
    fn evaluate(&self, points: &[Point], pairs: &[Pair]) -> Result<Evaluated, String> {
        let order = by_candidate(pairs, self.square.candidates.len());
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
        // The card's turn for the launches alone: the pairs sorted and the first launch's gathered
        // before them, the last launch's unpacked and the refused ones evaluated after, on the
        // cores beside the other evaluations' launches.
        let mut first = LaunchInputs::default();
        first.gather(
            &order[..order.len().min(PAIRS_PER_LAUNCH)],
            (points, pairs),
            &slot_of,
        );
        let Launched {
            mut energies,
            refused,
            bands: flat,
            last_sums,
        } = (self.turns)
            .in_turn(|| self.launch((points, pairs), &order, (&slot_of, reached.len()), first))?;
        if let Some(launch) = order.chunks(PAIRS_PER_LAUNCH).last() {
            unpack(launch, &last_sums, &mut energies);
        }
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
        let cpu: Vec<_> = refused
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
        for (&k, (energy, pair_bands)) in refused.iter().zip(cpu) {
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
