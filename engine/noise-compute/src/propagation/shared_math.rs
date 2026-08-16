//! Bit-identical f64 `atan`/`tan` shared by the CPU (Rust) and CUDA lanes.
//!
//! WHY THIS EXISTS. Model v2 places line nodes by mapping each (piece, receiver)
//! pair through the projective angle `u(s) = atan((s − f)/h)` and cutting `u`
//! into equal cells. The node COUNT is `ceil((u_hi − u_lo)/THETA_MAX)`, so one
//! ulp of disagreement at a cell boundary turns a count of 4 into 5 — and the
//! e2-full GPU-vs-CPU validator treats a count mismatch as a HARD failure, not
//! a tolerance (plan §6.3). Neither glibc's nor CUDA's f64 `atan`/`tan` is
//! correctly rounded (CUDA documents ≤ 2 ulp), so the lanes cannot both call
//! "the platform atan" and expect the same bits. This module is the ONE
//! implementation; `engine/noise-gpu/kernels/qm_shared_math.cuh` is its device
//! mirror, expression-for-expression, under the same QM-ATAN-n / QM-TAN-n
//! equation labels (plan §6.2 label discipline). A diff touching a labelled
//! expression here must show the matching hunk there.
//!
//! WHY A PORT AND NOT FRESH MINIMAX COEFFICIENTS. The requirement is
//! bit-identity between two lanes, not ulp perfection: the approximation error
//! nudges a cell boundary by nanoradians, which is invisible in the placement
//! metric. So the cheapest correct answer is the battle-tested one — a verbatim
//! port of the FreeBSD/fdlibm `s_atan.c`, `k_tan.c` and `e_rem_pio2.c` algorithms
//! in their musl form (read from the vendored `libm 0.2.16` Rust port,
//! `~/.cargo/registry/…/libm-0.2.16/src/math/{atan,k_tan,tan,rem_pio2}.rs`;
//! Sun Microsystems 1993/2004 permissive notice, MIT/Apache-2.0 as redistributed
//! by rust-lang/libm). Published constants, published error bound (< 1 ulp),
//! twenty years of exposure — versus a home-rolled Remez fit whose provenance
//! would be "we ran a script once". Occam: simplicity beats novelty.
//!
//! ARITHMETIC DISCIPLINE. Every expression below uses only `+ − × ÷` and bit
//! moves, all IEEE-754 correctly rounded on both lanes, in EXPLICIT evaluation
//! order (nested parentheses, no `mul_add`). Rust/LLVM never contracts to FMA
//! without fast-math; nvcc contracts by DEFAULT, so the device mirror wraps
//! every arithmetic operator in `__dadd_rn` / `__dsub_rn` / `__dmul_rn` /
//! `__ddiv_rn`, which the CUDA C Programming Guide defines as non-contractible
//! round-to-nearest-even operations. Same expression tree + same inputs +
//! correctly rounded ops = same bits, by IEEE-754 determinism.
//!
//! WHAT WE CHANGED vs musl (all deliberate, none numeric on the contracted
//! domain): the floating-point-flag rituals (`force_eval!` for inexact and
//! underflow) are dropped — they exist to set FP status flags nobody reads and
//! have no device equivalent; `qm_tan` covers only (−π/2, π/2), so the
//! `rem_pio2` quadrant table collapses to `n = ±1`, folded in as literal ±1
//! (multiplying by ±1.0 is exact, so the surviving expressions are musl's); the
//! third reduction round is dropped with a numeric argument (see QM-TAN-2); and
//! the sign is peeled off first, which is exact because both kernels are
//! provably odd (see `qm_tan`).

// The polynomial tables are the published fdlibm/musl decimals, carried at their
// full printed width so a reader can diff them against the upstream file
// character by character; the hex in each comment is the intended bit pattern
// and `constants_match_published_bit_patterns` asserts the parse against it.
// Shortening them to the "shortest round-tripping" form would keep the value and
// destroy the audit, so `excessive_precision` is off for this module. Likewise
// `approx_constant`: PIO4/PIO2_1 ARE π/4 and a truncated π/2 — deliberately, as
// split double-double halves that `std::f64::consts` does not provide.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

// ---------------------------------------------------------------------------
// Constants — fdlibm/musl tables, verbatim.
// ---------------------------------------------------------------------------

/// Leading halves of `atan` at the four reduction anchors (1/2, 1, 3/2, ∞).
const ATANHI: [f64; 4] = [
    4.63647609000806093515e-01, /* atan(0.5)hi 0x3FDDAC670561BB4F */
    7.85398163397448278999e-01, /* atan(1.0)hi 0x3FE921FB54442D18 */
    9.82793723247329054082e-01, /* atan(1.5)hi 0x3FEF730BD281F69B */
    1.57079632679489655800e+00, /* atan(inf)hi 0x3FF921FB54442D18 */
];

/// Trailing halves of the same four anchors (`atan(a) = ATANHI + ATANLO`).
const ATANLO: [f64; 4] = [
    2.26987774529616870924e-17, /* atan(0.5)lo 0x3C7A2B7F222F65E2 */
    3.06161699786838301793e-17, /* atan(1.0)lo 0x3C81A62633145C07 */
    1.39033110312309984516e-17, /* atan(1.5)lo 0x3C7007887AF0CBBD */
    6.12323399573676603587e-17, /* atan(inf)lo 0x3C91A62633145C07 */
];

