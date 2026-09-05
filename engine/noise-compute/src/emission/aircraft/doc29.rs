//! Doc 29 4th Edition airborne SEL kernel.
//!
//! Master equation (Eq. 4-8b):
//!   SEL_seg = L_E(P, d_p) + ΔV + ΔI(φ) - Λ(β, l) + ΔF
//!
//! `segment_energy_kernel` is the shared per-segment hot path; both the
//! popup wrapper (`segment_sel::*`) and tile-painter scatter delegate
//! here.

use std::f64::consts::LOG10_2;

use grid::geo::wrapped_longitude_delta;

use crate::propagation::iso9613::fast_exp_f64;
use crate::types::AircraftSegment;

use super::horizon::ReceiverHorizon;
use super::screening::BuildingHorizon;

use super::npd::{Installation, NpdLuts, NpdProfile, AIRCRAFT_FAR_FIELD_THRESHOLD_M, FT_PER_M};

// Doc 29 reference value — slightly higher precision than the
// crate-wide `crate::constants::M_PER_DEG_LAT` (110_540.0) used by the
// general geo helpers. Re-exported via `aircraft::*` so external callers
// (e.g. tile-painter's per-sub-seg line-perpendicular tile-envelope
// prune) project lat/lon to local meters consistent with the kernel's
// own distance math.
pub const M_PER_DEG_LAT: f64 = 111_132.92;

/// Slant² from receiver to the segment evaluated at the same unclamped CPA
/// foot the airborne kernel uses (`segment_energy_kernel`). Conservative
/// pre-filter for the source-reader R-tree loop: rejecting on `> reach²`
/// never drops a real contributor because both this helper and the kernel
/// use identical lateral CPA + altitude-extrapolation geometry, so they
/// produce the same `slant²` for the same `(seg, receiver)`. The kernel
/// can still drop the segment via Filter D — that path stays in the
/// kernel; the pre-filter never falsely rejects.
///
/// Earlier versions clamped altitude to `[min_alt, max_alt]`, which over-
/// estimated slant² for descending/ascending segments whose extrapolated
/// CPA foot crossed receiver elevation. /gg consensus (Codex + Gemini)
/// caught the false-negative band; this formulation matches the kernel
/// exactly.
///
/// `cos_lat` is hoisted by the caller (one cos per query, not per segment).
#[inline]
pub fn segment_min_slant_sq(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    cos_lat: f64,
) -> f64 {
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;

    let x1 = wrapped_longitude_delta(rx_lon, seg.start_lon) * m_per_deg_lon;
    let y1 = (seg.start_lat - rx_lat) * M_PER_DEG_LAT;
    let y2 = (seg.end_lat - rx_lat) * M_PER_DEG_LAT;

    let dx = wrapped_longitude_delta(seg.start_lon, seg.end_lon) * m_per_deg_lon;
    let dy = y2 - y1;
    let seg_len_sq = dx * dx + dy * dy;
    // Same `inv_lsq * ...` formulation the kernel uses (FP-equivalent to
    // `... / seg_len_sq` only modulo 1 ULP, so we mirror exactly). For
    // degenerate segments (sub-mm horizontal length) `inv_lsq = 0` makes
    // `t = 0`, which is also what the kernel sees — pre-filter never
    // takes a different branch from the kernel.
    let inv_lsq = if seg_len_sq > 1e-6 {
        1.0 / seg_len_sq
    } else {
        0.0
    };
    let t = -(x1 * dx + y1 * dy) * inv_lsq;
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    let lateral_sq = cx * cx + cy * cy;

    let start_alt = seg.start_alt_m as f64;
    let sdz = seg.end_alt_m as f64 - start_alt;
    let rel_alt = start_alt + t * sdz - rx_elev_m;

    lateral_sq + rel_alt * rel_alt
}

/// CPA result for one segment-receiver pair.
pub struct CpaResult {
    pub q_m: f64,            // signed distance from S1 to perpendicular foot
    pub d_p_m: f64,          // perpendicular slant distance (for NPD lookup)
    pub lateral_m: f64,      // horizontal distance to ground track extension
    pub relative_alt_m: f64, // signed altitude at foot relative to receiver
    pub beta_deg: f64,       // elevation angle from ground plane
    pub seg_len_m: f64,      // segment length
    pub t: f64,              // parametric projection (0..1 inside segment, else extrapolation)
}

/// Compute CPA on INFINITE segment extension (Doc 29 §4.4.1).
/// t is NOT clamped — this is the key correctness fix over all other implementations.
#[inline]
pub fn compute_cpa(
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    s1_lat: f64,
    s1_lon: f64,
    s1_alt_m: f64,
    s2_lat: f64,
    s2_lon: f64,
    s2_alt_m: f64,
) -> CpaResult {
    let cos_lat = rx_lat.to_radians().cos().max(0.2);
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;

    let x1 = wrapped_longitude_delta(rx_lon, s1_lon) * m_per_deg_lon;
    let y1 = (s1_lat - rx_lat) * M_PER_DEG_LAT;
    let y2 = (s2_lat - rx_lat) * M_PER_DEG_LAT;

    let dx = wrapped_longitude_delta(s1_lon, s2_lon) * m_per_deg_lon;
    let dy = y2 - y1;
    let seg_len_sq = dx * dx + dy * dy;
    let seg_len = seg_len_sq.sqrt().max(1.0);

    // Parametric projection — NO CLAMP (infinite-line CPA per Doc 29).
    let t = if seg_len_sq > 1e-6 {
        -(x1 * dx + y1 * dy) / seg_len_sq
    } else {
        0.5
    };

    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    let lateral_m = (cx * cx + cy * cy).sqrt();

    let relative_alt_m = s1_alt_m + t * (s2_alt_m - s1_alt_m) - rx_elev_m;
    let d_p_m = (lateral_m * lateral_m + relative_alt_m * relative_alt_m).sqrt();
    let q_m = t * seg_len;

    let beta_deg = if lateral_m > 0.01 || relative_alt_m.abs() > 0.01 {
        relative_alt_m.atan2(lateral_m).to_degrees()
    } else {
        90.0
    };

    CpaResult {
        q_m,
        d_p_m,
        lateral_m,
        relative_alt_m,
        beta_deg,
        seg_len_m: seg_len,
        t,
    }
}

