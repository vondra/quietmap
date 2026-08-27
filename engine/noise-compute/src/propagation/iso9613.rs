//! ISO 9613-2 sound propagation core.
//!
//! Per-band attenuation: geometric divergence + atmospheric absorption + ground effect.
//! Additional effects (diffraction, screening, vegetation, reflection) in separate modules.

use crate::constants::*;
use crate::types::NUM_BANDS;
use std::sync::OnceLock;
use wide::{f64x4, CmpGt};

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
/// Replaces 40× std::exp() per source-receiver pair in propagate_variants().
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

/// SIMD variant of `fast_exp_f64` on 4 lanes. Same polynomial, same error bounds.
/// The `2^n` reconstruction is the only part that can't stay in-lane (bit
/// manipulation on the raw exponent bytes), so we extract and rebuild once.
#[inline(always)]
fn fast_exp_f64x4(x: f64x4) -> f64x4 {
    let x = x
        .fast_max(f64x4::splat(EXP_CLAMP_LO))
        .fast_min(f64x4::splat(EXP_CLAMP_HI));
    let inv_ln2 = f64x4::splat(std::f64::consts::LOG2_E);
    let ln2 = f64x4::splat(std::f64::consts::LN_2);
    let n_f = (x * inv_ln2).round();
    let r = x - n_f * ln2;
    let r2 = r * r;
    let c5 = f64x4::splat(1.0 / 120.0);
    let c4 = f64x4::splat(1.0 / 24.0);
    let c3 = f64x4::splat(1.0 / 6.0);
    let c2 = f64x4::splat(0.5);
    let one = f64x4::splat(1.0);
    let poly = one + r + r2 * (c2 + r * (c3 + r * (c4 + r * c5)));
    let n = n_f.to_array();
    let scale = f64x4::from([
        pow2_from_int(n[0] as i64),
        pow2_from_int(n[1] as i64),
        pow2_from_int(n[2] as i64),
        pow2_from_int(n[3] as i64),
    ]);
    poly * scale
}

/// Source geometry type — affects geometric divergence formula.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceGeometry {
    /// Line source (roads, railways): cylindrical spreading
    Line,
    /// Point source (buildings, industrial, wind turbines): spherical spreading
    Point,
}

/// Bare-earth geometry and ground factors for one direct CNOSSOS path.
///
/// `dp_m` is deliberately the horizontal source--receiver distance.  The
/// equivalent endpoint heights are measured against the same unweighted OLS
/// bare-earth plane used by the diffraction `δ*` fits; buildings and walls do
/// not enter either fit.  `ground_path_g` is the interval-weighted IMD path
/// factor and `source_ground_g` is the source-end factor required only by the
/// §2.5.14 near-source correction.
#[derive(Debug, Clone, Copy)]
pub struct GroundPath {
    pub dp_m: f64,
    pub zs_h_m: f64,
    pub zr_h_m: f64,
    pub ground_path_g: f64,
    pub source_ground_g: f64,
}

impl GroundPath {
    /// Construct a numerically safe direct path.  The production profile
    /// builder supplies positive receiver/source heights; this constructor
    /// also makes standalone fixtures well-defined at degenerate geometry.
    #[inline]
    pub fn new(
        dp_m: f64,
        zs_h_m: f64,
        zr_h_m: f64,
        ground_path_g: f64,
        source_ground_g: f64,
    ) -> Self {
        Self {
            dp_m: dp_m.max(1e-6),
            zs_h_m: zs_h_m.abs().max(0.05),
            zr_h_m: zr_h_m.abs().max(0.05),
            ground_path_g: ground_path_g.clamp(0.0, 1.0),
            source_ground_g: source_ground_g.clamp(0.0, 1.0),
        }
    }
}

/// CNOSSOS-EU 2015/996 §2.5.19 favourable-ray curvature coefficient.
pub const CNOSSOS_GROUND_ALPHA0: f64 = 2e-4;
/// CNOSSOS-EU 2015/996 §2.5.19 terrain-height term [1].
pub const CNOSSOS_GROUND_DELTA_ZT_COEFF: f64 = 6e-3;

/// Runtime-evaluated frequency powers used by the literal ground formula.
///
/// These are intentionally initialized through the target's `f64::powf`, not
/// as source-level constants: replacing repeated calls with a cached value is
/// exact only when the cache is formed by the same target libm evaluation in
/// the process's default floating-point environment.
#[derive(Clone, Copy)]
struct GroundBandConstants {
    frequency_hz: f64,
    frequency_pow_2_5: f64,
    frequency_pow_1_5: f64,
    frequency_pow_0_75: f64,
}

static GROUND_BAND_CONSTANTS: OnceLock<[GroundBandConstants; NUM_BANDS]> = OnceLock::new();

#[inline]
fn ground_band_constants() -> &'static [GroundBandConstants; NUM_BANDS] {
    GROUND_BAND_CONSTANTS.get_or_init(|| {
        std::array::from_fn(|band| {
            // Keep the fixed band literals on the target runtime's libm path;
            // `black_box` prevents build-time constant folding.
            let frequency_hz = std::hint::black_box(BAND_FREQ[band]);
            GroundBandConstants {
                frequency_hz,
                frequency_pow_2_5: frequency_hz.powf(2.5),
                frequency_pow_1_5: frequency_hz.powf(1.5),
                frequency_pow_0_75: frequency_hz.powf(0.75),
            }
        })
    })
}

/// Shared geometry and §2.5.14 factor for one direct CNOSSOS path.
#[derive(Clone, Copy)]
struct PreparedGroundPath {
    dp_m: f64,
    zs_h_m: f64,
    zr_h_m: f64,
    ground_path_g: f64,
    height_sum_m: f64,
    g_prime: f64,
}

/// State-specific geometry and impedance powers, prepared once per path.
#[derive(Clone, Copy)]
struct PreparedGroundState {
    zs_m: f64,
    zr_m: f64,
    gw13: f64,
    gw26: f64,
}

