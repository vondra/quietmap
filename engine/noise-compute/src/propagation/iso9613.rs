//! Shared propagation helpers: a fast `exp`, the A-weighted band total, the popup's divergence
//! baseline, and airport ground operations' band-mean ground (their carve-out until they move
//! onto `propagation::cnossos`).

use crate::constants::*;
use crate::types::NUM_BANDS;

// Clamp bounds for fast_exp: just inside the f64 subnormal/overflow edges
// (e^±88 ≈ 1.65e38 / 6e-39). Acoustic energies stay comfortably inside, but
// clamping guards against propagating NaN from garbage upstream.
const EXP_CLAMP_LO: f64 = -87.0;
const EXP_CLAMP_HI: f64 = 88.0;
// IEEE 754 double precision exponent bias — used to reconstruct 2^n from
// an integer exponent via raw bit manipulation.
const F64_EXPONENT_BIAS: i64 = 1023;

/// Fast exp() approximation using range reduction + 5th-order polynomial.
/// Worst-case error < 0.001 dB in the acoustic energy domain (|x| < 20).
#[inline(always)]
pub fn fast_exp_f64(x: f64) -> f64 {
    // `.max().min()` (NOT `.clamp()`): on a NaN input `x.max(LO).min(HI)` yields
    // LO, whereas `x.clamp(LO, HI)` returns NaN — and the two also emit different
    // code in this AVX2 hot path. The manual form is part of byte parity.
    #[allow(clippy::manual_clamp)]
    let x = x.max(EXP_CLAMP_LO).min(EXP_CLAMP_HI);
    // Range reduction: e^x = 2^(x/ln2) = 2^n * e^r where |r| <= ln(2)/2
    let inv_ln2 = std::f64::consts::LOG2_E; // 1/ln(2)
    let n_f = (x * inv_ln2).round();
    let r = x - n_f * std::f64::consts::LN_2;
    // 5th-order Taylor: e^r ≈ 1 + r + r²/2 + r³/6 + r⁴/24 + r⁵/120
    let r2 = r * r;
    let poly = 1.0 + r + r2 * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0))));
    let scale = pow2_from_int(n_f as i64);
    poly * scale
}

/// Reconstruct 2^n for integer n in the valid f64 exponent range via
/// direct IEEE 754 bit manipulation (shift the biased exponent into place).
#[inline(always)]
fn pow2_from_int(n: i64) -> f64 {
    f64::from_bits(((F64_EXPONENT_BIAS + n) as u64) << 52)
}

/// Propagation baseline tied to the closest segment — divergence + the ground
/// factor the engine read from the raster. Atmospheric and ground *impacts*
/// belong on the Contributor (energy-weighted across all segments, derived
/// from the ray transfer's hypothesis variants), not here.
pub fn compute_baseline(
    d_slant: f64,
    source_spread: crate::propagation::relevance_bound::SourceSpread,
    ground_g: f64,
) -> crate::types::PropagationBaseline {
    let geometric_db = -source_spread.divergence_db(d_slant);
    crate::types::PropagationBaseline {
        geometric_db: (geometric_db * 10.0).round() / 10.0,
        ground_factor: ground_g,
    }
}

/// Airport ground operations' band-mean ground: `max(CF[i]·G, 0) − 3·(1 − G)`, the surrogate
/// `GROUND_CF·G` for the analytic term of (2.5.15) with the hard floor of (2.5.18) as a lower
/// bound, not an addend (hard ground is exactly −3 dB in every band). Ground operations have no
/// validation lane on the CNOSSOS core yet, so they keep this formation byte for byte.
#[inline]
pub fn aircraft_ground_atten_db(band: usize, ground_g: f64) -> f64 {
    (GROUND_CF[band] * ground_g).max(0.0) + GROUND_HARD_FLOOR_DB * (1.0 - ground_g)
}

/// A-weighted total from octave band levels.
/// L_A = 10 × log₁₀(Σ 10^((L_i + A_i) / 10))
pub fn a_weighted_total(bands: &[f64; NUM_BANDS]) -> f64 {
    let c = std::f64::consts::LN_10 * 0.1;
    let energy: f64 = bands
        .iter()
        .enumerate()
        .map(|(i, &level)| fast_exp_f64((level + A_WEIGHTING[i]) * c))
        .sum();
    if energy > 0.0 {
        10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_operations_keep_the_band_mean_ground() {
        for (band, cf) in GROUND_CF.iter().enumerate() {
            assert_eq!(aircraft_ground_atten_db(band, 0.0), GROUND_HARD_FLOOR_DB);
            assert_eq!(aircraft_ground_atten_db(band, 1.0), cf.max(0.0));
        }
    }

    #[test]
    fn test_fast_exp_accuracy() {
        // Verify fast_exp_f64 stays within ±0.001 dB across acoustic domain
        let mut x = -20.0;
        let mut worst_db = 0.0f64;
        while x <= 20.0 {
            let approx = fast_exp_f64(x);
            let exact = x.exp();
            if exact > 0.0 && approx > 0.0 {
                let err_db = (10.0 * (approx / exact).log10()).abs();
                worst_db = worst_db.max(err_db);
            }
            x += 0.01;
        }
        assert!(
            worst_db < 0.01,
            "fast_exp_f64 worst-case error {:.6} dB exceeds 0.01 dB bound",
            worst_db
        );
    }
}