/// Clamped closest approach for DISPLAY ONLY (popup Lmax / CPA distance /
/// altitude). `compute_cpa` keeps the unclamped infinite-line foot because the
/// SEL energy integral needs it — ΔF reads `q_m = t·seg_len` (Doc 29 §4.5.6) —
/// but that foot is NOT a physical aircraft position: for a curving departure,
/// one short segment's straight line extended km past its endpoints lands a
/// far/high aircraft "tens of m away / m overhead" (flight 39d311 over LKPR:
/// 7.2 km away, 1509 m AMSL, shown as 36 m / 30 m). This returns the closest
/// the aircraft ACTUALLY reaches while ON the observed segment (`t` clamped to
/// [0,1]): foot inside → identical to `d_p_m`/`relative_alt_m`; foot outside →
/// distance to the nearer endpoint. NEVER feed this to SEL — energy stays on
/// the unclamped `CpaResult`. `sdz = end_alt_m - start_alt_m` is the only input
/// `CpaResult` doesn't already carry.
#[inline]
pub fn clamped_display_cpa(cpa: &CpaResult, sdz: f64) -> (f64, f64) {
    if (0.0..=1.0).contains(&cpa.t) {
        return (cpa.d_p_m, cpa.relative_alt_m);
    }
    let tc = cpa.t.clamp(0.0, 1.0);
    // `back` = signed segment-fractions from the clamped endpoint out to the
    // unclamped foot; `along_m` is that offset in the ground plane (⊥ to the
    // perpendicular `lateral_m`), so horizontal distance is Pythagoras and the
    // altitude winds back along the climb gradient.
    let back = cpa.t - tc;
    let along_m = back * cpa.seg_len_m;
    let rel_alt = cpa.relative_alt_m - back * sdz;
    (
        (cpa.lateral_m * cpa.lateral_m + along_m * along_m + rel_alt * rel_alt).sqrt(),
        rel_alt,
    )
}

/// ΔV = 10 × log10(V_ref / V_seg) (Doc 29 §4.5.1, Eq. 4-14).
#[inline]
pub fn delta_v(speed_kt: f64, profile: &NpdProfile) -> f64 {
    if speed_kt > 10.0 {
        10.0 * (profile.v_ref_kt / speed_kt).log10()
    } else {
        0.0
    }
}

// `fast_atan` / `fast_delta_f` / `fast_lateral_attenuation` replace libm
// `atan` (~50-80 cycles) with a Padé [3/2] approximation (~8 cycles) at the
// cost of < 0.05 dB error per ΔF and < 0.15 dB per Λ. Pipeline ships all
// tile output through these, popup follows so popup ≡ tile parity is by
// construction (same approximations, same regime).

/// Padé [3/2] atan for |x| ≤ 1. Max error 0.0034 rad (0.19°).
#[inline(always)]
fn fast_atan_small(x: f64) -> f64 {
    let x2 = x * x;
    x * (1.0 + 0.1827 * x2) / (1.0 + 0.5124 * x2)
}

/// `atan(x)` via Padé [3/2] over the full real line. Max error ~0.003 rad.
#[inline(always)]
pub fn fast_atan(x: f64) -> f64 {
    if x.abs() > 1.0 {
        return x.signum() * std::f64::consts::FRAC_PI_2 - fast_atan_small(1.0 / x);
    }
    fast_atan_small(x)
}

/// Padé-atan variant of the exact ΔF (Doc 29 §4.5.6, Eq. 4-20). Max error vs exact: < 0.05 dB.
#[inline]
pub fn fast_delta_f(q_m: f64, seg_len_m: f64, d_bar_m: f64) -> f64 {
    if seg_len_m < 1.0 || d_bar_m < 1.0 {
        return 0.0;
    }
    let alpha1 = -q_m / d_bar_m;
    let alpha2 = -(q_m - seg_len_m) / d_bar_m;

    let g1 = alpha1 / (1.0 + alpha1 * alpha1) + fast_atan(alpha1);
    let g2 = alpha2 / (1.0 + alpha2 * alpha2) + fast_atan(alpha2);
    let f = (g2 - g1) * std::f64::consts::FRAC_1_PI;

    // log2 × 10·log10(2) ≡ 10·log10 at f64 precision; same trick the NPD
    // lookup uses (see `let log_d = d_ft.log2() * LOG10_2` above). libm
    // log10 was ~8 % of total airborne runtime in perf record on Dobříš
    // (call site verified via `perf annotate`); log2 is ~2× cheaper.
    (10.0 * LOG10_2) * f.max(1e-15).log2()
}