/// Prepare the band-invariant portion of the direct ground path.
#[inline]
fn prepare_ground_path(path: GroundPath) -> Option<PreparedGroundPath> {
    let g_path = path.ground_path_g;
    if g_path == 0.0 {
        // The standard states this case verbatim. Besides being exact, the
        // branch avoids the zero-impedance intermediate below.
        return None;
    }

    let height_sum_m = path.zs_h_m + path.zr_h_m;
    let test_form_h = path.dp_m / (30.0 * height_sum_m);
    // §2.5.14: at short paths blend the path ground with source ground.
    let g_prime = if test_form_h <= 1.0 {
        g_path * test_form_h + path.source_ground_g * (1.0 - test_form_h)
    } else {
        g_path
    };
    Some(PreparedGroundPath {
        dp_m: path.dp_m,
        zs_h_m: path.zs_h_m,
        zr_h_m: path.zr_h_m,
        ground_path_g: path.ground_path_g,
        height_sum_m,
        g_prime,
    })
}

/// Prepare one homogeneous or favourable state without changing its formula.
#[inline]
fn prepare_ground_state(path: &PreparedGroundPath, favourable: bool) -> PreparedGroundState {
    let (zs_m, zr_m, gw) = if favourable {
        let delta_zt = CNOSSOS_GROUND_DELTA_ZT_COEFF * path.dp_m / path.height_sum_m;
        let dp_sq_half = path.dp_m * path.dp_m * 0.5;
        let zs_f = path.zs_h_m
            + CNOSSOS_GROUND_ALPHA0 * (path.zs_h_m / path.height_sum_m).powi(2) * dp_sq_half
            + delta_zt;
        let zr_f = path.zr_h_m
            + CNOSSOS_GROUND_ALPHA0 * (path.zr_h_m / path.height_sum_m).powi(2) * dp_sq_half
            + delta_zt;
        (zs_f, zr_f, path.ground_path_g)
    } else {
        (path.zs_h_m, path.zr_h_m, path.g_prime)
    };
    let gw13 = gw.powf(1.3);
    let gw26 = gw13 * gw13;
    PreparedGroundState {
        zs_m,
        zr_m,
        gw13,
        gw26,
    }
}

/// Literal CNOSSOS ground attenuation for one prepared meteorological state.
///
/// `favourable=false` implements §2.5.15; `true` implements §2.5.20. The
/// latter deliberately uses `G_path` in the impedance term while both states
/// retain `G'_path` for the §2.5.18 lower bound, matching the standard's
/// §2.5.14/2.5.20 split.
#[inline]
fn ground_state_db(
    band_constants: &GroundBandConstants,
    path: &PreparedGroundPath,
    state: PreparedGroundState,
) -> f64 {
    let f = band_constants.frequency_hz;
    let k = 2.0 * std::f64::consts::PI * f / SPEED_OF_SOUND;
    // §2.5.17, then §2.5.16. Frequency powers are per-band constants;
    // `G^1.3` is prepared once per meteorological state.
    let w = 0.0185 * band_constants.frequency_pow_2_5 * state.gw26
        / (band_constants.frequency_pow_1_5 * state.gw26
            + 1.3e3 * band_constants.frequency_pow_0_75 * state.gw13
            + 1.16e6);
    let wd = w * path.dp_m;
    let cf = path.dp_m * (1.0 + 3.0 * wd * (-wd.sqrt()).exp()) / (1.0 + wd);
    let image_product = (state.zs_m * state.zs_m - (2.0 * cf / k).sqrt() * state.zs_m + cf / k)
        * (state.zr_m * state.zr_m - (2.0 * cf / k).sqrt() * state.zr_m + cf / k);
    let analytic = -10.0 * (4.0 * k * k / (path.dp_m * path.dp_m) * image_product).log10();
    // §2.5.18. The screen-specific split (§2.5.30--32) remains outside the
    // Quiet Map composite; callers still choose max(A_ground, A_barrier).
    analytic.max(GROUND_HARD_FLOOR_DB * (1.0 - path.g_prime))
}

/// Mix prepared homogeneous and favourable states using the engine convention.
#[inline]
fn mix_prepared_ground_states(
    band_constants: &GroundBandConstants,
    path: &PreparedGroundPath,
    homogeneous: PreparedGroundState,
    favourable: PreparedGroundState,
) -> f64 {
    let hom = ground_state_db(band_constants, path, homogeneous);
    let fav = ground_state_db(band_constants, path, favourable);
    let energy = P_FAV * 10.0_f64.powf(-fav / 10.0) + (1.0 - P_FAV) * 10.0_f64.powf(-hom / 10.0);
    -10.0 * energy.log10()
}

/// Literal homogeneous (§2.5.15) ground attenuation for fixtures and the
/// direct-state validator. Production propagation uses [`ground_atten_bands`],
/// which also includes §2.5.20.
#[inline]
pub fn cnossos_ground_homogeneous_atten_db(band: usize, path: GroundPath) -> f64 {
    let Some(prepared_path) = prepare_ground_path(path) else {
        return GROUND_HARD_FLOOR_DB;
    };
    let homogeneous = prepare_ground_state(&prepared_path, false);
    ground_state_db(&ground_band_constants()[band], &prepared_path, homogeneous)
}

/// [`cnossos_ground_homogeneous_atten_db`] over all octave bands.
#[inline]
pub fn cnossos_ground_homogeneous_atten_bands(path: GroundPath) -> [f64; NUM_BANDS] {
    let Some(prepared_path) = prepare_ground_path(path) else {
        return [GROUND_HARD_FLOOR_DB; NUM_BANDS];
    };
    let homogeneous = prepare_ground_state(&prepared_path, false);
    let band_constants = ground_band_constants();
    std::array::from_fn(|band| ground_state_db(&band_constants[band], &prepared_path, homogeneous))
}

