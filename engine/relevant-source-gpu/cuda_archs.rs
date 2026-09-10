//! Explicit CUDA target architecture for a reproducible painter build.
use std::env;
pub fn cuda_archs() -> Vec<String> {
    vec![env::var("NOISE_GPU_ARCH").expect("set NOISE_GPU_ARCH=sm_NN for the target card")]
}
pub fn compute_arch(arch: &str) -> String {
    let number = arch
        .strip_prefix("sm_")
        .expect("CUDA target must start with sm_");
    assert!(
        !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()),
        "CUDA target must be sm_NN"
    );
    format!("compute_{number}")
}
