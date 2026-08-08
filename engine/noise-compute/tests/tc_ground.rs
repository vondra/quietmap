//! Ground attenuation against the official CNOSSOS test cases TC01/TC02/TC03
//! (ISO/TR 17534-4 scenes as published by NoiseModelling —
//! `AttenuationComputeOutputCnossosTest.java` + `reference_cnossos.json`,
//! `Direct/LH` rows, transcribed verbatim below).
//!
//! Scene, identical in all three (only `groundCoefficient` changes):
//! S (10, 10, z 1) over zGround 0, R (200, 50, z 4) over zGround 0, flat, no
//! screens; projected path length 194.16 m; Lw = 93 dB in every octave band.
//! The reference's own decomposition is `L_H = Lw − A_div − A_atm − A_ground,H`
//! with `A_div = 56.76` dB, so `A_ground,H` is recovered exactly from the
//! published `L_H` and the ISO 9613-1 alpha the reference uses.
//!
//! WHAT IS PINNED AND WHAT IS NOT. The standard states `A_ground,H` for hard
//! ground VERBATIM — 2015/996 (2.5.15) "if Gpath = 0: Aground,H = −3 dB", and
//! (2.5.18) makes `−3(1 − Ḡm)` the lower bound of the governing max(). Those
//! two are laws and are asserted band by band. The value at G > 0 comes from
//! the full analytic (2.5.15) form (frequency × heights × distance); the engine
//! carries a band-mean surrogate (`GROUND_CF[i]·G`) in its place, so TC02/TC03
//! CANNOT match per band and are pinned on the A-weighted total with the
//! surrogate's measured deviation written out. Widening a tolerance here to
//! make a run pass is how the −3 dB went missing in the first place.
//!
//! `cargo test --release --test tc_ground`

use noise_compute::constants::{ALPHA_ATM, GROUND_CF, GROUND_GAIN_UB_DB, GROUND_HARD_FLOOR_DB};
use noise_compute::propagation::iso9613::{
    a_weighted_total, ground_atten_bands, propagate_bands, propagate_variants, SourceGeometry,
};
use noise_compute::types::{PropagationVariants, NUM_BANDS};

/// Projected source→receiver distance of the TC scene [m].
const D: f64 = 194.16;
/// Sound power in every octave band [dB].
const LW: [f64; NUM_BANDS] = [93.0; NUM_BANDS];
/// The reference's geometric divergence for this scene [dB].
const REF_A_DIV: f64 = 56.76;
/// ISO 9613-1 atmospheric absorption used by the reference [dB/km]. The engine
/// ships its own rounded table (`ALPHA_ATM`), so every comparison below is
/// rebuilt on the ENGINE's alpha — that isolates the ground term, which is what
/// this file is about.
const REF_ALPHA: [f64; NUM_BANDS] = [0.12, 0.41, 1.04, 1.93, 3.66, 9.66, 32.77, 116.88];

/// `reference_cnossos.json` → `TC0n.Direct.LH`, verbatim.
const TC01_LH: [f64; NUM_BANDS] = [39.21, 39.16, 39.03, 38.86, 38.53, 37.36, 32.87, 16.54];
const TC02_LH: [f64; NUM_BANDS] = [37.71, 37.66, 37.53, 35.01, 29.82, 35.86, 31.37, 15.04];
const TC03_LH: [f64; NUM_BANDS] = [36.21, 36.16, 34.45, 26.19, 30.49, 34.36, 29.87, 13.54];

/// Per-band tolerance on the quantities the standard states verbatim [dB]. The
/// reference publishes two decimals, so 0.1 is ~5× its own rounding.
const TOL_DB: f64 = 0.1;

/// `A_ground,H` recovered from a published `L_H` row through the reference's
/// own divergence and alpha.
fn ref_a_ground(lh: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|i| LW[i] - REF_A_DIV - REF_ALPHA[i] * D / 1000.0 - lh[i])
}

/// The reference's `L_H` rebuilt on the ENGINE's alpha table — the only fair
/// total to compare the engine against, since the alpha difference is a
/// separate (documented) engine choice and would otherwise be charged to the
/// ground term.
fn ref_lh_on_engine_alpha(a_ground: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|i| LW[i] - REF_A_DIV - ALPHA_ATM[i] * D / 1000.0 - a_ground[i])
}