/// Fast lateral attenuation. Computes `β` via `fast_atan` (no atan2) from
/// `rel_alt / lateral_m`, then applies the Doc 29 §4.5.4 Γ × Λ(β) formula
/// using the same Doc 29 piecewise. Skips the `atan2` call (caller passes
/// rel_alt + lateral instead of beta_deg + lateral). For `airport_ground`
/// callers Λ collapses to 0 — this kernel does the same gate.
/// Max error vs exact: < 0.15 dB per segment.
///
/// Λ(β) ≠ 0 only for **Wing-mounted jets** per Doc 29 §4.5.4 Eq. 4-19a /
/// FAA AEDT TM §6.2.4. Fuselage-mounted engines and propeller installations
/// (including helicopters) get Λ = 0 — engine-airframe geometry doesn't
/// produce wing-shielding lateral attenuation in those cases.
#[inline]
pub fn fast_lateral_attenuation(
    rel_alt: f64,
    lateral_m: f64,
    airport_ground: bool,
    installation: Installation,
) -> f64 {
    if airport_ground || !matches!(installation, Installation::Wing) {
        return 0.0;
    }

    let beta_deg = fast_atan(rel_alt / lateral_m.max(0.01)).to_degrees();
    if !(0.0..=50.0).contains(&beta_deg) {
        return if beta_deg < 0.0 { 10.857 } else { 0.0 };
    }

    // Doc 29 Γ × Λ uses two `exp` calls per Wing accept-path pixel; route
    // through the project's `fast_exp_f64` polynomial (Padé 5th-order, used
    // already in road/iso9613 paths) which is ~2-3× faster than libm exp/exp2
    // at the cost of < 0.001 dB error per call. Both args are tightly bounded
    // (|x| ≤ 8) so well inside `EXP_CLAMP_HI`.
    let gamma = if lateral_m <= 914.0 {
        1.089 * (1.0 - fast_exp_f64(-0.00274 * lateral_m))
    } else {
        1.0
    };

    let lambda_beta = 1.137 - 0.0229 * beta_deg + 9.72 * fast_exp_f64(-0.142 * beta_deg);
    gamma * lambda_beta
}

/// Per-installation `(installation, di_a, di_b, di_c)` constants for the
/// inline ΔI formula in `segment_energy_kernel`. Returning the `Installation`
/// echo lets pipeline's SoA build store both the enum and the trig
/// coefficients in one call. Output of the inline formula matches the
/// exact Doc 29 §4.5.3 ΔI (Eq. 4-15) to FP rounding for the same
/// `(rel_alt, slant)` pair, so swapping in this fast path doesn't move
/// popup ↔ tile parity.
#[inline]
pub fn delta_i_constants(installation: Installation) -> (Installation, f64, f64, f64) {
    match installation {
        Installation::Wing => (Installation::Wing, 0.0039, 0.062, 0.8786),
        Installation::Fuselage => (Installation::Fuselage, 0.1225, 0.329, 1.0),
        Installation::Propeller => (Installation::Propeller, 0.0, 0.0, 1.0),
    }
}

/// Output of the shared airborne kernel. SEL + CPA fields cover what
/// every caller needs; callers convert SEL → linear energy via
/// `propagation::iso9613::fast_exp_f64` themselves (no libm exp here).
#[derive(Clone, Copy, Debug)]
pub struct AircraftKernelResult {
    /// SEL in dB after Doc 29 master Eq. 4-8b.
    pub sel: f64,
    /// SEL before receiver-side terrain/building screening. This is retained
    /// for popup aggregation; the heatmap still consumes `sel` only.
    pub free_sel: f64,
    /// Screened SEL with only terrain removed, and with only buildings
    /// removed. These retain Doc 29's lateral attenuation/Lambda treatment;
    /// they are popup-only effect variants, not alternate heatmap paths.
    pub sel_no_terrain: f64,
    pub sel_no_screening: f64,
    /// Terrain and building diffraction values before the mutually-exclusive
    /// winner is applied. The popup uses them for the no-effect variants and
    /// the trace's winning-edge summary.
    pub terrain_dz: f64,
    pub building_dz: f64,
    /// Doc 29 Eq. 4-8b terms, retained for the popup segment trace.
    pub sel_npd_db: f64,
    pub delta_v_db: f64,
    pub delta_i_db: f64,
    pub lambda_db: f64,
    pub delta_f_db: f64,
    pub d_bar_m: f64,
    pub installation: Installation,
    pub cffk_fast_path: bool,
    /// Slant distance from receiver to CPA foot.
    pub d_p_m: f64,
    /// Signed altitude at CPA foot relative to `rcv_elev`.
    pub rel_alt_m: f64,
    /// Signed parametric distance from segment start to CPA foot.
    pub q_m: f64,
    /// Segment length (input echo).
    pub seg_len_m: f64,
    /// Horizontal CPA distance (perpendicular to ground track).
    pub lateral_m: f64,
    /// Elevation angle β from ground plane (degrees). For CFFK fast-path
    /// hits we don't need β to compute λ, so the kernel sets it to a
    /// 90° sentinel rather than paying for `fast_atan` — popup callers
    /// that need β at cruise distance recompute it themselves.
    pub beta_deg: f64,
    /// Unclamped parametric projection (foot in [0,1] = inside segment).
    pub t: f64,
}

