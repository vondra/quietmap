//! Byte-space stopping: a surface pixel is finished as soon as its OUTPUT BYTE
//! is decided, not when its VALUE is.
//!
//! The HM3 cell is a `u8 × 0.5 dB` ([`crate::wire_hm3::quantise_lden`]), so a
//! pixel's whole answer is one byte out of 256. Accumulating a pixel's
//! (source, receiver) pairs therefore does not need the sum — it needs the
//! sum's BYTE. Keep an interval instead of a running total:
//!
//! ```text
//!   P⁻ = Σ exact contributions already computed          (certain lower bound)
//!   P⁺ = P⁻ + Σ cheap upper bound over everything left   (certain upper bound)
//! ```
//!
//! `P⁻ ≤ E ≤ P⁺` holds for every completion of the tail because the cheap bound
//! is provably ≥ the exact contribution of its pair (free field, most
//! favourable ground: attenuation can only ever REMOVE energy — see
//! [`crate::scatter_band::budget_ub_lden`]). So the moment both ends quantise to
//! the same byte, the byte is decided and the whole remaining tail is
//! immaterial: it cannot move the output. The exact per-pair path is the
//! fallback, so the walk always terminates and a pixel that never closes is
//! computed in full — bit-identical to a kernel with no stopping at all.
//!
//! This is EXACT, not an approximation, and that is the point: it replaces the
//! energy-budget skip it supersedes (`skipped ≤ η·kept`, η = 0.40), which was a
//! running test against a `kept` that starts at zero and only grows — so its
//! verdict depended on the order sources were loaded in, and it admitted up to
//! `10·log10(1+η) = 1.46 dB` of energy never counted. The interval test has
//! neither property.
//!
//! ## Why the margin, and what it buys
//!
//! The walk is f64; the tile accumulator is f32 ([`crate::accumulator`]), and
//! the collapse takes `10·log10` of the f32 sums. Truncating the tail is
//! therefore not the only difference from an unstopped kernel — the f32 running
//! sum itself carries up to `n·u` of relative rounding (`u = 2⁻²⁴`) either way.
//! [`accum_margin`] turns that into an energy margin the decision must clear on
//! BOTH ends, which makes the byte-identity a proof rather than a hope: every
//! value the unstopped kernel could land on lies inside the widened interval, so
//! it quantises to the byte we commit to. The margin costs `≈ 0.009 dB` out of a
//! 0.25 dB half-bin at 4 000 pairs — under 2 % of the closures.

use crate::wire_hm3::quantise_lden;

/// f32 unit roundoff, `2⁻²⁴`. The accumulator is f32, the interval walk f64.
const F32_U: f64 = 5.960_464_477_539_063e-8;

/// Head-room over the strict f32 accumulation bound. A sequential f32 sum of
/// `n` non-negative terms carries at most `(n−1)·u/(1−(n−1)·u)` of relative
/// error and each term's own `power as f32` adds `u`, so `(n+1)·u` already
/// covers it; ×2 leaves room for the `10^(dB/10)` round-trip inside
/// [`noise_compute::periods::compute_lden`] and the f64 walk's own summation
/// (both ~1e-16 relative, i.e. nine orders down). Not larger, because the margin
/// is spent out of a 0.25 dB half-bin: it is 0.010 dB at 20 000 pairs per pixel
/// but would be 0.05 dB at 100 000, and past that the stop starts paying real
/// closures for head-room it does not need.
const ACC_SAFETY: f64 = 2.0;

/// Energy scale for the SURFACE layers: `dB = 10·log10(Σ_p W_p·e_p / 24)`, the
/// `/24` of [`noise_compute::periods::compute_lden`], with the `W_p` already in
/// [`crate::scatter_band::LDEN_WEIGHTS`]. Road / rail / industrial / building
/// accumulate a steady period POWER, so there is no `n_days × period_seconds`
/// division (see [`crate::wire_hm3::collapse_lden_surface_u8`]).
pub(crate) const SURFACE_LDEN_SCALE: f64 = 1.0 / 24.0;

/// Relative energy margin the byte decision must clear on both ends for a pixel
/// with `n_pairs` contributing pairs. See the module docs.
#[inline]
pub(crate) fn accum_margin(n_pairs: usize) -> f64 {
    (n_pairs as f64 + 1.0) * F32_U * ACC_SAFETY
}

/// The wire quantiser on a Lden ENERGY — `e × scale` is the number the collapse
/// takes `10·log10` of. It CALLS [`quantise_lden`] rather than mirroring it: the
/// rule's whole claim is about the byte the wire format ends up holding, so a
/// second copy of `round(dB × 2)` here could only ever be a way for the two to
/// disagree.
///
/// A zero, negative, NaN or infinite energy needs no branch of its own — its
/// `10·log10` is `-∞`/NaN/`+∞`, which is exactly what [`quantise_lden`] maps to
/// [`crate::wire_hm3::NO_DATA`]. That sentinel is a decided value like any other
/// and cannot collide with a real byte (the quantiser clamps those at 254), so
/// "both ends read NO_DATA" is itself a decided pixel: the provable skip at a
/// silent receiver, where the superseded `kept`-relative test still evaluated
/// every pair.
#[inline]
fn quant_of(e: f64, scale: f64) -> u8 {
    quantise_lden(10.0 * (e * scale).log10())
}