/// `A_ground` the propagation kernel actually applied, read back out of
/// `propagate_bands` — proves the shared term is wired into the kernel, not
/// merely available as a helper.
fn kernel_a_ground(ground_g: f64) -> [f64; NUM_BANDS] {
    let out = propagate_bands(&LW, D, SourceGeometry::Point, ground_g);
    let geo = 20.0 * D.log10() + 11.0;
    std::array::from_fn(|i| LW[i] - geo - ALPHA_ATM[i] * D / 1000.0 - out.bands[i])
}

fn fmt(v: &[f64; NUM_BANDS]) -> String {
    v.iter()
        .map(|x| format!("{x:6.2}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// TC01, hard ground: `A_ground,H = −3 dB` in EVERY band — 2015/996 (2.5.15)
/// quoted literally, ISO 9613-2 Table 3 (`As + Ar = −1.5 − 1.5`) independently.
/// This is the assertion the engine failed: `GROUND_CF[i]·G` collapses to 0.00
/// at G = 0, i.e. the whole 3 dB image-source gain over reflective ground went
/// missing in every band of every source layer.
#[test]
fn tc01_hard_ground_is_minus_three_db_in_every_band() {
    let reference = ref_a_ground(&TC01_LH);
    let helper = ground_atten_bands(0.0);
    let kernel = kernel_a_ground(0.0);
    for i in 0..NUM_BANDS {
        assert!(
            (reference[i] - GROUND_HARD_FLOOR_DB).abs() < TOL_DB,
            "reference transcription is wrong: TC01 band {i} backs out to {:.2}, \
             standard says {GROUND_HARD_FLOOR_DB}",
            reference[i]
        );
        assert!(
            (helper[i] - reference[i]).abs() < TOL_DB,
            "TC01 band {i}: engine A_ground = {:.2}, reference = {:.2}\n  engine    {}\n  reference {}",
            helper[i],
            reference[i],
            fmt(&helper),
            fmt(&reference)
        );
        assert!(
            (kernel[i] - reference[i]).abs() < TOL_DB,
            "TC01 band {i}: kernel applied A_ground = {:.2}, reference = {:.2}\n  kernel    {}\n  reference {}",
            kernel[i],
            reference[i],
            fmt(&kernel),
            fmt(&reference)
        );
    }
}

/// TC01 A-weighted total, on the engine's own alpha. With `A_ground` exactly
/// −3 dB in all eight bands the engine must land ON the reference here — there
/// is nothing else left to differ by.
#[test]
fn tc01_a_weighted_total_matches_reference() {
    let reference = ref_lh_on_engine_alpha(&ref_a_ground(&TC01_LH));
    let engine = propagate_bands(&LW, D, SourceGeometry::Point, 0.0);
    let (got, want) = (engine.a_weighted, a_weighted_total(&reference));
    assert!(
        (got - want).abs() < TOL_DB,
        "TC01 L_H total: engine {got:.2} dB(A), reference {want:.2} dB(A)\n  \
         engine    {}\n  reference {}",
        fmt(&engine.bands),
        fmt(&reference)
    );
}

/// TC02 (G = 0.5) and TC03 (G = 1.0): the band-mean surrogate cannot reproduce
/// the analytic (2.5.15) per-band shape — the reference puts 5.71 dB into the
/// 1 kHz band at G = 0.5 and 9.68 dB into 500 Hz at G = 1.0, where a single
/// number per band gives −0.50 and 2.50. What IS comparable is the A-weighted
/// total, and this is where the surrogate stands as of the hard-ground fix:
///
/// ```text
///        engine   reference   gap
/// TC02    41.54     40.74    +0.80
/// TC03    39.29     38.91    +0.38
/// ```
///
/// (before the fix: −0.69 and +0.39 — the fix is exact at G = 0, neutral at
/// G = 1, and trades a 0.69 dB undershoot for a 0.80 dB overshoot at G = 0.5).
/// The 1.0 dB bound is the surrogate's documented error budget, NOT a claim of
/// per-band agreement; tightening the model must tighten this number, never
/// loosen it.
#[test]
fn tc02_tc03_a_weighted_total_within_surrogate_budget() {
    const SURROGATE_BUDGET_DB: f64 = 1.0;
    for (name, lh, g) in [("TC02", TC02_LH, 0.5), ("TC03", TC03_LH, 1.0)] {
        let reference = ref_lh_on_engine_alpha(&ref_a_ground(&lh));
        let engine = propagate_bands(&LW, D, SourceGeometry::Point, g);
        let gap = engine.a_weighted - a_weighted_total(&reference);
        assert!(
            gap.abs() < SURROGATE_BUDGET_DB,
            "{name} (G={g}) L_H total gap {gap:+.2} dB exceeds the surrogate budget\n  \
             engine A_ground    {}\n  reference A_ground {}",
            fmt(&ground_atten_bands(g)),
            fmt(&ref_a_ground(&lh))
        );
    }
}

/// (2.5.18): `A_ground,H,min = −3(1 − Ḡm)` is the LOWER BOUND of the governing
/// max(), so no band may sit below it at any ground factor — and hard ground
/// must sit exactly ON it (the max()'s other arm is 0 there). TC02's reference
/// row shows the bound biting in 6 of its 8 bands, which is why it has to be a
/// floor rather than an addend.
#[test]
fn ground_attenuation_respects_the_cnossos_lower_bound() {
    for step in 0..=20 {
        let g = step as f64 / 20.0;
        let floor = GROUND_HARD_FLOOR_DB * (1.0 - g);
        let bands = ground_atten_bands(g);
        for i in 0..NUM_BANDS {
            assert!(
                bands[i] >= floor - 1e-12,
                "G={g}: band {i} A_ground {:.4} below the (2.5.18) floor {floor:.4}",
                bands[i]
            );
            // Bands whose surrogate term is non-positive sit ON the floor.
            if GROUND_CF[i] <= 0.0 {
                assert!(
                    (bands[i] - floor).abs() < 1e-12,
                    "G={g}: band {i} (CF={}) must equal the floor {floor:.4}, got {:.4}",
                    GROUND_CF[i],
                    bands[i]
                );
            }
        }
    }
}

/// The energy-budget skip in `tile-painter` bounds a source's contribution from
/// above by assuming the most FAVOURABLE ground it could possibly meet, i.e.
/// `+GROUND_GAIN_UB_DB` dB of gain. If that bound under-states the real gain the
/// skip stops being sound and the pipeline drops audible sources with no trace.
/// Pin it: the deepest `A_ground` over the whole `G ∈ [0,1] × band` domain is
/// exactly `−GROUND_GAIN_UB_DB`, attained at G = 0 in every band.
#[test]
fn ground_gain_upper_bound_is_tight_and_sound() {
    let mut deepest = f64::INFINITY;
    for step in 0..=1000 {
        let g = step as f64 / 1000.0;
        for a in ground_atten_bands(g) {
            deepest = deepest.min(a);
        }
    }
    assert!(
        deepest >= -GROUND_GAIN_UB_DB - 1e-12,
        "GROUND_GAIN_UB_DB {GROUND_GAIN_UB_DB} is UNSOUND: A_ground reaches {deepest:.4}"
    );
    assert!(
        (deepest + GROUND_GAIN_UB_DB).abs() < 1e-12,
        "GROUND_GAIN_UB_DB {GROUND_GAIN_UB_DB} is slack: deepest A_ground is {deepest:.4}"
    );
}

/// Both propagation lanes carry the same ground term: the scalar
/// `propagate_bands` and the SIMD `propagate_variants` free-field arm (which is
/// divergence + atmosphere + ground, no barriers/vegetation/reflection).
#[test]
fn simd_and_scalar_lanes_apply_the_same_ground_term() {
    let zero = [0.0f64; NUM_BANDS];
    for &g in &[0.0, 0.25, 0.5, 0.75, 1.0] {
        let scalar = propagate_bands(&LW, D, SourceGeometry::Point, g);
        let simd = propagate_variants(
            &LW,
            D,
            SourceGeometry::Point,
            g,
            &zero,
            &zero,
            &zero,
            0.0,
            0.0,
        );
        let simd_db = PropagationVariants::to_db(simd.free_field_energy);
        assert!(
            (simd_db - scalar.a_weighted).abs() < 1e-9,
            "G={g}: SIMD free-field {simd_db:.6} != scalar {:.6}",
            scalar.a_weighted
        );
    }
}