/// Minimax coefficients of `atan(t)/t − 1` on the reduced interval |t| ≤ 7/16
/// (fdlibm `aT[]`; the leading entries track the Taylor values 1/3, −1/5, 1/7 …).
const AT: [f64; 11] = [
    3.33333333333329318027e-01,  /* 0x3FD555555555550D */
    -1.99999999998764832476e-01, /* 0xBFC999999998EBC4 */
    1.42857142725034663711e-01,  /* 0x3FC24924920083FF */
    -1.11111104054623557880e-01, /* 0xBFBC71C6FE231671 */
    9.09088713343650656196e-02,  /* 0x3FB745CDC54C206E */
    -7.69187620504482999495e-02, /* 0xBFB3B0F2AF749A6D */
    6.66107313738753120669e-02,  /* 0x3FB10D66A0D03D51 */
    -5.83357013379057348645e-02, /* 0xBFADDE2D52DEFD9A */
    4.97687799461593236017e-02,  /* 0x3FA97B4B24760DEB */
    -3.65315727442169155270e-02, /* 0xBFA2B4442C6A6C2F */
    1.62858201153657823623e-02,  /* 0x3F90AD3AE322DA11 */
];

/// Minimax coefficients of `tan(t)/t − 1` on |t| ≤ 0.67434 (fdlibm `T[]`; the
/// leading entries track the Taylor values 1/3, 2/15, 17/315 …).
const T: [f64; 13] = [
    3.33333333333334091986e-01,  /* 0x3FD5555555555563 */
    1.33333333333201242699e-01,  /* 0x3FC111111110FE7A */
    5.39682539762260521377e-02,  /* 0x3FABA1BA1BB341FE */
    2.18694882948595424599e-02,  /* 0x3F9664F48406D637 */
    8.86323982359930005737e-03,  /* 0x3F8226E3E96E8493 */
    3.59207910759131235356e-03,  /* 0x3F6D6D22C9560328 */
    1.45620945432529025516e-03,  /* 0x3F57DBC8FEE08315 */
    5.88041240820264096874e-04,  /* 0x3F4344D8F2F26501 */
    2.46463134818469906812e-04,  /* 0x3F3026F71A8D1068 */
    7.81794442939557092300e-05,  /* 0x3F147E88A03792A6 */
    7.14072491382608190305e-05,  /* 0x3F12B80F32F0A7E9 */
    -1.85586374855275456654e-05, /* 0xBEF375CBDB605373 */
    2.59073051863633712884e-05,  /* 0x3EFB2A7074BF7AD4 */
];

/// π/4, split into a double and its residual for the `k_tan` fold.
const PIO4: f64 = 7.85398163397448278999e-01; /* 0x3FE921FB54442D18 */
const PIO4_LO: f64 = 3.06161699786838301793e-17; /* 0x3C81A62633145C07 */

/// π/2 as a three-term double-double: `PIO2_1` holds the first 33 significand
/// bits, `PIO2_1T` the rest, and `PIO2_2`/`PIO2_2T` split `PIO2_1T` again for the
/// second reduction round (fdlibm `e_rem_pio2.c`).
const PIO2_1: f64 = 1.57079632673412561417e+00; /* 0x3FF921FB54400000 */
const PIO2_1T: f64 = 6.07710050650619224932e-11; /* 0x3DD0B4611A626331 */
const PIO2_2: f64 = 6.07710050630396597660e-11; /* 0x3DD0B4611A600000 */
const PIO2_2T: f64 = 2.02226624879595063154e-21; /* 0x3BA3198A2E037073 */

// ---------------------------------------------------------------------------
// qm_atan
// ---------------------------------------------------------------------------

