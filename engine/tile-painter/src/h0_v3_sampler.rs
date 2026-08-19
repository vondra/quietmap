//! Frozen sampled receiver set for the pre-registered H0 V3 (S7) arms.
//!
//! The full-resolution seven-arm matrix was measured at 4,204.36 machine-hours
//! against a 192.0 ceiling (`budget/verdict.txt`, 2026-08-19T09:58Z), so the
//! pre-registered fallback renders every arm over a frozen sample of `S`
//! receivers per case instead of all `TILE_PX * TILE_PX`.
//!
//! Two properties make the sample admissible as evidence, and both are
//! structural here rather than procedural:
//!
//! * **Frozen before results.** `S`, the seed and the selection rule are
//!   compile-time constants. There is deliberately no CLI override and no
//!   environment input — a sample size chosen after seeing a judge field would
//!   make the experiment worthless, so the code offers no way to choose one.
//! * **Identical for every arm of a case.** The set is a pure function of the
//!   case index, so all seven arms score the same receivers. The field writer
//!   re-validates the key list on the way to disk, and the analyser compares
//!   arms by key, so a mismatched population fails closed rather than silently
//!   scoring two different populations against each other.
//!
//! `S = 1024` is not a hand-picked number: it is what the pre-registered sizing
//! rule yields from the four sealed GO-1 `h0-3` walls and judge-node censuses —
//! the largest power of two under 8192 keeping the binding case (`praha-rail`,
//! 43.72431 s of seven-arm work per receiver) inside the sealed 20 wall-hour
//! budget, i.e. `S_max = 1646.7`. See the sealed pre-registration block.

use sha2::{Digest, Sha256};

use raster_reader::fused_tile_z13::TILE_PX;

/// Receivers scored per case. Rule-derived and sealed; never an operator input.
pub const H0_V3_SAMPLED_RECEIVERS: usize = 1024;

/// Frozen sampler seed: the eight ASCII bytes `H0V3S7!1`, big-endian. The value
/// is arbitrary by nature — what is binding is that it was fixed before any
/// judge field ran.
pub const H0_V3_SAMPLER_SEED: u64 = 0x4830_5633_5337_2131;

/// Total receivers in one case's tile.
pub const H0_V3_RECEIVER_POPULATION: usize = TILE_PX * TILE_PX;

/// Keyed pseudorandom order for one receiver. Domain-separated by seed and
/// case so two cases draw independent samples from the same population.
fn selection_digest(case_index: u32, receiver_index: u32) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(H0_V3_SAMPLER_SEED.to_be_bytes());
    digest.update(case_index.to_be_bytes());
    digest.update(receiver_index.to_be_bytes());
    digest.finalize().into()
}

/// The case's frozen sampled receiver set, ascending.
///
/// Uniform without replacement: order the whole population by
/// `SHA-256(seed ‖ case ‖ receiver)` and take the first `S`. Uniformity is what
/// makes the pre-registered exceedance bound `1 − 0.05^(1/S)` an exact
/// binomial statement about the population rather than one conditional on a
/// stratification premise.
///
/// The digest is compared before the index, and indices are unique, so the
/// order is total and the result is identical on every host and every arm.
#[must_use]
pub fn h0_v3_sampled_receivers(case_index: u32) -> Vec<u32> {
    let mut ordered: Vec<([u8; 32], u32)> = (0..H0_V3_RECEIVER_POPULATION as u32)
        .map(|receiver_index| (selection_digest(case_index, receiver_index), receiver_index))
        .collect();
    ordered.sort_unstable();
    let mut selected: Vec<u32> = ordered
        .into_iter()
        .take(H0_V3_SAMPLED_RECEIVERS)
        .map(|(_, receiver_index)| receiver_index)
        .collect();
    selected.sort_unstable();
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_the_frozen_size_ascending_and_in_range() {
        for case_index in 0..4 {
            let sample = h0_v3_sampled_receivers(case_index);
            assert_eq!(sample.len(), H0_V3_SAMPLED_RECEIVERS);
            assert!(sample.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(sample
                .iter()
                .all(|&index| (index as usize) < H0_V3_RECEIVER_POPULATION));
        }
    }

    #[test]
    fn sample_is_deterministic_and_case_specific() {
        // Determinism is the property the whole matrix rests on: every arm of a
        // case must draw the identical population, on any host, at any moment.
        assert_eq!(h0_v3_sampled_receivers(2), h0_v3_sampled_receivers(2));
        // Domain separation: two cases must not score the same receivers.
        assert_ne!(h0_v3_sampled_receivers(0), h0_v3_sampled_receivers(1));
    }

    #[test]
    fn sample_covers_the_tile_rather_than_clustering() {
        // A uniform draw of 1024 from 262,144 should touch most tile rows; a
        // sampler accidentally keyed on a truncated index would not.
        let sample = h0_v3_sampled_receivers(0);
        let rows: std::collections::BTreeSet<usize> =
            sample.iter().map(|&i| i as usize / TILE_PX).collect();
        assert!(rows.len() > 400, "rows touched: {}", rows.len());
    }
}
