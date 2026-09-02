//! The CUDA architecture a kernel build targets. Every fleet build is a per-box
//! native build (`.cargo/config.toml` pins `target-cpu=native`), so the card in
//! this host is the card that will run the binary.
//!
//! `noise-gpu` includes this file by `#[path]`; `relevant-source-gpu` instead
//! builds its release fatbin for the whole fleet. Build scripts are never compiled under
//! `cfg(test)`, so `noise-gpu`'s lib includes it once more, test-only, to run
//! the parser tests below.

use std::env;
use std::process::Command;

/// Arch used when this host's own card cannot be determined: Ada (4060/4070).
const DEFAULT_ARCH: &str = "sm_89";

/// The architecture to compile device code for: `NOISE_GPU_ARCH` when the
/// caller pins one (the model-role artifact builder does), else this host's
/// first card. `crate_name` only names the crate in the warnings.
pub fn cuda_arch(crate_name: &str) -> String {
    env::var("NOISE_GPU_ARCH")
        .ok()
        .filter(|arch| !arch.is_empty())
        .unwrap_or_else(|| detect_arch(crate_name))
}

/// Announce the fallback and take `DEFAULT_ARCH`.
fn fall_back(crate_name: &str, reason: &str) -> String {
    println!(
        "cargo:warning={crate_name}: {reason}; the AOT device image targets {DEFAULT_ARCH}. \
         Set NOISE_GPU_ARCH if that is not this card: a rejected image costs a \
         PTX JIT in every process, and an exact path refuses to load at all."
    );
    DEFAULT_ARCH.to_owned()
}

/// `sm_NN` for the first card `nvidia-smi --query-gpu=compute_cap` reports.
///
/// The first row, not a unanimous one, because that is the card CUDA opens as
/// device 0 and is what the fleet's own provisioning picks. `caps_disagree`
/// reports the ambiguity separately so the caller can say so.
fn arch_from_compute_caps(listing: &str) -> Result<String, &'static str> {
    let first = listing
        .lines()
        .map(str::trim)
        .find(|row| !row.is_empty())
        .ok_or("nvidia-smi listed no GPU")?;
    let digits = first.replace('.', "");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("nvidia-smi returned an unparsable compute capability");
    }
    Ok(format!("sm_{digits}"))
}

/// Whether the reported cards differ, so that no one image fits all of them.
fn caps_disagree(listing: &str) -> bool {
    let mut rows = listing.lines().map(str::trim).filter(|row| !row.is_empty());
    rows.next()
        .is_some_and(|first| rows.any(|other| other != first))
}

/// Whether this host's nvcc can emit `arch` at all.
///
/// A toolkit older than the card would otherwise turn a build that used to work
/// into `nvcc fatal: Unsupported gpu architecture` — the fixed default merely
/// JIT-ed. Verified both ways: CUDA 11.5 lists sm_75 but not sm_89, CUDA 13.3
/// lists sm_120 but not sm_70. An nvcc too old to answer is left to the compile
/// step, which reports the real error.
fn nvcc_can_emit(arch: &str) -> bool {
    let Ok(listing) = Command::new("nvcc").arg("--list-gpu-code").output() else {
        return true;
    };
    !listing.status.success()
        || String::from_utf8_lossy(&listing.stdout)
            .lines()
            .any(|row| row.trim() == arch)
}

/// This host's own compute capability as `sm_NN`, for the AOT device image.
///
/// A fixed default was wrong on every non-Ada box, and wrong silently: a 5070
/// (sm_120) rejects an sm_89 image, so each W1 process JIT-compiled the
/// embedded PTX at startup (measured 2026-08-28, cold CUDA cache, road z12:
/// 57.2 s against 0.2 s, byte-identical tiles).
///
/// Never fails the build: an unreadable card, an unparsable answer, or an nvcc
/// that cannot emit the detected arch all fall back to `DEFAULT_ARCH` — the old
/// unconditional default, so no build that worked stops working. Cargo cannot
/// watch a card, so only the tracked `NOISE_GPU_ARCH` forces a rebuild; set it
/// if you reuse a target dir across a GPU swap.
fn detect_arch(crate_name: &str) -> String {
    let Ok(probe) = Command::new("nvidia-smi")
        .args(["--query-gpu=compute_cap", "--format=csv,noheader,nounits"])
        .output()
    else {
        return fall_back(crate_name, "nvidia-smi is not on PATH");
    };
    if !probe.status.success() {
        return fall_back(crate_name, "nvidia-smi reported no usable GPU");
    }
    let listing = String::from_utf8_lossy(&probe.stdout);
    let arch = match arch_from_compute_caps(&listing) {
        Ok(arch) => arch,
        Err(reason) => return fall_back(crate_name, reason),
    };
    if !nvcc_can_emit(&arch) {
        return fall_back(
            crate_name,
            "this host's nvcc is too old to emit its own card's arch",
        );
    }
    if caps_disagree(&listing) {
        println!(
            "cargo:warning={crate_name}: this host mixes GPU compute capabilities; \
             the device image targets {arch} (the first card). Set NOISE_GPU_ARCH to choose another."
        );
    }
    arch
}

#[cfg(test)]
mod arch_tests {
    use super::{arch_from_compute_caps, caps_disagree};

    #[test]
    fn one_card_of_each_fleet_generation_maps_to_its_arch() {
        for (listing, arch) in [
            ("12.0\n", "sm_120"),
            ("7.5\n", "sm_75"),
            ("8.9\n", "sm_89"),
            ("7.0\n", "sm_70"),
            (" 12.0 \r\n", "sm_120"),
            ("7.5\n\n", "sm_75"),
        ] {
            assert_eq!(
                arch_from_compute_caps(listing).unwrap(),
                arch,
                "{listing:?}"
            );
        }
    }

    #[test]
    fn identical_cards_are_not_a_disagreement() {
        let listing = "12.0\n12.0\n12.0\n12.0\n";
        assert_eq!(arch_from_compute_caps(listing).unwrap(), "sm_120");
        assert!(!caps_disagree(listing));
    }

    #[test]
    fn a_mixed_host_takes_the_first_card_and_is_flagged() {
        let listing = "12.0\n7.5\n";
        assert_eq!(arch_from_compute_caps(listing).unwrap(), "sm_120");
        assert!(caps_disagree(listing));
    }

    #[test]
    fn an_unreadable_listing_is_an_error_never_a_bogus_arch() {
        for listing in ["", "\n  \n", "N/A\n", "[N/A]\n", ".\n"] {
            assert!(arch_from_compute_caps(listing).is_err(), "{listing:?}");
        }
    }
}