/// `atan(x)` in radians for every finite `f64`, bit-identical to `qm_atan` in
/// `noise-gpu/kernels/qm_shared_math.cuh`.
///
/// Exactly odd: `qm_atan(-x)` has the sign bit of `qm_atan(x)` flipped and all
/// other bits equal, for every non-NaN input (±0 included).
pub fn qm_atan(x: f64) -> f64 {
    let bits = x.to_bits();
    let negative = (bits >> 63) != 0;
    let ix = ((bits >> 32) as u32) & 0x7fff_ffff;

    // QM-ATAN-1  |x| ≥ 2^66: atan is π/2 to well under half an ulp, and NaN
    // passes through unchanged. (musl's `atanhi[3] + 0x1p-120` only exists to
    // raise the inexact flag; the sum rounds back to `ATANHI[3]`.)
    if ix >= 0x4410_0000 {
        if ix > 0x7ff0_0000 || (ix == 0x7ff0_0000 && (bits & 0xffff_ffff) != 0) {
            return x;
        }
        return if negative { -ATANHI[3] } else { ATANHI[3] };
    }

    // QM-ATAN-2  Reduce |x| into |t| ≤ 7/16 by one of four identities, recorded
    // in `anchor`:  −1 → t = x            (|x| < 7/16, no anchor)
    //                0 → t = (2x−1)/(2+x)  (atan(1/2) anchor,  7/16 ≤ |x| < 11/16)
    //                1 → t = (x−1)/(x+1)   (atan(1)   anchor, 11/16 ≤ |x| < 19/16)
    //                2 → t = (x−3/2)/(1+3x/2) (atan(3/2),     19/16 ≤ |x| < 39/16)
    //                3 → t = −1/x          (atan(∞),          39/16 ≤ |x| < 2^66)
    let mut t = x;
    let anchor: i32 = if ix < 0x3fdc_0000 {
        // |x| < 2^-27: atan(x) = x − x³/3 + … rounds to x, and this is also the
        // branch that carries ±0 through with its sign.
        if ix < 0x3e40_0000 {
            return x;
        }
        -1
    } else {
        t = f64::from_bits(bits & 0x7fff_ffff_ffff_ffff); // |x|, exact bit move
        if ix < 0x3ff3_0000 {
            if ix < 0x3fe6_0000 {
                t = (2.0 * t - 1.0) / (2.0 + t);
                0
            } else {
                t = (t - 1.0) / (t + 1.0);
                1
            }
        } else if ix < 0x4003_8000 {
            t = (t - 1.5) / (1.0 + 1.5 * t);
            2
        } else {
            t = -1.0 / t;
            3
        }
    };

    // QM-ATAN-3  z = t·t ;  z2 = z·z   (musl's `z` and `w`)
    let z = t * t;
    let z2 = z * z;
    // QM-ATAN-4  odd half of Σ AT[i]·z^(i+1), Horner in z2   (musl's `s1`)
    let odd_sum =
        z * (AT[0] + z2 * (AT[2] + z2 * (AT[4] + z2 * (AT[6] + z2 * (AT[8] + z2 * AT[10])))));
    // QM-ATAN-5  even half of the same sum   (musl's `s2`)
    let even_sum = z2 * (AT[1] + z2 * (AT[3] + z2 * (AT[5] + z2 * (AT[7] + z2 * AT[9]))));

    // QM-ATAN-6  unanchored branch: atan(t) = t − t·(odd+even), odd in t by
    // construction (z and z2 are even), which is what makes qm_atan exactly odd.
    if anchor < 0 {
        return t - t * (odd_sum + even_sum);
    }

    // QM-ATAN-7  anchored branch: atan(|x|) = ATANHI − ((t·(odd+even) − ATANLO) − t),
    // then the sign is restored by an exact negation.
    let a = anchor as usize;
    let result = ATANHI[a] - (t * (odd_sum + even_sum) - ATANLO[a] - t);
    if negative {
        -result
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// qm_tan
// ---------------------------------------------------------------------------

/// `tan(x)` for `x` in (−π/2, π/2) — the only range the node generator uses,
/// since every cell boundary is an angle produced by [`qm_atan`]. Bit-identical
/// to `qm_tan` in `noise-gpu/kernels/qm_shared_math.cuh`.
///
/// Outside the contract the function stays total and deterministic (the ±1
/// reduction keeps it correct out to |x| < 3π/4 and merely inaccurate beyond),
/// but only (−π/2, π/2) is promised. The debug assert is a Rust-side caller
/// guard with no numeric effect; the device mirror has no panic path and omits
/// it, which is the one deliberate structural difference between the lanes.
pub fn qm_tan(x: f64) -> f64 {
    debug_assert!(
        x.abs() < core::f64::consts::FRAC_PI_2,
        "qm_tan is contracted on (-pi/2, pi/2); got {x}"
    );
    let bits = x.to_bits();
    let ix = ((bits >> 32) as u32) & 0x7fff_ffff;

    // QM-TAN-1  |x| < 2^-27: tan(x) = x + x³/3 + … rounds to x. This branch also
    // carries ±0 through with its sign, which the odd polynomial below would not.
    if ix < 0x3e40_0000 {
        return x;
    }

    // Both kernels are exactly odd — every term is an odd power of the argument,
    // and the two bit moves (`zero_low_word`, absolute value) preserve the sign
    // bit — so peeling the sign off here and restoring it at the end reproduces
    // musl's signed paths bit for bit while halving the branches.
    let a = f64::from_bits(bits & 0x7fff_ffff_ffff_ffff);

    let magnitude = if ix <= 0x3fe9_21fb {
        // |x| ≲ π/4: the kernel's own domain, no reduction.
        qm_tan_kernel(a, 0.0, 0)
    } else {
        // QM-TAN-2  π/4 < |x| < π/2: reduce by ONE quadrant, y = |x| − π/2 < 0,
        // and return −1/tan(y) via the kernel's `odd` arm. This is musl's
        // `rem_pio2` medium path with the quadrant count folded in as the literal
        // 1 (`f_n · c` with `f_n = 1` is exact, so the expressions are musl's
        // unchanged). `r` is exact by Sterbenz: |x| stays inside
        // [PIO2_1/2, 2·PIO2_1] on this branch.
        //
        // The second round fires when the first one cancels away more than 16
        // binary digits — i.e. |x| within ~4e-5 of π/2 — because π/2 −
        // (PIO2_1+PIO2_1T) ≈ 2e-21 is then no longer negligible against y
        // itself (at the double nearest π/2, y ≈ 6.1e-17, and one round would be
        // wrong in the 5th digit). musl has a THIRD round for |x| ~ π/2·2^k with
        // k large; here it is dead code: after round two the residual is ~1e-37
        // absolute against a y that our domain bounds below by 6.12e-17, i.e.
        // ~1e-21 relative, four orders under an ulp. Dropped, not ported.
        let mut r = a - PIO2_1;
        let mut w = PIO2_1T;
        let mut y0 = r - w;
        let exponent_of_x = (ix >> 20) as i32;
        let exponent_of_y = ((y0.to_bits() >> 52) as i32) & 0x7ff;
        if exponent_of_x - exponent_of_y > 16 {
            let t = r;
            w = PIO2_2;
            r = t - w;
            w = PIO2_2T - ((t - r) - w);
            y0 = r - w;
        }
        let y1 = (r - y0) - w;
        qm_tan_kernel(y0, y1, 1)
    };

    if (bits >> 63) != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// fdlibm `__kernel_tan` in its musl form: `tan(x + y)` for |x| ≲ π/4 with `y`
/// the tail of the argument, or `−1/tan(x + y)` when `odd == 1`.
///
/// Local names are spelled out rather than kept at musl's single letters (and
/// nothing is shadowed) so the device mirror can use the SAME names in C, where
/// shadowing is unavailable: `z2` is musl's second `w`, `poly_hi`/`poly_lo` its
/// `r`/`v`, `tail` its reassigned `r`, `sum` its reassigned `w`.
fn qm_tan_kernel(x: f64, y: f64, odd: i32) -> f64 {
    let mut x = x;
    let mut y = y;
    let hx = (x.to_bits() >> 32) as u32;
    let big = (hx & 0x7fff_ffff) >= 0x3fe5_9428; // |x| ≥ 0.6744
    let negative = (hx >> 31) != 0;

    // QM-TAN-3  |x| ≥ 0.6744: fold through tan(π/4 − y) = (1−tan y)/(1+tan y) so
    // the polynomial is never evaluated past its 0.67434 fit interval.
    if big {
        if negative {
            x = -x;
            y = -y;
        }
        x = (PIO4 - x) + (PIO4_LO - y);
        y = 0.0;
    }

    // QM-TAN-4  z = x·x ; z2 = z·z ; the degree-27 odd polynomial split into two
    // Horner chains in z2 (fdlibm's even/odd break, kept verbatim).
    let z = x * x;
    let z2 = z * z;
    let poly_hi = T[1] + z2 * (T[3] + z2 * (T[5] + z2 * (T[7] + z2 * (T[9] + z2 * T[11]))));
    let poly_lo = z * (T[2] + z2 * (T[4] + z2 * (T[6] + z2 * (T[8] + z2 * (T[10] + z2 * T[12])))));

    // QM-TAN-5  tan(x+y) ≈ x + (T[0]·x³ + (x²·(poly) + y·(1+x²))), assembled in
    // fdlibm's order so the tail `y` enters before the leading term.
    let s = z * x;
    let tail = y + z * (s * (poly_hi + poly_lo) + y) + s * T[0];
    let sum = x + tail;

    // QM-TAN-6  folded branch: undo the π/4 fold with the two-term correction,
    // choosing tan or −1/tan by `odd`, then restore the sign.
    if big {
        let sign_flip = 1.0 - 2.0 * (odd as f64);
        let folded = sign_flip - 2.0 * (x + (tail - sum * sum / (sum + sign_flip)));
        return if negative { -folded } else { folded };
    }
    if odd == 0 {
        return sum;
    }

    // QM-TAN-7  cotangent arm: a plain `-1/(x+tail)` carries up to 2 ulp, so
    // refine it once — `sum_hi + correction = tail + x` splits the divisor,
    // `reciprocal_hi` is the truncated reciprocal, and the last line is one
    // Newton step in disguise.
    let sum_hi = zero_low_word(sum);
    let correction = tail - (sum_hi - x);
    let reciprocal = -1.0 / sum;
    let reciprocal_hi = zero_low_word(reciprocal);
    reciprocal_hi + reciprocal * (1.0 + reciprocal_hi * sum_hi + reciprocal_hi * correction)
}

/// Clear the low 32 significand bits — an exact, sign-preserving bit move that
/// splits a double into an exactly-representable head plus a tail.
fn zero_low_word(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & 0xffff_ffff_0000_0000)
}

// ---------------------------------------------------------------------------
// qm_atan2 / qm_wrap_pi — streaming-reducer decision helpers
// ---------------------------------------------------------------------------

/// Finite-input `atan2(y, x)`, built exclusively from [`qm_atan`] and exact
/// quadrant operations so the CPU and CUDA lanes return identical bits.
///
/// Non-finite inputs return one canonical quiet NaN. Streaming geometry treats
/// that value as a hard fault; canonicalising it keeps the diagnostic dump
/// lane-identical instead of preserving platform-specific NaN payloads.
pub fn qm_atan2(y: f64, x: f64) -> f64 {
    const PI: f64 = core::f64::consts::PI;
    const FRAC_PI_2: f64 = core::f64::consts::FRAC_PI_2;
    const CANONICAL_NAN: f64 = f64::from_bits(0x7ff8_0000_0000_0000);

    // QM-ATAN2-1  Reject non-finite geometry before any ratio can produce a
    // platform-specific NaN payload.
    if !x.is_finite() || !y.is_finite() {
        return CANONICAL_NAN;
    }

    let x_bits = x.to_bits();
    let y_bits = y.to_bits();
    let x_negative = (x_bits >> 63) != 0;
    let y_negative = (y_bits >> 63) != 0;

    // QM-ATAN2-2  Signed axes follow IEEE atan2 ownership exactly.
    if y == 0.0 {
        if x_negative {
            return if y_negative { -PI } else { PI };
        }
        return y;
    }
    if x == 0.0 {
        return if y_negative { -FRAC_PI_2 } else { FRAC_PI_2 };
    }

    let ax = f64::from_bits(x_bits & 0x7fff_ffff_ffff_ffff);
    let ay = f64::from_bits(y_bits & 0x7fff_ffff_ffff_ffff);

    // QM-ATAN2-3  Form a ratio <= 1 to avoid overflow/underflow at exponent
    // extremes. Both expressions are positive and qm_atan returns [0, pi/4].
    let acute = if ax >= ay {
        qm_atan(ay / ax)
    } else {
        FRAC_PI_2 - qm_atan(ax / ay)
    };

    // QM-ATAN2-4  Restore the quadrant, then the sign of y. Negation is exact.
    let magnitude = if x_negative { PI - acute } else { acute };
    if y_negative {
        -magnitude
    } else {
        magnitude
    }
}

/// Fold a finite source-span delta from `[-2pi, 2pi]` into `(-pi, pi]`.
///
/// The source-span constructor subtracts two [`qm_atan2`] results, so one
/// add/subtract is sufficient and avoids a platform `fmod` fork. Inputs outside
/// that contracted range or non-finite inputs return canonical NaN, which is a
/// hard geometry fault for the caller.
pub fn qm_wrap_pi(angle: f64) -> f64 {
    const PI: f64 = core::f64::consts::PI;
    const TAU: f64 = core::f64::consts::TAU;
    const CANONICAL_NAN: f64 = f64::from_bits(0x7ff8_0000_0000_0000);

    // QM-WRAP-1  The branch domain is part of the parity contract.
    if !angle.is_finite() || !(-TAU..=TAU).contains(&angle) {
        return CANONICAL_NAN;
    }
    // QM-WRAP-2  The negative seam maps to +pi, hence (-pi, pi].
    if angle > PI {
        angle - TAU
    } else if angle <= -PI {
        angle + TAU
    } else {
        angle
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::FRAC_PI_2;

    /// The sample generator both lanes share: a 64-bit LCG (the constants are
    /// Knuth's MMIX / PCG multiplier + increment). Deterministic, seedable, and
    /// trivial to re-implement in C, which is what the cross-lane harness does.
    struct Lcg(u64);

    impl Lcg {
        fn next_bits(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0
        }
        /// Uniform in [0, 1) from the top 53 bits.
        fn next_unit(&mut self) -> f64 {
            (self.next_bits() >> 11) as f64 * (1.0 / 9007199254740992.0)
        }
    }

    /// Log-uniform magnitudes 1e-300 .. 1e300, both signs — the `qm_atan` sample
    /// law from the brief.
    fn atan_sample(rng: &mut Lcg) -> f64 {
        let unit = rng.next_unit();
        let sign_bit = rng.next_bits() & 1;
        let magnitude = 10f64.powf(-300.0 + 600.0 * unit);
        if sign_bit == 0 {
            magnitude
        } else {
            -magnitude
        }
    }

    /// Uniform in (−π/2 + 1e-9, π/2 − 1e-9) — the `qm_tan` sample law.
    fn tan_sample(rng: &mut Lcg) -> f64 {
        let half_span = FRAC_PI_2 - 1e-9;
        -half_span + 2.0 * half_span * rng.next_unit()
    }

    /// `qm_tan`'s dispatch threshold and `qm_tan_kernel`'s fold threshold are
    /// HIGH-WORD comparisons, so the double they actually switch at is the first
    /// value of the next high-word bucket — not π/4 and not 0.6744, which sit
    /// ~3e-7 and ~1e-5 below them. Both are seeded by bit pattern, because a
    /// mirror that mistypes one hex digit of a threshold diverges only within a
    /// few ulp of exactly these numbers.
    const TAN_DISPATCH_THRESHOLD: f64 = f64::from_bits(0x3fe921fc_00000000);
    const TAN_FOLD_THRESHOLD: f64 = f64::from_bits(0x3fe59428_00000000);

    fn relative_error(got: f64, want: f64) -> f64 {
        if want == 0.0 {
            if got == 0.0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            ((got - want) / want).abs()
        }
    }

    /// Distance in representable doubles — the honest unit for "how far apart are
    /// two sub-ulp implementations of the same function".
    fn ulp_distance(got: f64, want: f64) -> i64 {
        let ordered = |v: f64| {
            let bits = v.to_bits() as i64;
            if bits < 0 {
                i64::MIN - bits
            } else {
                bits
            }
        };
        (ordered(got) - ordered(want)).abs()
    }

    /// A mistyped digit in a table above would move cell boundaries on both
    /// lanes silently. Every constant is pinned to the bit pattern printed in
    /// the fdlibm/musl source next to it.
    #[test]
    fn constants_match_published_bit_patterns() {
        let atanhi = [
            0x3FDDAC670561BB4Fu64,
            0x3FE921FB54442D18,
            0x3FEF730BD281F69B,
            0x3FF921FB54442D18,
        ];
        let atanlo = [
            0x3C7A2B7F222F65E2u64,
            0x3C81A62633145C07,
            0x3C7007887AF0CBBD,
            0x3C91A62633145C07,
        ];
        let at = [
            0x3FD555555555550Du64,
            0xBFC999999998EBC4,
            0x3FC24924920083FF,
            0xBFBC71C6FE231671,
            0x3FB745CDC54C206E,
            0xBFB3B0F2AF749A6D,
            0x3FB10D66A0D03D51,
            0xBFADDE2D52DEFD9A,
            0x3FA97B4B24760DEB,
            0xBFA2B4442C6A6C2F,
            0x3F90AD3AE322DA11,
        ];
        let t = [
            0x3FD5555555555563u64,
            0x3FC111111110FE7A,
            0x3FABA1BA1BB341FE,
            0x3F9664F48406D637,
            0x3F8226E3E96E8493,
            0x3F6D6D22C9560328,
            0x3F57DBC8FEE08315,
            0x3F4344D8F2F26501,
            0x3F3026F71A8D1068,
            0x3F147E88A03792A6,
            0x3F12B80F32F0A7E9,
            0xBEF375CBDB605373,
            0x3EFB2A7074BF7AD4,
        ];
        for (i, want) in atanhi.iter().enumerate() {
            assert_eq!(ATANHI[i].to_bits(), *want, "ATANHI[{i}]");
        }
        for (i, want) in atanlo.iter().enumerate() {
            assert_eq!(ATANLO[i].to_bits(), *want, "ATANLO[{i}]");
        }
        for (i, want) in at.iter().enumerate() {
            assert_eq!(AT[i].to_bits(), *want, "AT[{i}]");
        }
        for (i, want) in t.iter().enumerate() {
            assert_eq!(T[i].to_bits(), *want, "T[{i}]");
        }
        assert_eq!(PIO4.to_bits(), 0x3FE921FB54442D18, "PIO4");
        assert_eq!(PIO4_LO.to_bits(), 0x3C81A62633145C07, "PIO4_LO");
        assert_eq!(PIO2_1.to_bits(), 0x3FF921FB54400000, "PIO2_1");
        assert_eq!(PIO2_1T.to_bits(), 0x3DD0B4611A626331, "PIO2_1T");
        assert_eq!(PIO2_2.to_bits(), 0x3DD0B4611A600000, "PIO2_2");
        assert_eq!(PIO2_2T.to_bits(), 0x3BA3198A2E037073, "PIO2_2T");
        // The double-double splits are what they claim to be.
        assert_eq!(PIO2_1 + PIO2_1T, FRAC_PI_2, "PIO2_1 + PIO2_1T = pi/2");
        assert_eq!(PIO4 + PIO4_LO, core::f64::consts::FRAC_PI_4, "PIO4 split");
    }

    #[test]
    fn atan_hits_the_landmark_values() {
        assert_eq!(qm_atan(0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(qm_atan(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(qm_atan(1.0), core::f64::consts::FRAC_PI_4);
        assert_eq!(qm_atan(f64::INFINITY), FRAC_PI_2);
        assert_eq!(qm_atan(f64::NEG_INFINITY), -FRAC_PI_2);
        assert!(qm_atan(f64::NAN).is_nan());
    }

    #[test]
    fn atan2_signed_axes_quadrants_and_extreme_ratios_are_exact() {
        use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        for (y, x, want) in [
            (0.0, 1.0, 0.0),
            (-0.0, 1.0, -0.0),
            (0.0, -1.0, PI),
            (-0.0, -1.0, -PI),
            (1.0, 0.0, FRAC_PI_2),
            (-1.0, 0.0, -FRAC_PI_2),
            (1.0, 1.0, FRAC_PI_4),
            (-1.0, 1.0, -FRAC_PI_4),
            (1.0, -1.0, 3.0 * FRAC_PI_4),
            (-1.0, -1.0, -3.0 * FRAC_PI_4),
        ] {
            assert_eq!(qm_atan2(y, x).to_bits(), want.to_bits(), "y={y} x={x}");
        }
        assert!(qm_atan2(f64::MAX, f64::MIN_POSITIVE).is_finite());
        assert!(qm_atan2(f64::MIN_POSITIVE, f64::MAX).is_finite());
        assert_eq!(
            qm_atan2(f64::INFINITY, 1.0).to_bits(),
            0x7ff8_0000_0000_0000
        );
    }

    #[test]
    fn wrap_pi_owns_the_negative_seam_and_rejects_out_of_contract_input() {
        use core::f64::consts::{PI, TAU};
        assert_eq!(qm_wrap_pi(-PI).to_bits(), PI.to_bits());
        assert_eq!(qm_wrap_pi(PI).to_bits(), PI.to_bits());
        assert_eq!(qm_wrap_pi(TAU).to_bits(), 0.0f64.to_bits());
        assert_eq!(qm_wrap_pi(-TAU).to_bits(), 0.0f64.to_bits());
        assert_eq!(
            qm_wrap_pi(PI.next_up()).to_bits(),
            (-PI).next_up().to_bits()
        );
        assert_eq!(
            qm_wrap_pi((-PI).next_down()).to_bits(),
            PI.next_down().to_bits()
        );
        assert!(qm_wrap_pi(TAU.next_up()).is_nan());
    }

    /// Accuracy pin, `qm_atan` vs the platform libm over 10^7 log-uniform
    /// samples. Both are sub-ulp implementations of the same function, so the
    /// gap is a couple of ulp; the brief's gate is 5e-15, the MEASURED max is
    /// pinned an order tighter so a future edit that quietly degrades the
    /// polynomial trips this test instead of drifting.
    #[test]
    fn atan_matches_platform_libm_over_ten_million_samples() {
        let mut rng = Lcg(0x1234_5678_9abc_def0);
        let mut worst = 0.0f64;
        let mut worst_at = 0.0f64;
        let mut worst_ulp = 0i64;
        for _ in 0..10_000_000 {
            let x = atan_sample(&mut rng);
            let error = relative_error(qm_atan(x), x.atan());
            worst_ulp = worst_ulp.max(ulp_distance(qm_atan(x), x.atan()));
            if error > worst {
                worst = error;
                worst_at = x;
            }
        }
        println!("qm_atan vs libm: max rel {worst:e} ({worst_ulp} ulp) at x = {worst_at:e}");
        assert!(worst <= 5e-15, "max rel err {worst:e} at x = {worst_at:e}");
        assert!(
            worst <= 5e-16,
            "MEASURED PIN: max rel err was 2.05e-16 (1 ulp) when written, now {worst:e} at {worst_at:e}"
        );
    }

    /// Same pin for `qm_tan` over 10^7 uniform samples of the contracted range.
    /// The brief allows 5e-14 near the poles; measurement says the two libms
    /// stay within a few ulp everywhere, because the pole conditioning cancels
    /// out of a RELATIVE comparison of two accurate implementations.
    #[test]
    fn tan_matches_platform_libm_over_ten_million_samples() {
        let mut rng = Lcg(0x0fed_cba9_8765_4321);
        let mut worst = 0.0f64;
        let mut worst_at = 0.0f64;
        let mut worst_ulp = 0i64;
        for _ in 0..10_000_000 {
            let x = tan_sample(&mut rng);
            let error = relative_error(qm_tan(x), x.tan());
            worst_ulp = worst_ulp.max(ulp_distance(qm_tan(x), x.tan()));
            if error > worst {
                worst = error;
                worst_at = x;
            }
        }
        println!("qm_tan vs libm: max rel {worst:e} ({worst_ulp} ulp) at x = {worst_at:e}");
        assert!(worst <= 5e-14, "max rel err {worst:e} at x = {worst_at:e}");
        assert!(
            worst <= 5e-15,
            "MEASURED PIN: max rel err was 4.44e-16 (2 ulp) when written, now {worst:e} at {worst_at:e}"
        );
    }

    /// Round trip `s → u → s`, the actual node-boundary path: `u = qm_atan(x/h)`
    /// then back to `x/h = qm_tan(u)`.
    ///
    /// The brief asks for 1e-13 relative out to |x| ≤ 1e6, which double
    /// precision cannot deliver and no implementation could: `d(tan)/du = 1+x²`,
    /// so the half-ulp that rounding `u` costs reappears amplified by (1+x²)/x —
    /// ~7e-10 relative at x = 1e6. The test therefore gates BOTH ways: the
    /// brief's flat 1e-13 over the range where it is attainable (|x| ≤ 100), and
    /// the conditioning bound `4·eps·(1+x²)/|x|` over the full |x| ≤ 1e6.
    /// Physically this is a non-event: at x/h = 1e6 the cell is θ·d²/h long, so a
    /// 1e-10 relative wobble in the s-boundary is nanometres of a kilometre-long
    /// cell.
    #[test]
    fn tan_of_atan_round_trips() {
        const EPS: f64 = f64::EPSILON;
        let mut rng = Lcg(0xdead_beef_cafe_f00d);
        let mut worst_small = 0.0f64;
        let mut worst_ratio = 0.0f64;
        let mut worst_ratio_at = 0.0f64;
        for _ in 0..1_000_000 {
            // log-uniform |x| in 1e-6 .. 1e6, both signs
            let unit = rng.next_unit();
            let sign_bit = rng.next_bits() & 1;
            let magnitude = 10f64.powf(-6.0 + 12.0 * unit);
            let x = if sign_bit == 0 { magnitude } else { -magnitude };

            let error = relative_error(qm_tan(qm_atan(x)), x);
            if x.abs() <= 100.0 {
                worst_small = worst_small.max(error);
            }
            let conditioning = 4.0 * EPS * (1.0 + x * x) / x.abs();
            let ratio = error / conditioning.max(4.0 * EPS);
            if ratio > worst_ratio {
                worst_ratio = ratio;
                worst_ratio_at = x;
            }
        }
        println!(
            "tan(atan(x)) round trip: |x|<=100 worst rel {worst_small:e}; \
             worst/conditioning {worst_ratio:.4} at x = {worst_ratio_at:e}"
        );
        assert!(worst_small <= 1e-13, "|x| <= 100 worst {worst_small:e}");
        assert!(
            worst_ratio <= 1.0,
            "conditioning bound exceeded by {worst_ratio:.3}x at x = {worst_ratio_at:e}"
        );
    }

    /// Exact oddness — a bit test, not a tolerance. Sign symmetry of the u-frame
    /// is what keeps a piece's node count independent of which endpoint the
    /// caller labels A.
    #[test]
    fn atan_is_exactly_odd() {
        let mut rng = Lcg(0xa5a5_5a5a_1111_2222);
        for _ in 0..1_000_000 {
            let x = atan_sample(&mut rng);
            let positive = qm_atan(x);
            let negated = qm_atan(-x);
            assert_eq!(
                negated.to_bits(),
                positive.to_bits() ^ (1u64 << 63),
                "qm_atan({x:e}) = {positive:e} but qm_atan({:e}) = {negated:e}",
                -x
            );
        }
        // The reduction seams and the special values, exhaustively.
        for x in [
            0.0,
            f64::MIN_POSITIVE,
            1e-300,
            2f64.powi(-27),
            0.4375,
            0.6875,
            1.0,
            1.1875,
            1.5,
            2.4375,
            1e6,
            2f64.powi(66),
            f64::MAX,
            f64::INFINITY,
        ] {
            assert_eq!(
                qm_atan(-x).to_bits(),
                qm_atan(x).to_bits() ^ (1u64 << 63),
                "seam x = {x:e}"
            );
        }
    }

    /// Monotone on an ascending grid: 10^6 samples, dense around every reduction
    /// seam (where a branch change could invert a pair) and spread over ±1e8.
    /// A non-decreasing atan is what lets the generator sort cell boundaries by
    /// `u` and trust that the s-order matches.
    #[test]
    fn atan_is_monotone_on_a_million_ascending_samples() {
        let mut rng = Lcg(0x3141_5926_5358_9793);
        let mut grid: Vec<f64> = Vec::with_capacity(1_000_000);
        for _ in 0..900_000 {
            let unit = rng.next_unit();
            let sign_bit = rng.next_bits() & 1;
            let magnitude = 10f64.powf(-8.0 + 16.0 * unit);
            grid.push(if sign_bit == 0 { magnitude } else { -magnitude });
        }
        // 100k samples inside ±512 ulp of the five seams, both signs.
        for seam in [0.4375f64, 0.6875, 1.1875, 2.4375, 2f64.powi(-27)] {
            for step in -5_000i64..5_000 {
                let bits = seam.to_bits().wrapping_add(step as u64);
                let value = f64::from_bits(bits);
                grid.push(value);
                grid.push(-value);
            }
        }
        grid.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut previous = f64::NEG_INFINITY;
        for x in grid {
            let u = qm_atan(x);
            assert!(
                u >= previous,
                "qm_atan fell at x = {x:e}: {u:e} < {previous:e}"
            );
            previous = u;
        }
    }

    /// The same check for `qm_tan` over its contracted range, seams included
    /// (the k_tan fold threshold, the quadrant-reduction threshold, and the fold
    /// threshold as the reduced argument meets it).
    #[test]
    fn tan_is_monotone_on_a_million_ascending_samples() {
        let mut rng = Lcg(0x2718_2818_2845_9045);
        let mut grid: Vec<f64> = Vec::with_capacity(1_000_000);
        for _ in 0..900_000 {
            grid.push(tan_sample(&mut rng));
        }
        for seam in [
            TAN_FOLD_THRESHOLD,
            TAN_DISPATCH_THRESHOLD,
            FRAC_PI_2 - TAN_FOLD_THRESHOLD,
            2f64.powi(-27),
        ] {
            for step in -6_250i64..6_250 {
                let value = f64::from_bits(seam.to_bits().wrapping_add(step as u64));
                grid.push(value);
                grid.push(-value);
            }
        }
        grid.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut previous = f64::NEG_INFINITY;
        for x in grid {
            let t = qm_tan(x);
            assert!(
                t >= previous,
                "qm_tan fell at x = {x:e}: {t:e} < {previous:e}"
            );
            previous = t;
        }
    }

    /// Every double within `radius_ulps` of each seam, both signs — the sample
    /// law that actually finds a mirrored branch threshold typed one hex digit
    /// wrong, which uniform random sampling never would.
    fn seam_neighborhoods(seams: &[f64], radius_ulps: i64) -> Vec<f64> {
        let mut out = Vec::with_capacity(seams.len() * (2 * radius_ulps as usize + 1) * 2);
        for &seam in seams {
            for step in -radius_ulps..=radius_ulps {
                let value = f64::from_bits(seam.to_bits().wrapping_add(step as u64));
                out.push(value);
                out.push(-value);
            }
        }
        out
    }

    /// Cross-lane evidence generator. Writes 10^6 shared samples and their
    /// results as raw little-endian u64 quadruples
    /// `[x_bits, qm_atan(x)_bits, u_bits, qm_tan(u)_bits]` for
    /// `noise-gpu/kernels/qm_shared_math_bitcheck.c` (host compilation of the
    /// .cuh) and for `qm_shared_math_selftest.cu` on real hardware. The leading
    /// records walk every reduction seam of both functions ulp by ulp; the rest
    /// are the random laws above. Off by default — set `QM_SHARED_MATH_DUMP`:
    ///
    /// ```text
    /// QM_SHARED_MATH_DUMP=/tmp/qm.bin cargo test -p noise-compute shared_math -- --nocapture
    /// gcc -O2 -std=c99 -ffp-contract=off -o /tmp/qmbit \
    ///     ../noise-gpu/kernels/qm_shared_math_bitcheck.c
    /// /tmp/qmbit /tmp/qm.bin
    /// ```
    #[test]
    fn cross_lane_sample_dump() {
        let Ok(path) = std::env::var("QM_SHARED_MATH_DUMP") else {
            return;
        };

        // qm_atan: the four reduction anchors (7/16, 11/16, 19/16, 39/16), the
        // tiny and saturating cutoffs, the anchor points themselves, and the
        // exponent extremes.
        let mut atan_seam = seam_neighborhoods(
            &[
                2f64.powi(-27),
                0.4375,
                0.5,
                0.6875,
                1.0,
                1.1875,
                1.5,
                2.4375,
                2f64.powi(66),
                1e-300,
                1e300,
            ],
            2_000,
        );
        atan_seam.extend_from_slice(&[
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ]);

        // qm_tan: the tiny cutoff, the 0.6744 kernel fold (both directly and
        // through the reduction, where it sits at π/2 − 0.6744), the π/4
        // quadrant boundary, and the π/2 − 2^−k ladder that steps the second
        // reduction round's `ex − ey > 16` test across every exponent.
        let mut tan_seams: Vec<f64> = vec![
            2f64.powi(-27),
            TAN_FOLD_THRESHOLD,
            TAN_DISPATCH_THRESHOLD,
            core::f64::consts::FRAC_PI_4,
            // the same fold threshold as the REDUCED argument sees it
            FRAC_PI_2 - TAN_FOLD_THRESHOLD,
        ];
        for k in 1..=60 {
            tan_seams.push(FRAC_PI_2 - 2f64.powi(-k));
        }
        let mut tan_seam = seam_neighborhoods(&tan_seams, 400);
        // Approach the open domain edge strictly from below. The first
        // predecessor of π/2 is the nearest legal f64 argument.
        for step in 1..=20_000u64 {
            let value = f64::from_bits(FRAC_PI_2.to_bits() - step);
            tan_seam.push(value);
            tan_seam.push(-value);
        }
        // Wide ULP neighbourhoods around the closest range-reduction seams can
        // cross the open function domain. They are not legal qm_tan inputs.
        tan_seam.retain(|value| value.abs() < FRAC_PI_2);

        let mut rng = Lcg(0x5eed_0f00_d15e_a5e5);
        let mut bytes: Vec<u8> = Vec::with_capacity(1_000_000 * 32);
        for i in 0..1_000_000usize {
            let x = match atan_seam.get(i) {
                Some(&value) => value,
                None => atan_sample(&mut rng),
            };
            let u = match tan_seam.get(i) {
                Some(&value) => value,
                None => tan_sample(&mut rng),
            };
            for word in [
                x.to_bits(),
                qm_atan(x).to_bits(),
                u.to_bits(),
                qm_tan(u).to_bits(),
            ] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        std::fs::write(&path, &bytes).expect("dump path writable");
        println!(
            "wrote {} bytes: {} qm_atan seam samples, {} qm_tan seam samples, \
             the rest random, to {path}",
            bytes.len(),
            atan_seam.len(),
            tan_seam.len()
        );
    }
}
