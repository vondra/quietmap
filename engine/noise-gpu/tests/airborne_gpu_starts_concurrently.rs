//! Every GPU stream worker must be able to open its own `AirborneGpu` at the same time.
//!
//! `gpu_airborne::stream` builds one per worker (two by default). While the embedded
//! fatbin went to a temp path keyed only by the process id, those constructions raced on
//! one file: `CUDA_ERROR_INVALID_IMAGE` when a worker read what its sibling was still
//! writing, `CUDA_ERROR_FILE_NOT_FOUND` when the sibling had already unlinked it — 4 of 5
//! starts of the shipped painter on an RTX 5070 — and a hang whenever both workers died
//! and the prep thread blocked forever on the depth-1 channel.
//!
//! The barrier is the point of the test: opening the CUDA device dominates `new`, so
//! without it the two fatbin writes can drift far enough apart that the racy code passes by
//! luck. Five rounds, because that is the sample the failure was measured over.
//!
//! Needs a CUDA device, so it runs on a GPU box under `--features gpu`; the whole file
//! compiles away without it.
#![cfg(feature = "gpu")]

use std::sync::Barrier;

use noise_compute::emission::aircraft::ClassWeights;
use noise_gpu::airborne::AirborneGpu;

const STREAM_WORKERS: usize = 2;
const ROUNDS: usize = 5;

#[test]
fn two_stream_workers_open_the_airborne_module_at_once() {
    let class_weights = ClassWeights::uniform();
    for round in 0..ROUNDS {
        let ready = Barrier::new(STREAM_WORKERS);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..STREAM_WORKERS)
                .map(|_| {
                    let ready = &ready;
                    scope.spawn(move || {
                        ready.wait();
                        AirborneGpu::new(&class_weights).vram_total_bytes()
                    })
                })
                .collect();
            for (worker, handle) in workers.into_iter().enumerate() {
                let vram = handle
                    .join()
                    .unwrap_or_else(|_| panic!("round {round} worker {worker}: AirborneGpu::new"));
                assert!(vram > 0, "round {round} worker {worker}: no device VRAM");
            }
        });
    }
}