/// Shared Doc 29 per-segment kernel: CPA → reach gate → Filter D → NPD →
/// CFFK fast path or full ΔF / Λ / ΔI corrections → SEL → energy. Inputs
/// are pre-projected by the caller (receiver-local meters), so popup pays
/// one cos/sin per receiver and pipeline pays them once at
/// `ProjectedAircraft::build`.
///
/// **Filter D** rejects per-(segment, receiver) pairs where BOTH the CPA
/// foot `t` falls outside the observed endpoints (`t ∉ [0, 1]`, i.e. the
/// implied aircraft position is a straight-line projection past the last
/// trace sample) AND the linearly-extrapolated altitude at the foot is
/// more than 30 m below terrain at `(foot_lat, foot_lon)`. Airport-ground
/// segments bypass the filter (they set `terrain_*_cut_m = f64::MIN`).
///
/// Why the `t ∈ [0, 1]` gate: legitimate cases keep CPA inside the
/// observed segment (Schiphol-style sub-sea descents have CPA within the
/// descent segment; hill-top receivers with aircraft in a valley have CPA
/// within the cruise segment). Only fictional extrapolations past
/// touchdown or beyond the last cruise sample can land outside `[0, 1]`.
///
/// Why the 30 m margin (encoded as `terrain_*_cut_m = terrain - 30 m`
/// pre-computed at z9-square load): DEM error plus short-segment extrapolation
/// uncertainty.
///
/// Historical: an earlier blanket filter at `CPA rel_alt < -50 m` / jet
/// `rel_alt < 30 m AGL` was tried and removed after independent review —
/// it created spatial discontinuities for valid hill-top receivers.
/// Filter D is the geometry-aware replacement.
///
/// **C2 terrain-horizon screening** (`horizon: Some(..)`): per-pair
/// `ReceiverHorizon::screening_dz` insertion loss for real geometry
/// below the receiver's terrain horizon. `None` remains byte-identical to the
/// pre-C2 kernel for regression tests and non-airborne callers; production
/// airborne popup and heatmap paths always pass a horizon.
/// No double-count with Filter D: D drops 100 %-fictional sub-terrain
/// extrapolations (`t ∉ [0, 1]` only) and never attenuates; the horizon
/// attenuates real above-terrain geometry — both can act on one pair.
///
/// **Vector-building screening** uses the closest physical point on the finite
/// subsegment. Terrain and roof diffraction compete by maximum; in the full
/// arm only their excess over the already-applied lateral attenuation is new.
// Flat signature, not a param struct: per `clippy.toml` (threshold 13) this
// acoustic kernel takes its segment-geometry + Doc 29 emission inputs as
// hoisted scalars — bundling them into a struct would add indirection without
// clarity on the per-pixel hot path (`segment_sel_at_pixel_energy`, ~65k
// calls/segment/tile).
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn segment_energy_kernel<const WANT_CPA: bool>(
    ax: f64,
    ay: f64,
    sdx: f64,
    sdy: f64,
    sdz: f64,
    sz1: f64,
    inv_lsq: f64,
    slen: f64,
    rcv_elev: f64,
    npd_luts: &NpdLuts,
    noise_class: usize,
    is_dep: bool,
    seg_dv: f64,
    d_bar_m: f64,
    inst: Installation,
    di_a: f64,
    di_b: f64,
    di_c: f64,
    airport_ground: bool,
    reach_sq: f64,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    horizon: Option<&ReceiverHorizon>,
    buildings: Option<&BuildingHorizon>,
) -> Option<AircraftKernelResult> {
    segment_energy_kernel_inner::<WANT_CPA, false>(
        ax,
        ay,
        sdx,
        sdy,
        sdz,
        sz1,
        inv_lsq,
        slen,
        rcv_elev,
        npd_luts,
        noise_class,
        is_dep,
        seg_dv,
        d_bar_m,
        inst,
        di_a,
        di_b,
        di_c,
        airport_ground,
        reach_sq,
        terrain_start_cut_m,
        terrain_end_cut_m,
        horizon,
        buildings,
    )
}

