//! The CUDA architectures a fleet GPU binary is compiled for.
//!
//! Both GPU build scripts include this file by `#[path = "../cuda_archs.rs"]`, so the
//! fleet's architecture list is one fact: `relevant-source-surface` and `gpu-airborne`
//! carry the same SASS set, and a box that runs one runs the other.

use std::env;

/// Every compute capability the rented fleet offers: Turing (2080 Ti), Ampere
/// (A100, 3090), Ada (4090), Hopper (H100), Blackwell (5070). Volta is absent
/// because CUDA 13 dropped `sm_70`; the Tesla V100 gets its own build from the
/// same source with nvcc 12.9 and `NOISE_GPU_ARCH=sm_70`.
pub const FLEET_CUDA_ARCHS: &[&str] = &["sm_75", "sm_80", "sm_86", "sm_89", "sm_90", "sm_120"];

/// The architectures this build targets, lowest first: the one `NOISE_GPU_ARCH`
/// pins (the model-role artifact builder and the Volta build), else the whole fleet.
pub fn cuda_archs() -> Vec<String> {
    env::var("NOISE_GPU_ARCH")
        .ok()
        .filter(|arch| !arch.is_empty())
        .map(|arch| vec![arch])
        .unwrap_or_else(|| FLEET_CUDA_ARCHS.iter().map(ToString::to_string).collect())
}

/// The `compute_NN` virtual architecture of an `sm_NN` real one.
pub fn compute_arch(arch: &str) -> String {
    let number = arch.strip_prefix("sm_").unwrap_or_else(|| {
        panic!("CUDA arch must be sm_NN (NOISE_GPU_ARCH or FLEET_CUDA_ARCHS), got {arch}")
    });
    format!("compute_{number}")
}
