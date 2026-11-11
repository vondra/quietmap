//! Retain each sampled hum pair's spectrum without a second trace or a point-band reduction.

use super::{GpuBatch, LaunchInputs, PAIRS_PER_LAUNCH, by_candidate, pair_energy};
use paint::exact::{Pair, PairEnergy, Point, whole_energy};
use physics::bands::{BANDS, PERIODS};
use popup::evaluate::{Bands, Scratch};
use rayon::prelude::*;

pub(super) fn evaluate(
    batch: &GpuBatch,
    points: &[Point],
    pairs: &[Pair],
) -> Result<Vec<(PairEnergy, Bands)>, String> {
    let order = by_candidate(pairs, batch.square.candidates.len());
    let mut evaluated = vec![(PairEnergy::default(), [[0.0; BANDS]; PERIODS]); pairs.len()];
    let mut refused = Vec::new();
    let slots = vec![0; points.len()];
    batch.turns.in_turn(|| -> Result<(), String> {
        let mut bands = batch.gpu.bands(1)?;
        let mut inputs = LaunchInputs::default();
        for launch in order.chunks(PAIRS_PER_LAUNCH) {
            inputs.gather(launch, (points, pairs), &slots);
            let mut flat = Vec::new();
            let (sums, failed) = batch.gpu.evaluate_pairs(
                batch.device_square,
                (&inputs.receivers, &inputs.own),
                (
                    &inputs.candidates,
                    &inputs.receiver_of_pair,
                    &inputs.at,
                    &inputs.weights,
                ),
                &mut bands,
                Some(&mut flat),
            )?;
            for (((&at, sums), bands), failed) in launch
                .iter()
                .zip(&sums)
                .zip(flat.chunks_exact(PERIODS * BANDS))
                .zip(failed)
            {
                if failed != 0 {
                    refused.push(at as usize);
                } else {
                    evaluated[at as usize] = (
                        pair_energy(sums),
                        std::array::from_fn(|p| {
                            std::array::from_fn(|b| f64::from(bands[p * BANDS + b]))
                        }),
                    );
                }
            }
        }
        Ok(())
    })?;
    let fallback: Vec<_> = refused
        .par_iter()
        .map_init(Scratch::default, |scratch, &at| {
            let pair = pairs[at];
            whole_energy(
                batch.square,
                &points[pair.point as usize],
                pair.candidate,
                scratch,
            )
        })
        .collect::<Result<_, String>>()?;
    for (at, value) in refused.into_iter().zip(fallback) {
        evaluated[at] = value;
    }
    Ok(evaluated)
}