/// Popup-only twin of [`segment_energy_kernel`]. It returns a screened
/// segment even when the received SEL falls below the 20 dB display floor so
/// the caller can retain the pre-screen energy and report a real impact.
#[allow(clippy::too_many_arguments)]
#[inline]
pub(crate) fn segment_energy_kernel_with_screening<const WANT_CPA: bool>(
    ax: f64,
    ay: f64,
    sdx: f64,
    sdy: f64,
    sdz: f64,
    sz1: f64,
    inv_lsq: f64,
    slen: f64,
    rcv_elev: f64,
    npd_luts: &NpdLuts,
    noise_class: usize,
    is_dep: bool,
    seg_dv: f64,
    d_bar_m: f64,
    inst: Installation,
    di_a: f64,
    di_b: f64,
    di_c: f64,
    airport_ground: bool,
    reach_sq: f64,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    horizon: Option<&ReceiverHorizon>,
    buildings: Option<&BuildingHorizon>,
) -> Option<AircraftKernelResult> {
    segment_energy_kernel_inner::<WANT_CPA, true>(
        ax,
        ay,
        sdx,
        sdy,
        sdz,
        sz1,
        inv_lsq,
        slen,
        rcv_elev,
        npd_luts,
        noise_class,
        is_dep,
        seg_dv,
        d_bar_m,
        inst,
        di_a,
        di_b,
        di_c,
        airport_ground,
        reach_sq,
        terrain_start_cut_m,
        terrain_end_cut_m,
        horizon,
        buildings,
    )
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn segment_energy_kernel_inner<const WANT_CPA: bool, const RETAIN_SCREENED: bool>(
    ax: f64,
    ay: f64,
    sdx: f64,
    sdy: f64,
    sdz: f64,
    sz1: f64,
    inv_lsq: f64,
    slen: f64,
    rcv_elev: f64,
    npd_luts: &NpdLuts,
    noise_class: usize,
    is_dep: bool,
    seg_dv: f64,
    d_bar_m: f64,
    inst: Installation,
    di_a: f64,
    di_b: f64,
    di_c: f64,
    airport_ground: bool,
    reach_sq: f64,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    horizon: Option<&ReceiverHorizon>,
    buildings: Option<&BuildingHorizon>,
) -> Option<AircraftKernelResult> {
    let t = -(ax * sdx + ay * sdy) * inv_lsq;
    let cpx = ax + t * sdx;
    let cpy = ay + t * sdy;
    let lateral_sq = cpx * cpx + cpy * cpy;

    let rel_alt = sz1 + t * sdz - rcv_elev;
    let slant_sq = lateral_sq + rel_alt * rel_alt;
    if slant_sq > reach_sq {
        return None;
    }

    // Filter D: reject sub-terrain extrapolation past observed endpoints.
    if t < 0.0 {
        if sz1 + t * sdz < terrain_start_cut_m {
            return None;
        }
    } else if t > 1.0 && sz1 + t * sdz < terrain_end_cut_m {
        return None;
    }

    let d_p_m = slant_sq.sqrt();
    let d_ft = (d_p_m * FT_PER_M).max(100.0);
    // log2 × log10(2) is identical to log10 to f64 precision and skips
    // the libm log10 division by ln(10) — saves a few cycles per call,
    // pipeline already shipped this way.
    let log_d = d_ft.log2() * LOG10_2;
    let sel_npd = npd_luts.lookup(noise_class, is_dep, log_d);

    // CFFK fast path: above 7.62 km slant Λ and ΔI collapse to within
    // fractions of a dB and the kernel can skip them; ΔF stays.
    //
    // For long aggregated segments **with the unclamped CPA foot well
    // inside the endpoints** (t in (q_m/d_bar, (slen-q_m)/d_bar) both
    // ≫ 1) ΔF ≈ 0 — matches the previous skip within fractions of a dB.
    // For aggregated segments where the foot is near an endpoint or
    // outside (e.g., receiver off the side of a 50 km cruise leg), ΔF
    // correctly suppresses by 3-10 dB; the previous skip was a latent
    // Doc 29 deviation, fixed here.
    //
    // For per-sample airborne (Commit A): one sub-segment owns the foot
    // (ΔF ≈ 0 → full event energy); the other N-1 sub-segments project
    // the foot far outside their endpoints (α₁, α₂ same sign, |α| ≫ 1
    // ⇒ f → 0 ⇒ ΔF strongly negative ⇒ negligible energy). Sum across
    // sub-segments converges to one event, avoiding the N× over-count
    // the previous skip would produce for collinear short segments.
    if d_p_m > AIRCRAFT_FAR_FIELD_THRESHOLD_M {
        // ΔF ≤ 0 always (it is 10·log10 of the energy fraction f ∈ (0, 1]:
        // g(α) = α/(1+α²) + atan(α) is monotone ⇒ g2 − g1 ∈ (0, π) ⇒ f ≤ 1).
        // So the fast-path sel ≤ sel_npd + seg_dv; if that ceiling is already
        // below the 20 dB floor the segment can never clear it — skip ΔF.
        if sel_npd + seg_dv < 20.0 {
            return None;
        }
        let q_m = t * slen;
        let df = fast_delta_f(q_m, slen, d_bar_m);
        let free_sel = sel_npd + seg_dv + df;
        let mut sel = free_sel;
        if sel < 20.0 {
            return None;
        }
        // C2 terrain-horizon screening, CFFK arm. Precheck (delta 3):
        // `rel_alt <= 0` routes receiver-above-aircraft geometry to the
        // full check (squaring alone would silently pass it); otherwise
        // sin²β ≥ max stored sin²h ⇒ above every horizon ⇒ skip. The
        // `lateral` sqrt is paid only after the precheck passes (delta 1
        // perf note). This branch subtracts Dz alone — CFFK never
        // computes Λ (premise: Λ negligible above 7.62 km slant), so
        // there is no Λ to credit back. CFFK's premise is that the omitted
        // installation/lateral terms are negligible at this slant.
        let terrain_dz = if let Some(hz) = horizon {
            if rel_alt <= 0.0 || rel_alt * rel_alt < slant_sq * hz.max_sin_sq {
                let lateral = lateral_sq.sqrt();
                hz.screening_dz(cpx, cpy, lateral, rel_alt)
            } else {
                0.0
            }
        } else {
            0.0
        };
        let building_dz = if let Some(buildings) = buildings {
            let physical_t = t.clamp(0.0, 1.0);
            buildings.screening_dz(
                ax + physical_t * sdx,
                ay + physical_t * sdy,
                sz1 + physical_t * sdz - rcv_elev,
            )
        } else {
            0.0
        };
        let diffraction_db = terrain_dz.max(building_dz);
        sel -= diffraction_db;
        // Floor check runs AFTER screening: a screened segment that
        // drops below 20 dB is inaudible and must be culled exactly like
        // an unscreened one (the pre-ΔF ceiling check above is an upper
        // bound that screening only tightens, so it stays valid).
        if !RETAIN_SCREENED && sel < 20.0 {
            return None;
        }
        return Some(AircraftKernelResult {
            sel,
            free_sel,
            sel_no_terrain: free_sel - building_dz,
            sel_no_screening: free_sel - terrain_dz,
            terrain_dz,
            building_dz,
            sel_npd_db: sel_npd,
            delta_v_db: seg_dv,
            delta_i_db: 0.0,
            lambda_db: 0.0,
            delta_f_db: df,
            d_bar_m,
            installation: inst,
            cffk_fast_path: true,
            d_p_m,
            rel_alt_m: rel_alt,
            q_m,
            seg_len_m: slen,
            // CPA-only (the fast-path sel ignores lateral_m); skipped for the
            // heatmap energy-only path (`WANT_CPA = false`).
            lateral_m: if WANT_CPA { lateral_sq.sqrt() } else { 0.0 },
            beta_deg: 90.0, // CFFK doesn't need β; sentinel
            t,
        });
    }

    let q_m = t * slen;
    let df = fast_delta_f(q_m, slen, d_bar_m);

    let lateral_m = lateral_sq.sqrt();
    let lambda = fast_lateral_attenuation(rel_alt, lateral_m, airport_ground, inst);

    // Inline ΔI: works off u² = rel_alt²/slant² (= sin²β) instead of trig
    // on β. Identical math to the exact Doc 29 §4.5.3 ΔI (Eq. 4-15) for the
    // same (rel_alt, slant) — verified to FP rounding.
    let di = if matches!(inst, Installation::Propeller) {
        0.0
    } else {
        let rel_alt_di = rel_alt.max(0.0);
        let u2 = (rel_alt_di * rel_alt_di) / slant_sq.max(1e-12);
        let v2 = 1.0 - u2;
        let x = di_a * v2 + u2;
        let den = di_c * (4.0 * u2 * v2) + (v2 - u2) * (v2 - u2);
        if den > 0.0 && x > 0.0 {
            // ln × (10/ln 10) ≡ log2 × (10·log10 2) at f64 precision; glibc
            // log2 is faster than log (~2× shared polynomial helper hits).
            (10.0 * LOG10_2) * (di_b * x.log2() - den.log2())
        } else {
            0.0
        }
    };

    let free_sel = sel_npd + seg_dv + di - lambda + df;
    let mut sel = free_sel;
    if sel < 20.0 {
        return None;
    }
    // C2 terrain-horizon screening, full arm (same delta-3 precheck as
    // the CFFK arm above). AEDT LOS-blockage bookkeeping: barrier loss
    // and lateral attenuation are mutually exclusive, never summed —
    // SEL −= max(Dz, Λ) − Λ ≡ (Dz − Λ).max(0) with the kernel's Λ
    // already inside `sel` (AEDT 3f TM "Line-of-Sight Blockage";
    // rationale Berton, AIAA 2021).
    let terrain_dz = if let Some(hz) = horizon {
        if rel_alt <= 0.0 || rel_alt * rel_alt < slant_sq * hz.max_sin_sq {
            hz.screening_dz(cpx, cpy, lateral_m, rel_alt)
        } else {
            0.0
        }
    } else {
        0.0
    };
    let building_dz = if let Some(buildings) = buildings {
        let physical_t = t.clamp(0.0, 1.0);
        buildings.screening_dz(
            ax + physical_t * sdx,
            ay + physical_t * sdy,
            sz1 + physical_t * sdz - rcv_elev,
        )
    } else {
        0.0
    };
    let diffraction_db = terrain_dz.max(building_dz);
    sel -= (diffraction_db - lambda).max(0.0);
    // Post-screening floor, same rationale as the CFFK arm.
    if !RETAIN_SCREENED && sel < 20.0 {
        return None;
    }
    // CPA-only — `beta_deg` never feeds `sel` (ΔI uses u² above). Skipped on the
    // heatmap energy-only path (`WANT_CPA = false`); saves one atan per pixel.
    let beta_deg = if WANT_CPA {
        fast_atan(rel_alt / lateral_m.max(0.01)).to_degrees()
    } else {
        0.0
    };

    Some(AircraftKernelResult {
        sel,
        free_sel,
        sel_no_terrain: free_sel - (building_dz - lambda).max(0.0),
        sel_no_screening: free_sel - (terrain_dz - lambda).max(0.0),
        terrain_dz,
        building_dz,
        sel_npd_db: sel_npd,
        delta_v_db: seg_dv,
        delta_i_db: di,
        lambda_db: lambda,
        delta_f_db: df,
        d_bar_m,
        installation: inst,
        cffk_fast_path: false,
        d_p_m,
        rel_alt_m: rel_alt,
        q_m,
        seg_len_m: slen,
        lateral_m,
        beta_deg,
        t,
    })
}

/// Period durations in seconds (EU Directive 2002/49/EC).
pub const PERIOD_SECONDS: [f64; 3] = [43200.0, 14400.0, 28800.0];

/// Convert period energy sum to Leq (Doc 29 §5, Eq. 5-1).
/// Divides by n_days × period_seconds.
#[inline]
pub fn period_leq(total_energy: f64, n_days: f64, period_seconds: f64) -> f64 {
    if total_energy <= 0.0 || n_days <= 0.0 {
        return f64::NEG_INFINITY;
    }
    10.0 * (total_energy / (n_days * period_seconds)).log10()
}

#[cfg(test)]
mod tests {
    use super::super::npd::PROFILES;
    use super::*;

    #[test]
    fn test_cpa_alongside() {
        let cpa = compute_cpa(
            50.005, 14.01, 300.0, 50.0, 14.0, 1000.0, 50.01, 14.0, 1000.0,
        );
        assert!(cpa.q_m > 0.0, "q should be positive");
        assert!(
            cpa.d_p_m > 500.0 && cpa.d_p_m < 2000.0,
            "d_p = {}",
            cpa.d_p_m
        );
        assert!(
            cpa.beta_deg > 20.0 && cpa.beta_deg < 70.0,
            "β = {}",
            cpa.beta_deg
        );
    }

    #[test]
    fn test_cpa_behind_segment() {
        let cpa = compute_cpa(49.99, 14.01, 300.0, 50.0, 14.0, 1000.0, 50.01, 14.0, 1000.0);
        assert!(
            cpa.q_m < 0.0,
            "q should be negative (behind), got {}",
            cpa.q_m
        );
    }

    #[test]
    fn test_cpa_directly_below() {
        let cpa = compute_cpa(50.005, 14.0, 300.0, 50.0, 14.0, 3000.0, 50.01, 14.0, 3000.0);
        assert!(
            cpa.lateral_m < 50.0,
            "lateral should be ~0, got {}",
            cpa.lateral_m
        );
        assert!(
            cpa.beta_deg > 80.0,
            "β should be ~90°, got {}",
            cpa.beta_deg
        );
    }

    #[test]
    fn test_cpa_below_receiver_keeps_signed_altitude() {
        let cpa = compute_cpa(
            49.7846, 14.0306, 684.0, 49.7813, 14.0350, 0.0, 49.7863, 14.0283, 0.0,
        );
        assert!(
            cpa.lateral_m < 50.0,
            "lateral should stay near the ridge crossing, got {}",
            cpa.lateral_m
        );
        assert!(
            cpa.relative_alt_m < -600.0,
            "relative altitude should stay signed, got {}",
            cpa.relative_alt_m
        );
        assert!(
            cpa.d_p_m > 600.0,
            "slant distance should include the vertical gap, got {}",
            cpa.d_p_m
        );
        assert!(
            cpa.beta_deg < 0.0,
            "beta should be negative for segments below the receiver, got {}",
            cpa.beta_deg
        );
    }

    /// Curving-departure phantom: a short, high segment ~7 km from the receiver
    /// whose straight line extended backward puts the unclamped infinite-line
    /// foot ~40 m away (real flight 39d311 over LKPR). `compute_cpa` keeps that
    /// phantom (the SEL ΔF integral needs it); `clamped_display_cpa` must
    /// recover the real multi-km closest approach for the popup display.
    #[test]
    fn clamped_display_cpa_kills_curving_departure_phantom() {
        let cpa = compute_cpa(
            50.1110, 14.3819, 279.0, // receiver (LKPR R4 reference)
            50.1700, 14.4197, 1509.0, // segment start (39d311 climb leg)
            50.1711, 14.4204, 1532.0, // segment end
        );
        assert!(
            cpa.d_p_m < 100.0,
            "unclamped d_p is the phantom, got {}",
            cpa.d_p_m
        );
        assert!(
            cpa.t < 0.0,
            "foot lies before the segment, got t = {}",
            cpa.t
        );

        let (dist, alt) = clamped_display_cpa(&cpa, 1532.0 - 1509.0);
        assert!(
            dist > 5000.0,
            "clamped distance should be the real far approach, got {dist}"
        );
        assert!(
            alt > 1000.0,
            "clamped altitude should be the real climb height, got {alt}"
        );
    }

    /// Foot inside the segment → clamp is a bit-for-bit no-op (returns the
    /// kernel's own `d_p_m` / `relative_alt_m`, so inside-segment popup rows are
    /// identical to today and can't wobble a `round1` band boundary).
    #[test]
    fn clamped_display_cpa_noop_when_foot_inside() {
        let cpa = compute_cpa(
            50.005, 14.01, 300.0, 50.0, 14.0, 1000.0, 50.01, 14.0, 1000.0,
        );
        assert!(
            (0.0..=1.0).contains(&cpa.t),
            "foot should be inside, got t = {}",
            cpa.t
        );
        let (dist, alt) = clamped_display_cpa(&cpa, 0.0);
        assert_eq!(dist, cpa.d_p_m);
        assert_eq!(alt, cpa.relative_alt_m);
    }

    /// Foot PAST the segment end (t > 1): the receiver sits beyond where a
    /// climbing segment ends, so the clamp must wind back to the END endpoint —
    /// larger distance, altitude at the segment end. Mirrors the t < 0 case.
    #[test]
    fn clamped_display_cpa_winds_back_to_end_when_foot_past_segment() {
        // N–S climbing segment at lon 14.0; receiver due north of the end.
        let cpa = compute_cpa(50.02, 14.0, 300.0, 50.0, 14.0, 1000.0, 50.01, 14.0, 1100.0);
        assert!(
            cpa.t > 1.0,
            "foot should lie past the segment end, got t = {}",
            cpa.t
        );
        let (dist, alt) = clamped_display_cpa(&cpa, 1100.0 - 1000.0);
        // Altitude winds back to the end endpoint (1100 − 300 = 800 m).
        assert!(
            (alt - 800.0).abs() < 1.0,
            "clamped altitude should be the end height, got {alt}"
        );
        assert!(
            dist > cpa.d_p_m,
            "clamped distance should exceed the unclamped foot, got {dist} vs {}",
            cpa.d_p_m
        );
    }

    #[test]
    fn test_delta_v_at_reference() {
        assert!(delta_v(160.0, &PROFILES[0]).abs() < 0.001);
    }

    #[test]
    fn test_delta_v_slow() {
        let dv = delta_v(80.0, &PROFILES[0]);
        assert!((dv - 3.01).abs() < 0.1, "ΔV = {dv}");
    }

    #[test]
    fn test_fast_delta_f_alongside() {
        let df = fast_delta_f(500.0, 1000.0, 370.0);
        assert!(df < 0.0 && df > -10.0, "ΔF = {df}");
    }

    #[test]
    fn test_fast_delta_f_behind() {
        let df = fast_delta_f(-500.0, 1000.0, 370.0);
        assert!(df < -5.0, "ΔF behind = {df}");
    }

    #[test]
    fn test_fast_lateral_directly_below() {
        // Receiver under the path (lateral≈0 ⇒ β≈90°, outside the 0–50° band) ⇒ Λ = 0.
        let att = fast_lateral_attenuation(1000.0, 0.0, false, Installation::Wing);
        assert!(att.abs() < 0.01, "Expected 0, got {att}");
    }

    #[test]
    fn test_fast_lateral_far_side() {
        // Shallow grazing angle (small β) ⇒ Λ near its ~10.86 dB peak.
        let att = fast_lateral_attenuation(1.0, 2000.0, false, Installation::Wing);
        assert!(
            att > 10.0 && att < 10.9,
            "low-β Wing Λ near peak, got {att}"
        );
    }

    #[test]
    fn test_fast_lateral_negative_beta() {
        // Below the wing-shielding geometry (β < 0) ⇒ fixed 10.857 dB.
        let att = fast_lateral_attenuation(-5.0, 100.0, false, Installation::Wing);
        assert!((att - 10.857).abs() < 0.01);
    }

    #[test]
    fn test_fast_lateral_non_wing_installations_zero() {
        for &(rel_alt, lat) in &[
            (50.0, 500.0),
            (200.0, 500.0),
            (1000.0, 500.0),
            (5000.0, 100.0),
        ] {
            assert_eq!(
                fast_lateral_attenuation(rel_alt, lat, false, Installation::Fuselage),
                0.0
            );
            assert_eq!(
                fast_lateral_attenuation(rel_alt, lat, false, Installation::Propeller),
                0.0
            );
        }
    }

    #[test]
    fn test_delta_i_constants() {
        // Propeller gets no ΔI directivity (a=b=0); Wing carries the Doc 29
        // §4.5.3 coefficients the inline kernel ΔI reads.
        let (_, a_p, b_p, _) = delta_i_constants(Installation::Propeller);
        assert_eq!((a_p, b_p), (0.0, 0.0));
        let (_, a_w, b_w, c_w) = delta_i_constants(Installation::Wing);
        assert_eq!((a_w, b_w, c_w), (0.0039, 0.062, 0.8786));
    }

    #[test]
    fn test_period_leq() {
        let energy = 1000.0 * 10f64.powf(91.0 / 10.0);
        let leq = period_leq(energy, 365.0, PERIOD_SECONDS[0]);
        assert!(leq > 40.0 && leq < 80.0, "Leq = {leq}");
    }

    /// Doc 29 §A.3.4 invariant: emitting N collinear sub-segments must
    /// give the same total linear energy as one aggregated segment over
    /// the same geometry. CFFK previously skipped ΔF in the far field
    /// (slant > 7.62 km), which let each sub-segment re-emit the full
    /// event and over-counted by factor N. With ΔF restored, the
    /// off-foot sub-segments collapse to ~0 and the foot-owning piece
    /// contributes the full event.
    #[test]
    fn cffk_partition_preserves_linear_energy() {
        use super::super::npd::{CLASS_REP_PROFILE_IDX, REACH_SQ_TABLE};
        let npd_luts = NpdLuts::shared();
        let class_idx = 0; // WING_FALLBACK
        let profile = &PROFILES[CLASS_REP_PROFILE_IDX[class_idx] as usize];
        let (inst_code, di_a, di_b, di_c) = delta_i_constants(profile.installation);
        let reach_sq = REACH_SQ_TABLE[class_idx][1]; // departure
                                                     // Geometry: 20 km-long flight at 8 km altitude, receiver 8 km
                                                     // off to the side at sea level → slant ~11 km (far field).
        let start_x = -10_000.0;
        let start_y = 8_000.0;
        let end_x = 10_000.0;
        let end_y = 8_000.0;
        let alt_m = 8_000.0;
        let dv = delta_v(profile.v_ref_kt, profile);
        let kernel = |ax: f64, ay: f64, sdx: f64, sdy: f64, slen: f64| {
            let inv_lsq = 1.0 / (sdx * sdx + sdy * sdy);
            segment_energy_kernel::<true>(
                ax,
                ay,
                sdx,
                sdy,
                0.0,
                alt_m,
                inv_lsq,
                slen,
                0.0,
                npd_luts,
                class_idx,
                true,
                dv,
                profile.d_bar_m,
                inst_code,
                di_a,
                di_b,
                di_c,
                false,
                reach_sq,
                f64::MIN,
                f64::MIN,
                None,
                None,
            )
        };
        // One aggregated segment.
        let agg = kernel(start_x, start_y, end_x - start_x, end_y - start_y, 20_000.0).unwrap();
        // Same geometry split into 10 collinear sub-segments.
        let n = 10;
        let mut sum_energy = 0.0;
        for i in 0..n {
            let t0 = i as f64 / n as f64;
            let t1 = (i + 1) as f64 / n as f64;
            let sx = start_x + (end_x - start_x) * t0;
            let ex = start_x + (end_x - start_x) * t1;
            let slen = (end_x - start_x) / n as f64;
            if let Some(r) = kernel(sx, start_y, ex - sx, 0.0, slen) {
                sum_energy += (r.sel * std::f64::consts::LN_10 * 0.1).exp();
            }
        }
        let agg_energy = (agg.sel * std::f64::consts::LN_10 * 0.1).exp();
        let ratio = sum_energy / agg_energy;
        assert!(
            (0.7..1.3).contains(&ratio),
            "partition / aggregate energy ratio = {ratio:.3} (expected ~1.0)"
        );
    }
}