/// Is the pixel's byte decided? `p_lo` is the exactly-computed energy so far,
/// `p_hi = p_lo + Σ bound over the tail`; `margin` comes from
/// [`accum_margin`]. Widening OUTWARD before quantising is what makes the
/// answer hold for the f32 accumulator as well as for the f64 walk.
///
/// A non-finite `p_hi` is never decided: an infinite or NaN bound would
/// otherwise quantise to NO_DATA and match a silent `p_lo`, turning a broken
/// bound into a silently emptied pixel.
#[inline]
pub(crate) fn decided(p_lo: f64, p_hi: f64, scale: f64, margin: f64) -> bool {
    if !p_hi.is_finite() {
        return false;
    }
    quant_of(p_lo * (1.0 - margin), scale) == quant_of(p_hi * (1.0 + margin), scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire_hm3::NO_DATA;

    /// The ENERGY→dB step is what this pins now that the byte step is one shared
    /// function: `10·log10(e × scale)` must recover the dB the collapse would
    /// have computed closely enough to land on the same byte, across the range
    /// and on both sides of a bin edge, and the out-of-range energies must reach
    /// the wire sentinel rather than a byte.
    #[test]
    fn quant_of_agrees_with_the_wire_quantiser() {
        for &db in &[
            -20.0, -0.001, 0.0, 0.24, 0.25, 0.26, 1.0, 33.3, 45.0, 45.24, 45.26, 100.0, 126.9,
            127.0, 130.0, 300.0,
        ] {
            let e = 10f64.powf(db / 10.0) / SURFACE_LDEN_SCALE;
            assert_eq!(
                quant_of(e, SURFACE_LDEN_SCALE),
                quantise_lden(db),
                "dB={db}: quantisers disagree"
            );
        }
        assert_eq!(quant_of(0.0, SURFACE_LDEN_SCALE), NO_DATA);
        assert_eq!(quant_of(-1.0, SURFACE_LDEN_SCALE), NO_DATA);
        assert_eq!(quant_of(f64::NAN, SURFACE_LDEN_SCALE), NO_DATA);
        assert_eq!(quant_of(f64::INFINITY, SURFACE_LDEN_SCALE), NO_DATA);
    }

    /// The rule's whole contract: whenever it says "decided", the byte it
    /// commits to (`p_lo`) equals the byte of EVERY total the interval admits —
    /// including the extremes. Swept across a bin so both the mid-bin case
    /// (decides early) and the edge case (must not decide) are covered.
    #[test]
    fn a_decided_interval_commits_the_same_byte_as_any_completion() {
        let s = SURFACE_LDEN_SCALE;
        for step in 0..400 {
            let db = 40.0 + step as f64 * 0.0025;
            let lo = 10f64.powf(db / 10.0) / s;
            for &grow in &[1.0, 1.001, 1.01, 1.02, 1.06, 1.2, 2.0] {
                let hi = lo * grow;
                if !decided(lo, hi, s, accum_margin(1000)) {
                    continue;
                }
                let want = quant_of(lo, s);
                for f in 0..=20 {
                    let mid = lo + (hi - lo) * f as f64 / 20.0;
                    assert_eq!(
                        quant_of(mid, s),
                        want,
                        "db={db} grow={grow}: decided but completion {mid} differs"
                    );
                }
            }
        }
    }

    /// A zero-width interval is always decided (that is the "computed every
    /// pair" terminal case), and an interval spanning a whole byte never is.
    #[test]
    fn degenerate_and_wide_intervals() {
        let s = SURFACE_LDEN_SCALE;
        let e = 10f64.powf(4.5) / s;
        assert!(decided(e, e, s, accum_margin(10)));
        assert!(!decided(e, e * 10f64.powf(0.05), s, accum_margin(10)));
        // A silent pixel whose ENTIRE bound is still under 0 dB is decided at
        // pair zero — the provable skip today's `kept`-relative test cannot make.
        assert!(decided(0.0, 0.5 / s, s, accum_margin(10_000)));
        // …and one whose bound reaches over 0 dB is not.
        assert!(!decided(0.0, 4.0 / s, s, accum_margin(10_000)));
        // A NaN/inf bound must never read as decided.
        assert!(!decided(0.0, f64::INFINITY, s, accum_margin(10)));
        assert!(!decided(e, f64::NAN, s, accum_margin(10)));
    }

    /// The margin widens with the pair count — a longer f32 sum really is
    /// noisier — but stays a small slice of the 0.25 dB half-bin it is spent out
    /// of at the pair counts real tiles reach (a dense Praha road pixel is a few
    /// thousand pairs; the whole rail tile 2206/1391 averages ~2 900).
    #[test]
    fn margin_grows_with_pairs_and_stays_small() {
        assert!(accum_margin(10) < accum_margin(10_000));
        for (n, cap_db) in [(1_000usize, 0.002), (20_000, 0.012), (100_000, 0.06)] {
            let db = 10.0 * (1.0 + accum_margin(n)).log10();
            assert!(
                db < cap_db,
                "{n} pairs: margin {db} dB eats too much of a 0.25 dB half-bin"
            );
        }
    }
}