/// Literal CNOSSOS direct-ground core, with homogeneous and favourable
/// propagation mixed energetically using the engine-wide `P_FAV` convention.
#[inline]
pub fn ground_atten_db(band: usize, path: GroundPath) -> f64 {
    let Some(prepared_path) = prepare_ground_path(path) else {
        // Both meteorological states are the standard's explicit hard-ground
        // case. Return it before the energy round-trip so the byte-stop bound
        // keeps its exact −3 dB anchor, not merely a 1-ULP approximation.
        return GROUND_HARD_FLOOR_DB;
    };
    let homogeneous = prepare_ground_state(&prepared_path, false);
    let favourable = prepare_ground_state(&prepared_path, true);
    mix_prepared_ground_states(
        &ground_band_constants()[band],
        &prepared_path,
        homogeneous,
        favourable,
    )
}

/// [`ground_atten_db`] for all octave bands.
#[inline]
pub fn ground_atten_bands(path: GroundPath) -> [f64; NUM_BANDS] {
    let Some(prepared_path) = prepare_ground_path(path) else {
        return [GROUND_HARD_FLOOR_DB; NUM_BANDS];
    };
    let homogeneous = prepare_ground_state(&prepared_path, false);
    let favourable = prepare_ground_state(&prepared_path, true);
    let band_constants = ground_band_constants();
    std::array::from_fn(|band| {
        mix_prepared_ground_states(
            &band_constants[band],
            &prepared_path,
            homogeneous,
            favourable,
        )
    })
}

/// Propagation baseline tied to the closest segment — divergence + the ground
/// factor the engine read from the raster. Atmospheric and ground *impacts*
/// belong on the Contributor (energy-weighted across all segments, derived
/// from [`propagate_variants_full`] deltas), not here.
pub fn compute_baseline(
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_g: f64,
) -> crate::types::PropagationBaseline {
    let d = d_slant.max(1.0);
    let geometric_db = -match source_geom {
        SourceGeometry::Line => 10.0 * (2.0 * std::f64::consts::PI * d).log10(),
        SourceGeometry::Point => 20.0 * d.log10() + 11.0,
    };
    crate::types::PropagationBaseline {
        geometric_db: (geometric_db * 10.0).round() / 10.0,
        ground_factor: ground_g,
    }
}

/// Former band-mean ground attenuation retained only for aircraft ground
/// operations and compatibility wrappers that have no geometry input.
///
/// Surface line and point propagation forms the literal per-band core exactly
/// once through [`ground_atten_bands`]. This scalar remains deliberately
/// isolated so aircraft ground operations stay byte-stable until they gain an
/// independent validation lane.
///
/// CNOSSOS-EU 2015/996 (2.5.15) + (2.5.18) read
/// `A_ground,H = max(analytic(G, f, h_s, h_r, d), −3(1 − Ḡm))`. The engine
/// carries the band-mean surrogate [`GROUND_CF`]`·G` in place of the analytic
/// term, so the same shape becomes
///
/// ```text
/// A_gr[i] = max(CF[i]·G, 0) − 3·(1 − G)
/// ```
///
/// The `max(·, 0)` is what keeps the two parts from stacking: (2.5.18) makes
/// `−3(1 − Ḡm)` a LOWER BOUND on `A_ground`, not an addend, so the surrogate's
/// negative low-frequency lobe is REPLACED by the floor instead of piling onto
/// it. Hard ground (`G = 0`) then lands on exactly −3 dB in every band, which
/// the standard states verbatim; `tests/tc_ground.rs` pins that against the
/// official TC01 and pins the `G > 0` totals against TC02/TC03.
#[inline]
fn band_mean_ground_atten_db(band: usize, ground_g: f64) -> f64 {
    (GROUND_CF[band] * ground_g).max(0.0) + GROUND_HARD_FLOOR_DB * (1.0 - ground_g)
}

/// Aircraft-ground-operations' explicitly carved-out band-mean formation.
///
/// Aircraft has no ground-core validation lane yet, so it is intentionally
/// insulated from the surface-source CNOSSOS upgrade.  Keep this as the only
/// aircraft call target; its byte stability is tested before any landing.
#[inline]
pub fn aircraft_ground_atten_db(band: usize, ground_g: f64) -> f64 {
    band_mean_ground_atten_db(band, ground_g)
}

/// Transitional band-mean compatibility helper. New surface propagation must
/// use [`ground_atten_db`] via a ray-derived [`GroundPath`]; this wrapper
/// remains only for old arc/testing callers until node evaluation retires that
/// return channel.
#[inline]
pub fn legacy_ground_atten_db(band: usize, ground_g: f64) -> f64 {
    band_mean_ground_atten_db(band, ground_g)
}

/// [`legacy_ground_atten_db`] over all octave bands.
#[inline]
pub fn legacy_ground_atten_bands(ground_g: f64) -> [f64; NUM_BANDS] {
    std::array::from_fn(|i| legacy_ground_atten_db(i, ground_g))
}

/// The ground/barrier term for one band (§7.3.1, as
/// [`propagate_variants_impl`] applies it): the barrier arm REPLACES the ground
/// arm when it exists, and `A_ground` alone stands when it does not.
///
/// Every angular quadrature of a line source averages THIS term rather than the
/// bare screening increment, because the two are not independent — a fan 14 %
/// blocked by 17 dB yields ~0.3 dB of average screening, which the `max` would
/// discard. So it lives here, once: `arc_screening` (adaptive nodes),
/// `seg_sampling` (uniform nodes) and the tile painter's cp path all have to
/// composite identically or their answers are not comparable.
#[inline]
pub fn ground_or_barrier_db(ground_db: f64, terrain_db: f64, screening_db: f64) -> f64 {
    let barrier = terrain_db + screening_db;
    if barrier > 0.0 {
        ground_db.max(barrier)
    } else {
        ground_db
    }
}

