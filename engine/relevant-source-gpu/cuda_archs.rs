//! The CUDA architectures the painter's fatbin carries: the whole fleet, or one pinned card.

/// Every compute capability the rented fleet offers: Turing (2080 Ti), Ampere
/// (A100, 3090), Ada (4090), Hopper (H100), Blackwell (5070). Volta is absent
/// because CUDA 13 dropped `sm_70`.
pub const FLEET_CUDA_ARCHS: [&str; 6] = ["sm_75", "sm_80", "sm_86", "sm_89", "sm_90", "sm_120"];

/// The whole fleet unless `NOISE_GPU_ARCH=sm_NN` pins one card's image; an
/// empty variable is no pin (a builder exporting `NOISE_GPU_ARCH=` gets the fleet).
pub fn cuda_archs(pinned: Option<String>) -> Vec<String> {
    match pinned.filter(|arch| !arch.is_empty()) {
        Some(arch) => vec![arch],
        None => FLEET_CUDA_ARCHS.iter().map(ToString::to_string).collect(),
    }
}

/// The `compute_NN` virtual architecture of an `sm_NN` real one.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fatbin_carries_the_fleet_unless_one_card_is_pinned() {
        assert_eq!(cuda_archs(None), FLEET_CUDA_ARCHS);
        assert_eq!(cuda_archs(Some(String::new())), FLEET_CUDA_ARCHS);
        assert_eq!(cuda_archs(Some("sm_120".into())), ["sm_120"]);
        for arch in FLEET_CUDA_ARCHS {
            assert_eq!(compute_arch(arch), arch.replace("sm_", "compute_"));
        }
    }
}