/// Propagation result per band.
#[derive(Debug, Clone)]
pub struct BandLevels {
    pub bands: [f64; NUM_BANDS],
    pub a_weighted: f64,
}

/// Compute received levels per band from emission bands and distance.
///
/// Applies: geometric divergence + atmospheric absorption + ground effect.
/// Caller adds: finite-line correction, diffraction, screening, vegetation, reflection.
pub fn propagate_bands(
    emission_bands: &[f64; NUM_BANDS],
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_g: f64,
) -> BandLevels {
    let mut bands = [0.0f64; NUM_BANDS];

    let d = d_slant.max(1.0);
    let geo_div = match source_geom {
        SourceGeometry::Line => 10.0 * (2.0 * std::f64::consts::PI * d).log10(),
        SourceGeometry::Point => 20.0 * d.log10() + 11.0,
    };
    let d_over_1000 = d_slant / 1000.0;

    for i in 0..NUM_BANDS {
        bands[i] = emission_bands[i]
            - geo_div
            - ALPHA_ATM[i] * d_over_1000
            - legacy_ground_atten_db(i, ground_g);
    }

    let a_weighted = a_weighted_total(&bands);
    BandLevels { bands, a_weighted }
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

/// ISO 9613-2 propagation core — computes 5 or 7 propagation variants in a
/// single SIMD pass.
///
/// `FULL=false` is the pipeline path: 5 variants (full / free_field / no_terrain
/// / no_screening / no_vegetation). Pipeline tile output only reads `full_energy`,
/// `free_field_energy`, and `band_energy`; the other variants feed per-effect
/// contribution visualizations.
///
/// `FULL=true` is the popup path: adds `no_ground` and `no_atmospheric` so the
/// contributor detail can compute A-weighted `ΔL_A` per effect. LLVM
/// monomorphizes the const generic so the pipeline path pays no cost for the
/// extra variants.
///
/// **Ground/barrier interaction (ISO 9613-2 §7.3.1):** when barriers (terrain
/// diffraction + building screening) are present, barrier attenuation REPLACES
/// ground effect: use `max(A_gr, A_bar)`, not `A_gr + A_bar`. Vegetation is
/// independent and always additive (ISO 9613-2:2024 A.2.2).
fn propagate_variants_impl<const FULL: bool>(
    emission_bands: &[f64; NUM_BANDS],
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_bands: &[f64; NUM_BANDS],
    terrain_atten: &[f64; NUM_BANDS],
    screening_atten: &[f64; NUM_BANDS],
    vegetation_atten: &[f64; NUM_BANDS],
    reflection_boost_db: f64,
    finite_line_corr: f64,
) -> crate::types::PropagationVariants {
    let mut full_energy = 0.0f64;
    let mut free_energy = 0.0f64;
    let mut no_terrain_energy = 0.0f64;
    let mut no_screening_energy = 0.0f64;
    let mut no_vegetation_energy = 0.0f64;
    let mut no_ground_energy = 0.0f64;
    let mut no_atmospheric_energy = 0.0f64;
    let mut band_energy = [0.0f64; NUM_BANDS];

    let d = d_slant.max(1.0);
    let geo_div = match source_geom {
        SourceGeometry::Line => 10.0 * (2.0 * std::f64::consts::PI * d).log10(),
        SourceGeometry::Point => 20.0 * d.log10() + 11.0,
    };
    let d_over_1000 = d_slant / 1000.0;

    let geo_v = f64x4::splat(geo_div);
    let d1000_v = f64x4::splat(d_over_1000);
    let refl_v = f64x4::splat(reflection_boost_db);
    let flc_v = f64x4::splat(finite_line_corr);
    let zero_v = f64x4::splat(0.0);
    let c_ln10_10 = f64x4::splat(std::f64::consts::LN_10 * 0.1);

    for half in 0..2 {
        let lo = half * 4;
        let load4 = |arr: &[f64; NUM_BANDS]| -> f64x4 {
            f64x4::from([arr[lo], arr[lo + 1], arr[lo + 2], arr[lo + 3]])
        };
        let emission_v = load4(emission_bands);
        let alpha_v = load4(&ALPHA_ATM);
        let aw_v = load4(&A_WEIGHTING);
        let terr_v = load4(terrain_atten);
        let scr_v = load4(screening_atten);
        let veg_v = load4(vegetation_atten);

        let base = emission_v - geo_v - alpha_v * d1000_v;
        let a_gr = load4(ground_bands);

        let a_bar_full = terr_v + scr_v;
        let gob_full = a_bar_full
            .cmp_gt(zero_v)
            .blend(a_gr.fast_max(a_bar_full), a_gr);
        let gob_nt = scr_v.cmp_gt(zero_v).blend(a_gr.fast_max(scr_v), a_gr);
        let gob_ns = terr_v.cmp_gt(zero_v).blend(a_gr.fast_max(terr_v), a_gr);

        let free = base - a_gr + flc_v;
        let full = base - gob_full - veg_v + refl_v + flc_v;
        let no_t = base - gob_nt - veg_v + refl_v + flc_v;
        let no_s = base - gob_ns - veg_v + refl_v + flc_v;
        let no_v = base - gob_full + refl_v + flc_v;

        let full_e = fast_exp_f64x4((full + aw_v) * c_ln10_10);
        let free_e = fast_exp_f64x4((free + aw_v) * c_ln10_10);
        let no_t_e = fast_exp_f64x4((no_t + aw_v) * c_ln10_10);
        let no_s_e = fast_exp_f64x4((no_s + aw_v) * c_ln10_10);
        let no_v_e = fast_exp_f64x4((no_v + aw_v) * c_ln10_10);

        let full_arr = full_e.to_array();
        full_energy += full_arr[0] + full_arr[1] + full_arr[2] + full_arr[3];
        band_energy[lo] = full_arr[0];
        band_energy[lo + 1] = full_arr[1];
        band_energy[lo + 2] = full_arr[2];
        band_energy[lo + 3] = full_arr[3];

        let free_arr = free_e.to_array();
        free_energy += free_arr[0] + free_arr[1] + free_arr[2] + free_arr[3];
        let no_t_arr = no_t_e.to_array();
        no_terrain_energy += no_t_arr[0] + no_t_arr[1] + no_t_arr[2] + no_t_arr[3];
        let no_s_arr = no_s_e.to_array();
        no_screening_energy += no_s_arr[0] + no_s_arr[1] + no_s_arr[2] + no_s_arr[3];
        let no_v_arr = no_v_e.to_array();
        no_vegetation_energy += no_v_arr[0] + no_v_arr[1] + no_v_arr[2] + no_v_arr[3];

        if FULL {
            // `no_ground`: replace ground/barrier max() with just the barrier
            // term (ground set to 0). When no barrier, gob_no_g = 0 → path
            // keeps veg/refl/flc only. Over soft ground at 63/125 Hz CF[i] is
            // negative (LF boost), so `no_ground` can be LOUDER than `full` —
            // callers must not clamp the signed delta.
            let gob_no_g = a_bar_full.cmp_gt(zero_v).blend(a_bar_full, zero_v);
            let no_g = base - gob_no_g - veg_v + refl_v + flc_v;
            // `no_atmospheric`: full + atm added back per band.
            let no_a = full + alpha_v * d1000_v;

            let no_g_e = fast_exp_f64x4((no_g + aw_v) * c_ln10_10);
            let no_a_e = fast_exp_f64x4((no_a + aw_v) * c_ln10_10);
            let no_g_arr = no_g_e.to_array();
            no_ground_energy += no_g_arr[0] + no_g_arr[1] + no_g_arr[2] + no_g_arr[3];
            let no_a_arr = no_a_e.to_array();
            no_atmospheric_energy += no_a_arr[0] + no_a_arr[1] + no_a_arr[2] + no_a_arr[3];
        }
    }

    crate::types::PropagationVariants {
        full_energy,
        free_field_energy: free_energy,
        no_terrain_energy,
        no_screening_energy,
        no_vegetation_energy,
        no_ground_energy,
        no_atmospheric_energy,
        band_energy,
    }
}

/// Pipeline kernel — 5 variants, no per-effect ground/atmospheric breakdown.
/// Hot path in `tile-painter` batch compute; popup callers use
/// `propagate_variants_full` instead.
#[inline]
pub fn propagate_variants(
    emission_bands: &[f64; NUM_BANDS],
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_g: f64,
    terrain_atten: &[f64; NUM_BANDS],
    screening_atten: &[f64; NUM_BANDS],
    vegetation_atten: &[f64; NUM_BANDS],
    reflection_boost_db: f64,
    finite_line_corr: f64,
) -> crate::types::PropagationVariants {
    let ground_bands = legacy_ground_atten_bands(ground_g);
    propagate_variants_impl::<false>(
        emission_bands,
        d_slant,
        source_geom,
        &ground_bands,
        terrain_atten,
        screening_atten,
        vegetation_atten,
        reflection_boost_db,
        finite_line_corr,
    )
}

/// Popup kernel — 7 variants including `no_ground` and `no_atmospheric` so the
/// contributor detail can derive A-weighted `ΔL_A` per effect. Pays ~40% more
/// per-band work than the pipeline kernel; never call from the pipeline hot
/// path.
#[inline]
pub fn propagate_variants_full(
    emission_bands: &[f64; NUM_BANDS],
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_g: f64,
    terrain_atten: &[f64; NUM_BANDS],
    screening_atten: &[f64; NUM_BANDS],
    vegetation_atten: &[f64; NUM_BANDS],
    reflection_boost_db: f64,
    finite_line_corr: f64,
) -> crate::types::PropagationVariants {
    let ground_bands = legacy_ground_atten_bands(ground_g);
    propagate_variants_impl::<true>(
        emission_bands,
        d_slant,
        source_geom,
        &ground_bands,
        terrain_atten,
        screening_atten,
        vegetation_atten,
        reflection_boost_db,
        finite_line_corr,
    )
}

/// Popup kernel with the analytic CNOSSOS ground core.  This is intentionally
/// a separate entrypoint while aircraft ground operations retain their
/// validated band-mean formation; all non-aircraft source callers must pass a
/// ray-specific [`GroundPath`] rather than silently falling back to receiver G.
#[inline]
pub fn propagate_variants_cnossos_ground_full(
    emission_bands: &[f64; NUM_BANDS],
    d_slant: f64,
    source_geom: SourceGeometry,
    ground_path: GroundPath,
    terrain_atten: &[f64; NUM_BANDS],
    screening_atten: &[f64; NUM_BANDS],
    vegetation_atten: &[f64; NUM_BANDS],
    reflection_boost_db: f64,
    finite_line_corr: f64,
) -> crate::types::PropagationVariants {
    let ground_bands = ground_atten_bands(ground_path);
    propagate_variants_impl::<true>(
        emission_bands,
        d_slant,
        source_geom,
        &ground_bands,
        terrain_atten,
        screening_atten,
        vegetation_atten,
        reflection_boost_db,
        finite_line_corr,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propagation_k4_hard_ground() {
        // K4: Propagation 100m, G=0 (hard), line source
        // Expected attenuation: 25.56 dB
        // Use reference emission from K1: cat1, 50 km/h, 10000 AADT
        // (We test attenuation = emission_aw - received_aw)
        //
        // WAS 28.58 dB, and that number was the BUG, not the reference: it is
        // 25.56 + 3.00, the value this kernel produced while `A_ground` was
        // `CF[i]·G` alone and therefore vanished over hard ground. ISO 9613-2
        // Table 3 and CNOSSOS (2.5.15) both put `A_ground = −3 dB` at G = 0
        // (`GROUND_HARD_FLOOR_DB`), so 25.56 dB is what the cited standard
        // actually gives. Recomputed with the fix, never re-fitted; the
        // per-band anchor lives in `tests/tc_ground.rs` against official TC01.
        let emission = [65.0, 70.0, 73.0, 74.0, 73.0, 70.0, 65.0, 58.0];
        let em_aw = a_weighted_total(&emission);

        let result = propagate_bands(&emission, 100.0, SourceGeometry::Line, 0.0);
        let attenuation = em_aw - result.a_weighted;

        // Allow ±1 dB tolerance (our emission approximation, not exact CNOSSOS coefficients)
        assert!(
            (attenuation - 25.56).abs() < 1.5,
            "K4: expected ~25.56, got {:.2} (emission_aw={:.2}, received_aw={:.2})",
            attenuation,
            em_aw,
            result.a_weighted
        );
    }

    #[test]
    fn aircraft_carveout_keeps_the_band_mean_bytes() {
        // Aircraft ground operations are deliberately exempt from the surface
        // core until they own an independent validation lane.  Pin the exact
        // formation here so a later surface migration cannot alter it through
        // a shared helper by accident; HM3 A/B remains the landing receipt.
        for g in [0.0, 0.1, 0.5, 0.9, 1.0] {
            for band in 0..NUM_BANDS {
                assert_eq!(
                    aircraft_ground_atten_db(band, g).to_bits(),
                    band_mean_ground_atten_db(band, g).to_bits(),
                    "aircraft carve-out drifted at G={g}, band={band}"
                );
            }
        }
    }

    #[test]
    fn literal_ground_core_keeps_the_existing_three_db_gain_bound() {
        // In each state the literal core is `max(analytic, -3(1-Gm))`.
        // Both G inputs are clamped, §2.5.14 is their convex blend, hence
        // `Gm ∈ [0,1]` and every state is >= -3 dB.  The P_FAV energy mix of
        // values >= -3 dB is also >= -3 dB.  This grid pins the implemented
        // arithmetic at the domain corners; the accompanying derivation is
        // the review/landing authority for the continuous domain.
        for &dp_m in &[1e-6, 0.01, 1.0, 30.0, 300.0, 3_000.0, 10_000.0] {
            // 15,615.5 m is the current worst raw building-source midpoint;
            // this regression pins that a bad OSM height cannot invalidate the
            // byte-stop ground-gain bound while a separate data policy handles
            // whether that source itself is plausible.
            for &zs_h_m in &[0.05, 0.5, 1.5, 10.0, 105.0, 175.0, 414.0, 15_615.5] {
                for &ground_path_g in &[0.0, 0.01, 0.5, 0.99, 1.0] {
                    for &source_ground_g in &[0.0, 0.5, 1.0] {
                        let path = GroundPath::new(
                            dp_m,
                            zs_h_m,
                            DEFAULT_RECEIVER_HEIGHT,
                            ground_path_g,
                            source_ground_g,
                        );
                        for band in 0..NUM_BANDS {
                            assert!(
                                ground_atten_db(band, path) >= -GROUND_GAIN_UB_DB - 1e-12,
                                "dp={dp_m}, zs={zs_h_m}, Gpath={ground_path_g}, Gsource={source_ground_g}, band={band}"
                            );
                        }
                    }
                }
            }
        }
        let hard = GroundPath::new(100.0, 0.05, DEFAULT_RECEIVER_HEIGHT, 0.0, 1.0);
        assert_eq!(
            ground_atten_db(0, hard).to_bits(),
            GROUND_HARD_FLOOR_DB.to_bits(),
            "hard-ground P_FAV arithmetic must retain the byte-exact −3 dB anchor"
        );
    }

    #[test]
    fn literal_core_uses_source_end_g_and_the_barrier_replaces_it() {
        // §2.5.14 is observable only on a short path: the same path mean with
        // opposite source-end IMD must not collapse back to the old scalar G.
        let hard_source = GroundPath::new(1.0, 1.0, 4.0, 0.5, 0.0);
        let soft_source = GroundPath::new(1.0, 1.0, 4.0, 0.5, 1.0);
        let delta = (0..NUM_BANDS)
            .map(|band| {
                (cnossos_ground_homogeneous_atten_db(band, hard_source)
                    - cnossos_ground_homogeneous_atten_db(band, soft_source))
                .abs()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            delta > 0.5,
            "source-end G' correction vanished: {delta:.3} dB"
        );

        let emission = [70.0; NUM_BANDS];
        let zero = [0.0; NUM_BANDS];
        let opaque_barrier = [100.0; NUM_BANDS];
        let v = propagate_variants_cnossos_ground_full(
            &emission,
            200.0,
            SourceGeometry::Line,
            GroundPath::new(200.0, 1.0, 4.0, 1.0, 1.0),
            &zero,
            &opaque_barrier,
            &zero,
            0.0,
            0.0,
        );
        assert_eq!(
            v.full_energy.to_bits(),
            v.no_ground_energy.to_bits(),
            "an opaque barrier must replace ground, not add to it"
        );
    }

    #[test]
    fn test_propagation_ground_effect() {
        // Soft ground (G=1) should attenuate MORE than hard ground (G=0)
        let emission = [70.0; NUM_BANDS];

        let hard = propagate_bands(&emission, 100.0, SourceGeometry::Line, 0.0);
        let soft = propagate_bands(&emission, 100.0, SourceGeometry::Line, 1.0);

        // Ground effect delta should be ~3 dB (K5 reference: 3.08 dB)
        let delta = hard.a_weighted - soft.a_weighted;
        assert!(
            delta > 1.0 && delta < 6.0,
            "Ground effect delta: expected ~3 dB, got {:.2}",
            delta
        );
    }

    #[test]
    fn test_point_vs_line() {
        // Point source divergence should be higher than line source at same distance
        let emission = [70.0; NUM_BANDS];

        let line = propagate_bands(&emission, 100.0, SourceGeometry::Line, 0.0);
        let point = propagate_bands(&emission, 100.0, SourceGeometry::Point, 0.0);

        // Point = 20·log₁₀(100)+11 = 51.0 dB
        // Line = 10·log₁₀(2π·100) = 27.98 dB
        // Point attenuates ~23 dB more
        assert!(
            line.a_weighted > point.a_weighted,
            "Line should be louder than point at same distance"
        );
        let diff = line.a_weighted - point.a_weighted;
        assert!(diff > 20.0 && diff < 26.0, "diff={:.1}", diff);
    }

    #[test]
    fn test_variants_matches_propagate_bands() {
        // propagate_variants with zero attenuation should match propagate_bands
        let emission = [65.0, 70.0, 73.0, 74.0, 73.0, 70.0, 65.0, 58.0];
        let zero = [0.0f64; NUM_BANDS];

        let bands = propagate_bands(&emission, 100.0, SourceGeometry::Line, 0.5);
        let variants = propagate_variants(
            &emission,
            100.0,
            SourceGeometry::Line,
            0.5,
            &zero,
            &zero,
            &zero,
            0.0,
            0.0,
        );

        let full_db = crate::types::PropagationVariants::to_db(variants.full_energy);
        assert!(
            (full_db - bands.a_weighted).abs() < 0.01,
            "full={:.2} vs bands={:.2}",
            full_db,
            bands.a_weighted
        );
    }

    #[test]
    fn test_variants_free_equals_full_with_zero_atten() {
        // With zero terrain/screening/vegetation: full == free
        let emission = [70.0; NUM_BANDS];
        let zero = [0.0f64; NUM_BANDS];

        let v = propagate_variants(
            &emission,
            200.0,
            SourceGeometry::Point,
            0.5,
            &zero,
            &zero,
            &zero,
            0.0,
            0.0,
        );

        let full_db = crate::types::PropagationVariants::to_db(v.full_energy);
        let free_db = crate::types::PropagationVariants::to_db(v.free_field_energy);
        assert!(
            (full_db - free_db).abs() < 0.01,
            "full={:.2} free={:.2} should be equal with zero atten",
            full_db,
            free_db
        );
    }

    #[test]
    fn test_variants_barrier_replaces_ground_not_adds() {
        // ISO 9613-2 §7.3.1: barrier replaces ground, use max(A_gr, A_bar)
        let emission = [70.0; NUM_BANDS];
        let zero = [0.0f64; NUM_BANDS];
        let big_barrier = [15.0; NUM_BANDS]; // 15 dB barrier (bigger than ground ~3 dB)

        // With big barrier: should use barrier (15 dB), NOT ground+barrier (18 dB)
        let v = propagate_variants(
            &emission,
            200.0,
            SourceGeometry::Line,
            1.0, // soft ground G=1 → A_gr ~3 dB
            &big_barrier,
            &zero,
            &zero,
            0.0,
            0.0,
        );
        // Free-field uses ground only
        let free_db = crate::types::PropagationVariants::to_db(v.free_field_energy);
        let full_db = crate::types::PropagationVariants::to_db(v.full_energy);

        // Barrier effect: free - full ≈ 15 - 3 = 12 dB (barrier minus ground, since max() picks barrier)
        // If we had A_gr + A_bar (bug): free - full ≈ 15 + 3 - 3 = 15 dB (over-attenuated)
        let effect = free_db - full_db;
        assert!(
            effect > 8.0 && effect < 14.0,
            "barrier effect = {:.1} dB, expected ~12 (not ~15 if double-counted)",
            effect
        );
    }

    #[test]
    fn test_variants_small_barrier_uses_ground() {
        // When barrier < ground effect, ground wins
        let emission = [70.0; NUM_BANDS];
        let zero = [0.0f64; NUM_BANDS];
        let small_barrier = [1.0; NUM_BANDS]; // 1 dB barrier (smaller than ground ~3 dB)

        let v = propagate_variants(
            &emission,
            200.0,
            SourceGeometry::Line,
            1.0, // soft ground G=1 → A_gr ~3 dB
            &small_barrier,
            &zero,
            &zero,
            0.0,
            0.0,
        );
        let free_db = crate::types::PropagationVariants::to_db(v.free_field_energy);
        let full_db = crate::types::PropagationVariants::to_db(v.full_energy);

        // Ground wins: effect should be ~0 (full uses max(3, 1) = 3 = same as free's ground)
        let effect = free_db - full_db;
        assert!(
            effect.abs() < 1.0,
            "small barrier effect = {:.1} dB, expected ~0 (ground dominates)",
            effect
        );
    }

    #[test]
    fn test_variants_terrain_reduces_noise() {
        let emission = [70.0; NUM_BANDS];
        let zero = [0.0f64; NUM_BANDS];
        let terrain = [5.0; NUM_BANDS]; // 5 dB terrain diffraction per band

        let v = propagate_variants(
            &emission,
            200.0,
            SourceGeometry::Line,
            0.5,
            &terrain,
            &zero,
            &zero,
            0.0,
            -3.0,
        );

        let full_db = crate::types::PropagationVariants::to_db(v.full_energy);
        let no_terrain_db = crate::types::PropagationVariants::to_db(v.no_terrain_energy);

        // Without terrain should be louder
        assert!(
            no_terrain_db > full_db,
            "no_terrain={:.1} should be louder than full={:.1}",
            no_terrain_db,
            full_db
        );
        // Difference should be roughly 5 dB (not exact due to A-weighting)
        assert!(
            (no_terrain_db - full_db) > 2.0 && (no_terrain_db - full_db) < 8.0,
            "terrain effect = {:.1} dB",
            no_terrain_db - full_db
        );
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

    #[test]
    fn test_pipeline_full_agree_on_shared_variants() {
        // Split kernel contract: `propagate_variants` (pipeline path,
        // FULL=false) and `propagate_variants_full` (popup path, FULL=true)
        // MUST produce byte-identical energies for the 5 shared variants.
        // Guards the pipeline hash-regression invariant.
        let emission = [65.0, 70.0, 73.0, 74.0, 73.0, 70.0, 65.0, 58.0];
        let terrain = [2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let screening = [1.5; NUM_BANDS];
        let vegetation = [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        for geom in [SourceGeometry::Line, SourceGeometry::Point] {
            for &g in &[0.0, 0.5, 1.0] {
                let pipe = propagate_variants(
                    &emission,
                    150.0,
                    geom,
                    g,
                    &terrain,
                    &screening,
                    &vegetation,
                    0.5,
                    -2.0,
                );
                let full = propagate_variants_full(
                    &emission,
                    150.0,
                    geom,
                    g,
                    &terrain,
                    &screening,
                    &vegetation,
                    0.5,
                    -2.0,
                );
                assert_eq!(pipe.full_energy.to_bits(), full.full_energy.to_bits());
                assert_eq!(
                    pipe.free_field_energy.to_bits(),
                    full.free_field_energy.to_bits()
                );
                assert_eq!(
                    pipe.no_terrain_energy.to_bits(),
                    full.no_terrain_energy.to_bits()
                );
                assert_eq!(
                    pipe.no_screening_energy.to_bits(),
                    full.no_screening_energy.to_bits()
                );
                assert_eq!(
                    pipe.no_vegetation_energy.to_bits(),
                    full.no_vegetation_energy.to_bits()
                );
                for i in 0..NUM_BANDS {
                    assert_eq!(pipe.band_energy[i].to_bits(), full.band_energy[i].to_bits());
                }
                // Pipeline path leaves no_ground / no_atmospheric at zero;
                // popup path populates them.
                assert_eq!(pipe.no_ground_energy, 0.0);
                assert_eq!(pipe.no_atmospheric_energy, 0.0);
                assert!(full.no_ground_energy > 0.0);
                assert!(full.no_atmospheric_energy > 0.0);
            }
        }
    }

    #[test]
    fn test_variants_full_monotonicity() {
        // ISO 9613-2 physical invariants:
        //  - atm / vegetation absorb per band → removing them ⇒ LOUDER
        //  - terrain / screening ≥ 0 per band → removing them ⇒ LOUDER
        //  - ground is signed (CF[i] < 0 at 63/125 Hz over soft ground) →
        //    removing it can be QUIETER; never assert a bound.
        let emission = [70.0; NUM_BANDS];
        let zero = [0.0f64; NUM_BANDS];
        let terrain = [4.0; NUM_BANDS];
        let screening = [2.0; NUM_BANDS];
        let vegetation = [3.0; NUM_BANDS];
        let v = propagate_variants_full(
            &emission,
            500.0,
            SourceGeometry::Line,
            0.3,
            &terrain,
            &screening,
            &vegetation,
            0.0,
            -1.0,
        );
        assert!(v.no_terrain_energy > v.full_energy);
        assert!(v.no_screening_energy > v.full_energy);
        assert!(v.no_vegetation_energy > v.full_energy);
        assert!(v.no_atmospheric_energy > v.full_energy);
        // Sanity: no_ground is finite (sign unrestricted).
        assert!(v.no_ground_energy.is_finite() && v.no_ground_energy > 0.0);

        // Zero-attenuation case: no_terrain / no_screening / no_vegetation
        // collapse onto `full` (removing a zero effect changes nothing).
        let v0 = propagate_variants_full(
            &emission,
            500.0,
            SourceGeometry::Line,
            0.3,
            &zero,
            &zero,
            &zero,
            0.0,
            -1.0,
        );
        assert!((v0.no_terrain_energy - v0.full_energy).abs() < 1e-9 * v0.full_energy);
        assert!((v0.no_screening_energy - v0.full_energy).abs() < 1e-9 * v0.full_energy);
        assert!((v0.no_vegetation_energy - v0.full_energy).abs() < 1e-9 * v0.full_energy);
    }

    #[test]
    fn test_variants_add() {
        let mut a = crate::types::PropagationVariants {
            full_energy: 1.0,
            free_field_energy: 2.0,
            no_terrain_energy: 3.0,
            no_screening_energy: 4.0,
            no_vegetation_energy: 5.0,
            no_ground_energy: 6.0,
            no_atmospheric_energy: 7.0,
            band_energy: [0.0; crate::types::NUM_BANDS],
        };
        let b = crate::types::PropagationVariants {
            full_energy: 10.0,
            free_field_energy: 20.0,
            no_terrain_energy: 30.0,
            no_screening_energy: 40.0,
            no_vegetation_energy: 50.0,
            no_ground_energy: 60.0,
            no_atmospheric_energy: 70.0,
            band_energy: [0.0; crate::types::NUM_BANDS],
        };
        a.add(&b);
        assert!((a.full_energy - 11.0).abs() < 1e-10);
        assert!((a.no_vegetation_energy - 55.0).abs() < 1e-10);
    }

    #[test]
    fn test_atmospheric_absorption() {
        // At 8 kHz, 1km: 58.4 dB absorption. Should dominate at distance.
        let emission = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 80.0]; // only 8 kHz
        let r100 = propagate_bands(&emission, 100.0, SourceGeometry::Point, 0.0);
        let r1000 = propagate_bands(&emission, 1000.0, SourceGeometry::Point, 0.0);

        // At 8 kHz: geometric + atmospheric
        // 100m: atm = 58.4 × 0.1 = 5.84 dB
        // 1000m: atm = 58.4 × 1.0 = 58.4 dB
        // Difference in atm alone: 52.56 dB
        let diff = r100.bands[7] - r1000.bands[7];
        assert!(diff > 50.0, "8kHz 100m→1km diff={:.1} (expected >50)", diff);
    }
}
