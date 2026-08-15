//! ISO 9613-2 single-edge diffraction (§7.4, max 20 dB) + CNOSSOS-EU (2.5.21)
//! Maekawa per-band attenuation.
//!
//! The dominant edge is selected upstream by [`super::horizon`] (largest
//! path-length difference δ); this module computes that edge's δ geometry
//! ([`compute_single_edge`] for a cadence sample, [`compute_single_edge_at`]
//! for an explicit vector-obstacle crossing between samples), the bare-earth
//! δ\* OLS mean-ground fit ([`compute_delta_star`]), the Rayleigh admission
//! ([`rayleigh_admits`]) and the banded attenuation
//! ([`diffraction_attenuation_mixed`]).
//!
//! # The Rayleigh δ\* gate applies to UNBLOCKED rays only (fixed 2026-08-05)
//!
//! Until this date [`maekawa_bands`] zeroed every band with `δ ≤ λ/4 − δ*`,
//! **for either sign of δ**. On the blocked side that is a STEP FUNCTION on a
//! quantity the attenuation formula is already smooth in: at the cut the
//! formula reads `10·lg(3 + 20·(λ/4 − δ*)/λ) = 10·lg(8 − 20δ*/λ)`, i.e.
//! **9.03 dB at δ\* = 0 and 7.40 dB on flat ground** (where `δ* = |δ|`
//! exactly), thrown away in one step. On a 200 m path with a 0.05 m source and
//! a 4 m receiver, a wall growing from 4.1229 m to 4.1249 m took the 1 kHz band
//! from 0 to 7.48 dB — two millimetres of wall height, fifteen palette steps of
//! map. Obstacle heights are quantised, so the step printed building outlines
//! into the map.
//!
//! What the standards say:
//!
//! * **ISO 9613-2** (§7.4, `Dz = 10·lg(3 + (C2/λ)·C3·z·Kmet)`) has no λ/4
//!   condition of any kind — its admission tests are about the OBJECT (closed
//!   surface, surface density ≥ 10 kg/m², horizontal dimension normal to the
//!   path > λ), never about δ.
//! * **CNOSSOS-EU 2015/996** (2.5.21) admits a band exactly while
//!   `(40/λ)·C″·δ ≥ −2` — that is `δ ≥ −λ/20`, and it is continuous BY
//!   CONSTRUCTION: at the bound `10·lg(3 − 2) = 0 dB`. The base text names
//!   "Rayleigh's criterion" once, non-normatively and with no formula
//!   ("*If an obstacle does not produce diffraction, this for instance being
//!   determined according to Rayleigh's criterion, there is no need to
//!   calculate Adif*"). The string `λ/4` does not occur in the 2015 directive.
//! * **Commission Delegated Directive (EU) 2021/1226**, point (9)(c) — which
//!   replaces that sub-paragraph — is where `λ/4 − δ*` comes from, and it is
//!   scoped by its own first words: "***If the direct ray is not blocked***,
//!   the edge D is sought which produces the largest path length difference δ
//!   (the smallest absolute value **because these path length differences are
//!   negative**). Diffraction is taken into account if: — this path length
//!   difference is larger than −λ/20, and — if the "Rayleigh-criterion" is
//!   fulfilled. This is the case, if δ is larger than λ/4 − δ*…".
//!
//! * **ISO/TR 17534-4:2020 §5.9 "Rayleigh's Criterion"** is the agreed
//!   interpretation of exactly this passage, and it says it in one sentence:
//!   "*The decision whether diffraction must be calculated is made separately
//!   for homogeneous and favourable conditions respectively.* **If the line of
//!   sight is blocked, diffraction is always calculated.** *If the line of
//!   sight from source to receiver is unobstructed, Rayleigh's Criterion is
//!   employed as follows: … Diffraction is calculated only if `δD > -λ/20` and
//!   `δD > λ/4 - δD*`*".
//!
//! The reference implementation is the same predicate verbatim —
//! `AttenuationCnossos.isValidRcrit`, NoiseModelling `35d2da1b`:
//!
//! ```java
//! // Eq 2.5.21: if delta >= 0, diffraction always applies; Rayleigh criterion only for delta < 0
//! return pp.delta >= 0 || (pp.delta > -lambda / 20 && pp.delta > lambda / 4 - pp.deltaPrime);
//! ```
//!
//! and its path-build stage reaches `computeRayleighDiff` only from the
//! `p0 == null` branch — "*Direct propagation (no diffraction over obstructing
//! objects)*". A blocked ray never meets a λ/4 test in any of these documents.
//! NMPB-2008, which CNOSSOS derives from, contains neither "Rayleigh" nor
//! "λ/4" at all: the criterion enters with the 2021 amendment.
//!
//! **So the gate stays, scoped to δ < 0.** It is not decoration there: with it
//! removed, a 0.5 m wall sitting 1.5 m BELOW the sight line 100 m away screens
//! 3.8 dB at 63 Hz, because on a 200 m path no obstacle can reach even
//! `δ = −λ₆₃/20 = −0.27 m` — the whole penumbra window is open and every kerb
//! becomes a bass screen. `δ + δ*` is the prominence the criterion measures,
//! and on flat ground it is exactly 0 for an edge at ground level (mirroring
//! both endpoints about the plane fixes the edge, so `δ* = |δ|`); requiring it
//! to exceed λ/4 is precisely "the obstacle must be resolved by the wave".
//!
//! What is left at δ = 0, and why it is the standard's and not ours: a band
//! whose `δ* ≤ λ/4` is rejected at 0⁻ and takes the blocked branch's
//! `10·lg 3 = 4.77 dB` at 0⁺. That ceiling holds after the (2.5.9) mix too —
//! each arm steps from 0 to at most `10·lg 3`, and the energetic mean is
//! 1-Lipschitz in each dB input with weights summing to 1 — and 4.32 dB is the
//! worst measured over the sweep geometries (63 Hz). CNOSSOS absorbs that step
//! in the model switch its "otherwise" prescribes — "*a common mean ground
//! plane for the path S → R is calculated, and Aground is calculated with no
//! diffraction*" versus two split planes with
//! `Adif = Δdif(S,R) + Δground(S,O) + Δground(O,R)` (2.5.30) — a switch this
//! engine does not implement (SPEC §3.3 keeps
//! `max(A_ground, A_terrain + A_screen)`). It is bounded, it sits ONLY on the
//! sight line — a shadow boundary, not an obstacle-height contour — and
//! closing it means implementing the Δground split, not another gate. Pinned
//! by `the_sight_line_step_is_the_standards_own_and_bounded`; SPEC §3.5 carries
//! it as the open item.

use crate::constants::*;
use crate::types::NUM_BANDS;

/// Single-edge diffraction geometry + the CNOSSOS Rayleigh δ\*.
pub struct DiffractionResult {
    pub delta: f64, // path difference in meters
    /// CNOSSOS-EU (2.5.25) favourable-conditions path difference over the SAME
    /// edge: ray curved toward the ground with Γ = max(1000, 8·d_SR), which
    /// arches ABOVE the straight chord and so shortens the detour over the top —
    /// δ_F < δ (and can go negative, i.e. the curved ray clears the edge
    /// entirely). (2.5.26) when the straight SR is broken,
    /// (2.5.27) when it is not. Consumed only when
    /// [`FAVOURABLE_MIXING`] is on; the edge itself stays max-δ-selected on
    /// straight geometry (plan-accepted second-order simplification).
    pub delta_fav: f64,
    /// Rayleigh δ\* of 2021/1226 point (9)(c): path difference over the
    /// dominant edge with mirror source/receiver reflected across the per-side
    /// mean ground planes. 0.0 when there is no obstruction. Kept
    /// straight-geometry under the favourable state too (review-pinned
    /// conservative choice).
    ///
    /// Feeds the `δ ≤ λ/4 − δ*` criterion on the NEGATIVE arm of
    /// [`maekawa_bands`] only (2021/1226 point (9)(c) scopes it to an
    /// unblocked direct ray) and the popup trace's `delta_star_m`. It carried
    /// the blocked arm too until 2026-08-05 — module header.
    pub delta_star: f64,
    /// Number of diffraction edges found (0 = clear path, 1 = dominant edge).
    pub n_edges: u8,
    /// Profile sample index of the dominant edge (meaningful when `n_edges == 1`).
    pub edge_idx: usize,
}

#[inline]
fn empty_result() -> DiffractionResult {
    DiffractionResult {
        delta: 0.0,
        delta_fav: 0.0,
        delta_star: 0.0,
        n_edges: 0,
        edge_idx: 0,
    }
}

/// CNOSSOS-EU (2.5.24)/(2.5.25)/(2.5.26) favourable-conditions path difference
/// for an edge that DOES break the direct ray: each straight chord is replaced
/// by the arc of a circle with radius Γ = max(1000, 8·dsr) through its
/// endpoints (arc = 2Γ·asin(ℓ/2Γ)), so `δ_F = ŜO + ÔR − ŜR`.
/// Γ ≥ 8·dsr keeps every asin argument ≤ ~1/16, far from the domain edge.
pub(crate) fn curved_path_difference(d_sb: f64, d_br: f64, dsr: f64, gamma: f64) -> f64 {
    let arc = |chord: f64| 2.0 * gamma * (chord / (2.0 * gamma)).asin();
    arc(d_sb) + arc(d_br) - arc(dsr)
}

/// CNOSSOS-EU (2.5.27) — the OTHER branch of the same construction, for an edge
/// O that does NOT break the direct ray SR:
///
/// ```text
/// δ_F = 2·ŜA + 2·ÂR − ŜO − ÔR − ŜR
/// ```
///
/// where A is where the STRAIGHT SR crosses the vertical through O, and every
/// term is again an arc (2.5.25). The branch is chosen on the straight ray, not
/// the curved one — that is the standard's own rule and what
/// [`compute_single_edge_at`]'s `sign` already encodes.
///
/// It is negative and it MEETS (2.5.26) at the crossing: when O sits on the
/// sight line, A = O, so `ŜA = ŜO` and `ÂR = ÔR` and the expression collapses to
/// `ŜO + ÔR − ŜR`. The naive `−(d_SO + d_OR − d_SR)` does not — it is the
/// STRAIGHT-ray form of this same formula (A lies on SR, so `d_SA + d_AR = d_SR`
/// exactly), and mixing a straight negative arm with a curved positive one put a
/// step of ~0.1 m of δ, ~3 dB at 8 kHz, right at the sight line.
pub(crate) fn curved_path_difference_near_miss(
    d_sa: f64,
    d_ar: f64,
    d_sb: f64,
    d_br: f64,
    dsr: f64,
    gamma: f64,
) -> f64 {
    let arc = |chord: f64| 2.0 * gamma * (chord / (2.0 * gamma)).asin();
    2.0 * arc(d_sa) + 2.0 * arc(d_ar) - arc(d_sb) - arc(d_br) - arc(dsr)
}

/// CNOSSOS-EU (2.5.9) long-term energetic mix of the favourable and
/// homogeneous states, expressed on ATTENUATIONS: both states share every
/// other term of the level chain (emission, divergence, atmosphere, ground,
/// vegetation), so mixing the diffraction attenuation is algebraically
/// identical to mixing the received levels — and with the single flat
/// [`P_FAV`] it is also identical to mixing per period or mixing Lden
/// (verified in the plan review). Mixing leans to the LOUDER (favourable)
/// state, which is the standard's point.
pub(crate) fn mix_fav_hom(
    hom: &[f64; NUM_BANDS],
    fav: &[f64; NUM_BANDS],
    p_fav: f64,
) -> [f64; NUM_BANDS] {
    let mut mixed = [0.0_f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        let e =
            p_fav * 10.0_f64.powf(-fav[i] / 10.0) + (1.0 - p_fav) * 10.0_f64.powf(-hom[i] / 10.0);
        mixed[i] = -10.0 * e.log10();
    }
    mixed
}

/// δ + Rayleigh δ\* over a single pre-selected edge `idx`. `edge_profile` is the
/// composite (or bare) top the δ geometry runs on; `ols_profile` MUST be
/// bare-earth elevation for the §2.5.6(c) mean-ground fit.
pub(super) fn compute_single_edge(
    t: &[f64],
    edge_profile: &[f64],
    ols_profile: &[f64],
    total_dist: f64,
    idx: usize,
    src_elev: f64,
    rcv_elev: f64,
    dsr: f64,
    source_height: f64,
    receiver_height: f64,
) -> DiffractionResult {
    let los = src_elev + (rcv_elev - src_elev) * t[idx];
    if edge_profile[idx] <= los {
        return empty_result();
    }
    let d_sg = t[idx] * total_dist;
    let d_rg = (1.0 - t[idx]) * total_dist;
    let top = edge_profile[idx];
    let d_sb = (d_sg * d_sg + (top - src_elev).powi(2)).sqrt();
    let d_br = (d_rg * d_rg + (top - rcv_elev).powi(2)).sqrt();
    let delta_star = compute_delta_star(
        t,
        ols_profile,
        idx,
        total_dist,
        source_height,
        receiver_height,
    );
    let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
    DiffractionResult {
        delta: d_sb + d_br - dsr,
        delta_fav: curved_path_difference(d_sb, d_br, dsr, gamma),
        delta_star,
        n_edges: 1,
        edge_idx: idx,
    }
}

/// δ + Rayleigh δ\* over an EXPLICIT edge point `(t_e, top_e)` that need not
/// coincide with any profile sample — the vector-obstacle candidate path
/// (geodata-v2 1.3). Same geometry as [`compute_single_edge`]. Per SPEC §3.5b
/// (and the sample path at [`compute_delta_star`]), the diffraction point D —
/// the bare ground LERPed at `t_e` — belongs to BOTH §2.5.6(c) mean-ground
/// fits: source side = samples with `t < t_e` plus D, receiver side = D plus
/// samples with `t > t_e`. At an exact sample t these are the sample path's
/// point sets verbatim (bit-parity on any terrain), and between samples the
/// fits vary continuously as `t_e` crosses a cadence sample.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_single_edge_at(
    t: &[f64],
    ols_profile: &[f64],
    t_e: f64,
    top_e: f64,
    total_dist: f64,
    src_elev: f64,
    rcv_elev: f64,
    dsr: f64,
    source_height: f64,
    receiver_height: f64,
) -> DiffractionResult {
    // A candidate BELOW the sight line keeps its geometry and takes the
    // NEGATIVE path difference — the CNOSSOS penumbra branch of
    // [`maekawa_bands`] decides whether such a near miss still attenuates.
    // (Sample-path edges do not: every bare-ground sample sits below the LOS,
    // so a negative branch there would turn flat terrain into a diffractor and
    // double-count the ground term. Vector candidates are real obstacles with
    // a height, which is exactly the geometry §2.5.6(c) describes.)
    let los = src_elev + (rcv_elev - src_elev) * t_e;
    let sign = if top_e >= los { 1.0 } else { -1.0 };
    let d_sg = t_e * total_dist;
    let d_rg = (1.0 - t_e) * total_dist;
    let d_sb = (d_sg * d_sg + (top_e - src_elev).powi(2)).sqrt();
    let d_br = (d_rg * d_rg + (top_e - rcv_elev).powi(2)).sqrt();

    let n = ols_profile.len();
    // Strict partitions: src samples t < t_e, rcv samples t > t_e; D joins
    // both fits exactly once. Candidates carry t ∈ (0, 1) so each side keeps
    // at least its endpoint sample.
    let p_lo = t.partition_point(|&x| x < t_e);
    let p_hi = t.partition_point(|&x| x <= t_e);
    // Bare ground under the candidate edge, LERPed between its neighbours
    // (equals the sample's elevation when t_e sits exactly on a sample).
    let i1 = p_hi.clamp(1, n - 1);
    let (t0, t1) = (t[i1 - 1], t[i1]);
    let frac = if t1 > t0 {
        ((t_e - t0) / (t1 - t0)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d_top = ols_profile[i1 - 1] + frac * (ols_profile[i1] - ols_profile[i1 - 1]);

    let (_, b_src) = fit_plane_with_point(
        &t[..p_lo],
        &ols_profile[..p_lo],
        t_e,
        d_top,
        0.0,
        total_dist,
    );
    let (a_rcv, b_rcv) = fit_plane_with_point(
        &t[p_hi..],
        &ols_profile[p_hi..],
        t_e,
        d_top,
        t_e,
        total_dist,
    );
    let plane_rcv_at_end = a_rcv * d_rg + b_rcv;
    let s_star_z = 2.0 * b_src - (ols_profile[0] + source_height);
    let r_star_z = 2.0 * plane_rcv_at_end - (ols_profile[n - 1] + receiver_height);
    let d_sd = (d_sg * d_sg + (d_top - s_star_z).powi(2)).sqrt();
    let d_dr = (d_rg * d_rg + (r_star_z - d_top).powi(2)).sqrt();
    let d_sr = (total_dist * total_dist + (r_star_z - s_star_z).powi(2)).sqrt();
    let delta_star = (d_sd + d_dr - d_sr).max(0.0);

    let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
    let straight = d_sb + d_br - dsr;
    // WHICH WAY THE FAVOURABLE RAY BENDS. Downward refraction makes the
    // (2.5.24) ray a circular arc CONCAVE TOWARD THE GROUND, so between its
    // endpoints it stands `≈ d²/8Γ` ABOVE the straight chord — ~3 m over a
    // 200 m path at Γ = 1600 m. It does not sag below the chord; the arc
    // lengths `2Γ·asin(ℓ/2Γ) > ℓ` are the same statement (Fig. 2.5.e; amendment
    // 2021/1226 point (g); reference `CurvedProfileGenerator.applyTransformation`
    // and `testTC28DirectCurvedProfile`).
    //
    // So the favourable state screens a low obstacle LESS, never more: a screen
    // that already passes under the straight sight line passes even further
    // under the ray that arches over it, and δ_F is MORE negative than the
    // straight δ. There is no "curved δ turns knee-high fences into barriers"
    // regime to protect against — that would be the sign the old comment
    // assumed. The one thing the curve must not do is JUMP at the sight line,
    // and (2.5.27) is the formula that makes it not.
    //
    // Branch on the STRAIGHT SR (`sign`), per the standard: (2.5.26) when the
    // direct ray is broken, (2.5.27) when it is not. A is the crossing of the
    // straight SR with the vertical through the edge, hence at the edge's own
    // station on the sight line.
    let delta_fav = if sign > 0.0 {
        curved_path_difference(d_sb, d_br, dsr, gamma)
    } else {
        let d_sa = (d_sg * d_sg + (los - src_elev).powi(2)).sqrt();
        let d_ar = (d_rg * d_rg + (rcv_elev - los).powi(2)).sqrt();
        curved_path_difference_near_miss(d_sa, d_ar, d_sb, d_br, dsr, gamma)
    };
    DiffractionResult {
        delta: sign * straight,
        delta_fav,
        delta_star,
        n_edges: 1,
        edge_idx: p_hi.clamp(1, n - 1),
    }
}

/// [`fit_plane`] with one extra point `(extra_t, extra_z)` folded into the
/// regression — the diffraction point D for the explicit-edge δ\* fits.
fn fit_plane_with_point(
    ts: &[f64],
    zs: &[f64],
    extra_t: f64,
    extra_z: f64,
    t_offset: f64,
    total_dist: f64,
) -> (f64, f64) {
    let n = zs.len() as f64 + 1.0;
    let mut sx = 0.0_f64;
    let mut sz = 0.0_f64;
    let mut sxx = 0.0_f64;
    let mut sxz = 0.0_f64;
    for (&ti, &z) in ts
        .iter()
        .zip(zs.iter())
        .chain(std::iter::once((&extra_t, &extra_z)))
    {
        let x = (ti - t_offset) * total_dist;
        sx += x;
        sz += z;
        sxx += x * x;
        sxz += x * z;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-9 {
        return (0.0, sz / n);
    }
    let a = (n * sxz - sx * sz) / denom;
    let b = (sz - a * sx) / n;
    (a, b)
}

/// The Rayleigh δ\* of 2021/1226 point (9)(c) — mirror source and mirror
/// receiver across the two per-side OLS mean ground planes, same edge D.
/// Reported in the path trace; it gates nothing (module header).
fn compute_delta_star(
    t: &[f64],
    profile: &[f64],
    d_idx: usize,
    total_dist: f64,
    source_height: f64,
    receiver_height: f64,
) -> f64 {
    let n = profile.len();
    let d_sg = t[d_idx] * total_dist;
    let d_rg = (1.0 - t[d_idx]) * total_dist;

    let (_, b_src) = fit_mean_ground_plane(&t[..=d_idx], &profile[..=d_idx], 0.0, total_dist);
    let (a_rcv, b_rcv) =
        fit_mean_ground_plane(&t[d_idx..], &profile[d_idx..], t[d_idx], total_dist);
    let plane_rcv_at_end = a_rcv * d_rg + b_rcv;

    let s_star_z = 2.0 * b_src - (profile[0] + source_height);
    let r_star_z = 2.0 * plane_rcv_at_end - (profile[n - 1] + receiver_height);

    let d_top = profile[d_idx];
    let d_sd = (d_sg * d_sg + (d_top - s_star_z).powi(2)).sqrt();
    let d_dr = (d_rg * d_rg + (r_star_z - d_top).powi(2)).sqrt();
    let d_sr = (total_dist * total_dist + (r_star_z - s_star_z).powi(2)).sqrt();
    (d_sd + d_dr - d_sr).max(0.0)
}

/// Unweighted OLS mean-ground plane used by every CNOSSOS ground consumer.
///
/// The diffraction `δ*` construction fits one such plane on each side of its
/// dominant edge.  A direct-ground path fits it over the whole bare-earth ray.
/// Keeping the regression here prevents the two consumers from silently
/// acquiring different mean-ground conventions.
pub(crate) fn fit_mean_ground_plane(
    ts: &[f64],
    zs: &[f64],
    t_offset: f64,
    total_dist: f64,
) -> (f64, f64) {
    let n = zs.len() as f64;
    debug_assert_eq!(ts.len(), zs.len());
    if n < 1.0 {
        return (0.0, 0.0);
    }
    let mut sx = 0.0_f64;
    let mut sz = 0.0_f64;
    let mut sxx = 0.0_f64;
    let mut sxz = 0.0_f64;
    for (&ti, &z) in ts.iter().zip(zs.iter()) {
        let x = (ti - t_offset) * total_dist;
        sx += x;
        sz += z;
        sxx += x * x;
        sxz += x * z;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-9 {
        return (0.0, sz / n);
    }
    let a = (n * sxz - sx * sz) / denom;
    let b = (sz - a * sx) / n;
    (a, b)
}

/// Per-band single-edge attenuation over a path difference `delta`.
///
/// `delta` is SIGNED: positive when the edge breaks the line of sight,
/// negative when it passes below it (a near miss). The two arms meet at
/// δ = 0 with 10·log10(3) ≈ 4.8 dB:
///
/// - δ ≥ 0 — ISO 9613-2 §7.4 `10·log10(3 + (20/λ)·δ)`, capped at 20 dB.
/// - δ < 0 — CNOSSOS-EU (2.5.21) penumbra `10·log10(3 + (40/λ)·C″·δ)` with
///   C″ = 1, which reaches exactly 0 dB at its own admission bound
///   `(40/λ)·C″·δ = −2` ⟺ δ = −λ/20 and is not computed below it. The steeper
///   40/λ slope IS the standard's negative branch; without it an edge a
///   millimetre below the sight line drops from 4.8 dB to nothing, the hard
///   shadow edge SPEC §3.5 carried as a known gap (fix-pack Fix 2).
///
/// The Rayleigh criterion of 2021/1226 point (9)(c), per band: **is this edge a
/// diffractor at all?** Two things pin where it may be asked.
///
/// **Only on an unblocked ray.** "*If the direct ray is not blocked*" is the
/// sentence it lives in, and "*because these path length differences are
/// negative*" is the standard's own note that δ < 0 throughout that branch. A
/// blocked ray meets no λ/4 test in CNOSSOS, in the 2021 amendment, in
/// ISO 9613-2 or in NoiseModelling; asking it there was this engine's own
/// addition and put a 7.40–9.03 dB step at `δ = λ/4 − δ*` (module header).
///
/// **Once per path, on the homogeneous straight-ray δ**, and the verdict is
/// then spent on BOTH meteorological states. This is a DELIBERATE DEVIATION
/// and the one place this module departs from ISO/TR 17534-4 §5.9, which says
/// "*The decision whether diffraction must be calculated is made separately for
/// homogeneous and favourable conditions respectively*". The standard can
/// afford that because it re-derives δ\* per state on curved rays too
/// (NoiseModelling's `computeRayleighDiff` has a whole `toCurve` branch for
/// it); ours is straight-geometry by review-pinned choice, so asking the
/// criterion per state would pair a CURVED δ_F with a STRAIGHT δ\* — two
/// constructions mixed. It showed, too: the favourable arm then took its own
/// admission step at `δ_F = 0`, which on a 200 m path lands at `δ = +0.098 m`,
/// i.e. **3.13 dB across 2 mm of wall height at a 5.155 m wall** — an
/// arbitrary-height contour of exactly the kind this fix exists to delete.
/// Asking once on the geometry δ\* is fitted on leaves the sight line as the
/// only transition. Revisit if δ\* ever goes per-state.
fn rayleigh_admits(delta: f64, delta_star: f64) -> [bool; NUM_BANDS] {
    std::array::from_fn(|i| {
        let lambda = SPEED_OF_SOUND / BAND_FREQ[i];
        delta >= 0.0 || delta > lambda / 4.0 - delta_star
    })
}

/// Per-band single-edge attenuation over a path difference `delta`, for the
/// bands `admits` lets through.
fn maekawa_bands(delta: f64, admits: &[bool; NUM_BANDS]) -> [f64; NUM_BANDS] {
    let mut atten = [0.0_f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        if !admits[i] {
            continue;
        }
        let lambda = SPEED_OF_SOUND / BAND_FREQ[i];
        // Positive arm kept in its original `δ·f/c` form — bit-parity with the
        // pre-penumbra kernel (and its CUDA mirror) on every blocking path.
        let n = if delta < 0.0 {
            3.0 + 40.0 * delta / lambda
        } else {
            3.0 + 20.0 * delta * BAND_FREQ[i] / SPEED_OF_SOUND
        };
        if n <= 1.0 {
            continue; // δ < −λ/20: outside the penumbra, and 0 dB at the bound
        }
        atten[i] = (10.0 * n.log10()).min(SINGLE_DIFF_CAP);
    }
    atten
}

/// Pure Maekawa band attenuation (no Rayleigh criterion) — reference-vector
/// helper.
pub fn diffraction_attenuation(delta: f64) -> [f64; NUM_BANDS] {
    maekawa_bands(delta, &[true; NUM_BANDS])
}

/// The banded attenuation of a computed edge: [`maekawa_bands`] on the
/// homogeneous δ, mixed with the favourable-ray δ_F per (2.5.9) when
/// [`FAVOURABLE_MIXING`] is on. One [`rayleigh_admits`] verdict feeds both.
pub fn diffraction_attenuation_mixed(result: &DiffractionResult) -> [f64; NUM_BANDS] {
    // NO EDGE means NO ATTENUATION, and it has to be spelled out: `n_edges == 0`
    // is a clear path (`empty_result`), but `maekawa_bands(δ = 0, admitted)` is
    // `10·lg(3)` = 4.77 dB in EVERY band — the (2.5.21) curve's value AT the
    // grazing point, not zero. Every caller today filters clear paths out before
    // reaching here, so this returned the right answer by luck; returning `hom`
    // for an edge-less result was a trap armed for the first caller that did not
    // (2026-08-08 review).
    if result.n_edges == 0 {
        return [0.0; NUM_BANDS];
    }
    let admits = rayleigh_admits(result.delta, result.delta_star);
    let hom = maekawa_bands(result.delta, &admits);
    if !FAVOURABLE_MIXING {
        return hom;
    }
    let fav = maekawa_bands(result.delta_fav, &admits);
    mix_fav_hom(&hom, &fav, P_FAV)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact Lipschitz constant of [`maekawa_bands`] in δ [dB/m].
    ///
    /// `A = 10·lg(n)`, so `dA/dδ = (10/ln10)·(1/n)·dn/dδ`. The steepest arm is
    /// the (2.5.21) penumbra one, `dn/dδ = 40/λ`, at its own admission bound
    /// `n = 1`, in the shortest wavelength: `(10/ln10)·40·f₈ₖ/c ≈ 4087 dB/m`.
    /// The positive arm is gentler on both counts (`20/λ`, and `n ≥ 3`), and
    /// the 20 dB cap only ever flattens the curve. Sup of |A'| over the whole
    /// domain, hence a bound on every secant slope.
    pub(super) const MAX_DB_PER_M_OF_DELTA: f64 =
        (10.0 / std::f64::consts::LN_10) * 40.0 * 8000.0 / SPEED_OF_SOUND;

    #[test]
    fn test_k6_barrier_atten() {
        let atten = diffraction_attenuation(0.5);
        let at_1khz = atten[4];
        assert!(
            (at_1khz - 15.28).abs() < 1.0,
            "K6 1kHz: expected ~15.28, got {:.2}",
            at_1khz
        );
    }

    /// Kytín-shaped geometry: source and receiver ~2.2 km apart, an edge 25 m
    /// above the LOS near mid-path. δ_H comes out sub-metre (matches the live
    /// popup's δ = 0.9 m); the curved favourable ray must always shorten the
    /// detour (δ_F < δ_H), here enough to go NEGATIVE — the curved ray clears
    /// the hill, which is exactly the "distant motorway audible under
    /// inversion" mechanism the plan implements.
    fn kytin_edge() -> (f64, f64, f64) {
        let d_sg = 1000.0_f64;
        let d_rg = 1172.0_f64;
        let rise = 25.0_f64;
        let d_sb = (d_sg * d_sg + rise * rise).sqrt();
        let d_br = (d_rg * d_rg + rise * rise).sqrt();
        let dsr = d_sg + d_rg; // level endpoints
        (d_sb, d_br, dsr)
    }

    #[test]
    fn curved_ray_shortens_the_detour() {
        let (d_sb, d_br, dsr) = kytin_edge();
        let delta_h = d_sb + d_br - dsr;
        assert!(
            delta_h > 0.5 && delta_h < 1.0,
            "fixture δ_H ≈ 0.58 m, got {delta_h:.3}"
        );
        let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
        let delta_f = curved_path_difference(d_sb, d_br, dsr, gamma);
        assert!(delta_f < delta_h, "δ_F must be smaller than δ_H");
        assert!(
            delta_f < 0.0,
            "at 2.2 km the Γ=8·d curvature clears this sub-metre-δ hill (got {delta_f:.3})"
        );
    }

    /// Γ → ∞ recovers straight rays: δ_F → δ_H.
    #[test]
    fn infinite_curvature_recovers_straight_delta() {
        let (d_sb, d_br, dsr) = kytin_edge();
        let delta_h = d_sb + d_br - dsr;
        let delta_f = curved_path_difference(d_sb, d_br, dsr, 1.0e12);
        assert!(
            (delta_f - delta_h).abs() < 1e-6,
            "Γ→∞: δ_F {delta_f:.9} must equal δ_H {delta_h:.9}"
        );
    }

    /// (2.5.9) mix endpoints and monotonicity: p=0 → homogeneous, p=1 →
    /// favourable, p=0.5 strictly between and BELOW the arithmetic midpoint
    /// (energetic mean leans to the louder, less-attenuated state).
    #[test]
    fn mix_endpoints_and_energetic_lean() {
        let hom = [12.0_f64; NUM_BANDS];
        let fav = [2.0_f64; NUM_BANDS];
        let m0 = mix_fav_hom(&hom, &fav, 0.0);
        let m1 = mix_fav_hom(&hom, &fav, 1.0);
        let mh = mix_fav_hom(&hom, &fav, 0.5);
        for i in 0..NUM_BANDS {
            assert!((m0[i] - hom[i]).abs() < 1e-9);
            assert!((m1[i] - fav[i]).abs() < 1e-9);
            assert!(mh[i] > fav[i] && mh[i] < hom[i]);
            assert!(
                mh[i] < (hom[i] + fav[i]) / 2.0,
                "energetic mean must sit below the dB midpoint (louder state dominates)"
            );
        }
        // 10 dB spread at p=0.5 → mixed ≈ fav + 2.6 dB (= −10·log10(0.5·(1+10⁻¹)) above fav).
        assert!((mh[0] - (fav[0] + 2.61)).abs() < 0.05, "got {:.3}", mh[0]);
    }

    /// Flag ON (the shipped state since the 2026-07-28 flip): the public band
    /// function must equal the explicit favourable/homogeneous mix. The
    /// constant assert forces whoever flips the flag again to rewrite this
    /// test consciously (the OFF-era twin did the same job in reverse).
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn flag_on_matches_explicit_mix() {
        assert!(FAVOURABLE_MIXING, "shipped state is ON (flip 2026-07-28)");
        let r = DiffractionResult {
            delta: 0.7,
            delta_fav: -0.1,
            delta_star: 0.05,
            n_edges: 1,
            edge_idx: 3,
        };
        let public = diffraction_attenuation_mixed(&r);
        let admits = rayleigh_admits(r.delta, r.delta_star);
        let hom = maekawa_bands(r.delta, &admits);
        let fav = maekawa_bands(r.delta_fav, &admits);
        assert_eq!(public, mix_fav_hom(&hom, &fav, P_FAV));
    }

    /// A flat bare-earth profile at elevation 0 with `n` cadence samples.
    fn flat_profile(n: usize) -> (Vec<f64>, Vec<f64>) {
        let t: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        (t, vec![0.0_f64; n])
    }

    /// THE CONTINUITY GATE for (2.5.26)↔(2.5.27).
    ///
    /// The two formulas describe ONE quantity on either side of the sight line
    /// and must agree where they meet: at the crossing A = O, so
    /// `2ŜA + 2ÂR − ŜO − ÔR − ŜR` collapses to `ŜO + ÔR − ŜR`. Sweeping a
    /// screen through the sight line in 2 mm steps, neither δ_F nor the mixed
    /// band attenuation may step.
    ///
    /// The straight-ray negative arm this replaced (`δ_F = −δ`) is the
    /// STRAIGHT-ray reading of the very same (2.5.27) — correct chords, no
    /// arcs — so pairing it with the curved positive arm left the whole arc
    /// correction as a cliff: δ_F jumped ~0.098 m at the crossing on this
    /// geometry, 4.77 → 1.76 dB at 8 kHz. That cliff is what made a TALLER
    /// screen come out LOUDER in 47/108 wall geometries
    /// (`arc_screening::taller_screen_never_makes_the_receiver_louder`).
    #[test]
    fn favourable_delta_is_continuous_across_the_sight_line() {
        // (total_dist, source height, receiver height, edge station)
        for &(dist, src_h, rcv_h, t_e) in &[
            (200.0_f64, 0.05_f64, 4.0_f64, 0.5_f64),
            (200.0, 0.05, 4.0, 0.25),
            (80.0, 0.5, 1.5, 0.6),
            (1200.0, 0.05, 10.0, 0.35),
        ] {
            let (t, ols) = flat_profile(33);
            let (src_elev, rcv_elev) = (src_h, rcv_h);
            let dsr = (dist * dist + (rcv_elev - src_elev).powi(2)).sqrt();
            let los = src_elev + (rcv_elev - src_elev) * t_e;

            let (mut prev, mut prev_bands): (Option<(f64, f64)>, Option<[f64; NUM_BANDS]>) =
                (None, None);
            for k in 0..=60 {
                let top_e = los - 0.06 + k as f64 * 0.002;
                let r = compute_single_edge_at(
                    &t, &ols, t_e, top_e, dist, src_elev, rcv_elev, dsr, src_h, rcv_h,
                );
                let bands = diffraction_attenuation_mixed(&r);
                if let (Some((pd, pf)), Some(pb)) = (prev, prev_bands) {
                    assert!(
                        (r.delta_fav - pf).abs() < 1.0e-3,
                        "δ_F stepped {:.6} m at top {top_e:.4} (d={dist}, t_e={t_e})",
                        r.delta_fav - pf
                    );
                    // THE ONE EXEMPT SAMPLE. Where δ changes sign the 2021/1226
                    // criterion stops being consulted at all ("*If the direct
                    // ray is not blocked*"), so a band it was rejecting takes
                    // the blocked branch's value in one step. That edge is the
                    // standard's, is bounded, and is pinned on its own in
                    // `the_sight_line_step_is_the_standards_own_and_bounded`.
                    // Its analytic ceiling is `10·lg 3` — each arm steps from 0
                    // to at most `10·lg 3`, and the (2.5.9) mix is 1-Lipschitz
                    // in each of its dB inputs with weights summing to 1.
                    // Measured worst over these geometries: 4.32 dB at 63 Hz,
                    // where the favourable arm is still inside its own penumbra.
                    let bar = if (pd < 0.0) != (r.delta < 0.0) {
                        10.0 * 3.0_f64.log10() + 1.0e-9
                    } else {
                        0.05
                    };
                    for i in 0..NUM_BANDS {
                        assert!(
                            (bands[i] - pb[i]).abs() < bar,
                            "band {i} stepped {:.3} dB at top {top_e:.4} (d={dist}, t_e={t_e}): \
                             {:.3} → {:.3}",
                            bands[i] - pb[i],
                            pb[i],
                            bands[i]
                        );
                    }
                }
                prev = Some((r.delta, r.delta_fav));
                prev_bands = Some(bands);
            }
        }
    }

    /// …and the two arms are not merely close at the crossing, they are the
    /// SAME NUMBER: evaluated with the edge exactly on the sight line, (2.5.27)
    /// must return (2.5.26) to the last bits the two expression orders allow.
    #[test]
    fn near_miss_arm_meets_the_blocked_arm_on_the_sight_line() {
        let (dist, src_elev, rcv_elev, t_e) = (200.0_f64, 0.05_f64, 4.0_f64, 0.5_f64);
        let dsr = (dist * dist + (rcv_elev - src_elev).powi(2)).sqrt();
        let los = src_elev + (rcv_elev - src_elev) * t_e;
        let (d_sg, d_rg) = (t_e * dist, (1.0 - t_e) * dist);
        // O ON the sight line ⇒ A = O ⇒ d_SA = d_SO and d_AR = d_OR.
        let d_sb = (d_sg * d_sg + (los - src_elev).powi(2)).sqrt();
        let d_br = (d_rg * d_rg + (los - rcv_elev).powi(2)).sqrt();
        let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
        let blocked = curved_path_difference(d_sb, d_br, dsr, gamma);
        let near_miss = curved_path_difference_near_miss(d_sb, d_br, d_sb, d_br, dsr, gamma);
        assert!(
            (blocked - near_miss).abs() < 1e-12,
            "(2.5.26) {blocked:.15} ≠ (2.5.27) {near_miss:.15} on the sight line"
        );
        // And it is the CURVED value, not zero: the arc correction is what the
        // straight-ray arm used to throw away at the crossing.
        assert!(
            near_miss < -0.09 && near_miss > -0.11,
            "arc correction on this geometry is ≈ −0.098 m, got {near_miss:.6}"
        );
    }

    /// Γ → ∞ collapses (2.5.27) onto the straight-ray form `−(d_SO + d_OR − d_SR)`
    /// — the expression the negative arm used to carry unconditionally. This is
    /// what makes the old arm a LIMIT of the new one rather than a rival model,
    /// and it pins the `d_SA + d_AR = d_SR` identity the derivation rests on.
    #[test]
    fn infinite_curvature_recovers_the_straight_near_miss() {
        let (dist, src_elev, rcv_elev, t_e) = (200.0_f64, 0.05_f64, 4.0_f64, 0.4_f64);
        let dsr = (dist * dist + (rcv_elev - src_elev).powi(2)).sqrt();
        let los = src_elev + (rcv_elev - src_elev) * t_e;
        let top_e = los - 0.03;
        let (d_sg, d_rg) = (t_e * dist, (1.0 - t_e) * dist);
        let d_sb = (d_sg * d_sg + (top_e - src_elev).powi(2)).sqrt();
        let d_br = (d_rg * d_rg + (top_e - rcv_elev).powi(2)).sqrt();
        let d_sa = (d_sg * d_sg + (los - src_elev).powi(2)).sqrt();
        let d_ar = (d_rg * d_rg + (rcv_elev - los).powi(2)).sqrt();
        let straight = d_sb + d_br - dsr;
        let curved = curved_path_difference_near_miss(d_sa, d_ar, d_sb, d_br, dsr, 1.0e12);
        assert!(
            (curved - -straight).abs() < 1e-6,
            "Γ→∞: (2.5.27) {curved:.9} must equal −δ {:.9}",
            -straight
        );
    }

    /// THE CONTINUITY GATE for the removed Rayleigh δ\* gate.
    ///
    /// Diffraction attenuation is a continuous function of the geometry, so
    /// growing an obstacle by a millimetre may not move a band by 7 dB. The
    /// sweep runs a wall from BELOW the sight line up past `δ = λ₆₃/4`, which
    /// is above the old cut `λ/4 − δ*` in EVERY band (δ\* ≥ 0), so it crosses
    /// every gate the old code carried and the crossing of the sight line
    /// (2.5.26)↔(2.5.27) as well.
    ///
    /// The predecessor test `favourable_delta_is_continuous_across_the_sight_line`
    /// could not see this: ±0.06 m of wall height at mid-path on 200 m is only
    /// ±3.6e-5 m of δ, four orders of magnitude short of the 1 kHz cut at
    /// δ = 0.044 m. A continuity sweep has to span the gate, not the geometry
    /// that looks interesting.
    ///
    /// PINNED AGAINST THE ANALYTIC BOUND, not a guessed tolerance. The band
    /// value is Lipschitz in δ with constant [`MAX_DB_PER_M_OF_DELTA`], the
    /// (2.5.9) mix is a weighted energy mean and so 1-Lipschitz in each of its
    /// dB inputs, and the sweep knows its own `Δδ` and `Δδ_F` at every step —
    /// so `|ΔA| ≤ L·max(|Δδ|, |Δδ_F|)` is an exact statement about this
    /// function, and any jump violates it by construction. The absolute bar is
    /// carried alongside because it is what a reader checks: the largest step
    /// measured over all five geometries is **0.45 dB** (8 kHz, favourable arm
    /// entering its own −λ/20 knee, where the true slope really is ~4000 dB/m),
    /// against the 7.48 dB the gate used to put on 1 kHz two metres away.
    ///
    /// ONE sample is exempt from the Lipschitz bound and carries its own,
    /// larger bar: the step on which δ changes sign. That is the standard's own
    /// sight-line edge (`the_sight_line_step_is_the_standards_own_and_bounded`),
    /// not a gate — it does not move with the obstacle, it IS the obstacle
    /// reaching the sight line.
    #[test]
    fn attenuation_is_continuous_in_obstacle_height() {
        // (total_dist, source height, receiver height, edge station)
        for &(dist, src_h, rcv_h, t_e) in &[
            (200.0_f64, 0.05_f64, 4.0_f64, 0.5_f64),
            (200.0, 0.05, 4.0, 0.25),
            (400.0, 0.05, 4.0, 0.5),
            (80.0, 0.5, 1.5, 0.6),
            (1200.0, 0.05, 10.0, 0.35),
        ] {
            let (t, ols) = flat_profile(33);
            let (src_elev, rcv_elev) = (src_h, rcv_h);
            let dsr = (dist * dist + (rcv_elev - src_elev).powi(2)).sqrt();
            let los = src_elev + (rcv_elev - src_elev) * t_e;
            let eval = |top_e: f64| {
                let r = compute_single_edge_at(
                    &t, &ols, t_e, top_e, dist, src_elev, rcv_elev, dsr, src_h, rcv_h,
                );
                (r.delta, r.delta_fav, diffraction_attenuation_mixed(&r))
            };

            // Tall enough that δ clears λ₆₃/4 = 1.349 m — the largest cut the
            // old gate could place (δ* ≥ 0 only ever lowered it).
            let mut top_hi = los;
            while eval(top_hi).0 < SPEED_OF_SOUND / BAND_FREQ[0] / 4.0 {
                top_hi += 0.5;
            }
            let lo = los - 0.05;
            let steps = ((top_hi - lo) / 0.002).ceil() as usize;

            let mut prev = eval(lo);
            let (mut worst, mut worst_at, mut crossings) = (0.0_f64, (0.0_f64, 0usize), 0);
            for k in 1..=steps {
                let top_e = lo + k as f64 * 0.002;
                let cur = eval(top_e);
                let crosses_sight_line = (prev.0 < 0.0) != (cur.0 < 0.0);
                crossings += usize::from(crosses_sight_line);
                let lip = if crosses_sight_line {
                    10.0 * 3.0_f64.log10() // the standard's own edge, see above
                } else {
                    MAX_DB_PER_M_OF_DELTA * (cur.0 - prev.0).abs().max((cur.1 - prev.1).abs())
                } + 1.0e-9;
                for i in 0..NUM_BANDS {
                    let step = (cur.2[i] - prev.2[i]).abs();
                    assert!(
                        step <= lip,
                        "band {i} stepped {step:.4} dB over 2 mm at top {top_e:.4} m, \
                         past the {lip:.4} dB allowed (d={dist}, t_e={t_e})"
                    );
                    if step > worst && !crosses_sight_line {
                        worst = step;
                        worst_at = (top_e, i);
                    }
                }
                prev = cur;
            }
            assert_eq!(
                crossings, 1,
                "the sweep must cross the sight line exactly once"
            );
            assert!(
                worst < 0.6,
                "band {} stepped {worst:.3} dB over 2 mm at top {:.4} m \
                 (d={dist}, t_e={t_e}, sweep {lo:.3}..{top_hi:.3})",
                worst_at.1,
                worst_at.0,
            );
        }
    }

    /// THE DEFECT ITSELF, pinned on the geometry it was reported on: 200 m
    /// path, 0.05 m source, 4 m receiver, wall at mid-path. The old
    /// `δ ≤ λ/4 − δ*` gate cut the 1 kHz band at δ = λ/4 − δ* = 0.04402 m,
    /// which this wall reaches at 4.1239 m — so 4.1229 m returned 0.00 dB and
    /// 4.1249 m returned 7.48 dB, a 7.48 dB step across 2 mm of wall. Both
    /// sides must now read the same ~7.47 dB the formula always gave.
    #[test]
    fn the_reported_1khz_wall_step_is_gone() {
        let (dist, src_elev, rcv_elev, t_e) = (200.0_f64, 0.05_f64, 4.0_f64, 0.5_f64);
        let dsr = (dist * dist + (rcv_elev - src_elev).powi(2)).sqrt();
        let (d_sg, d_rg) = (t_e * dist, (1.0 - t_e) * dist);
        let delta_of = |top: f64| {
            let d_sb = (d_sg * d_sg + (top - src_elev).powi(2)).sqrt();
            let d_br = (d_rg * d_rg + (top - rcv_elev).powi(2)).sqrt();
            d_sb + d_br - dsr
        };
        // δ* of this scene (flat bare ground, D at the ground under the edge)
        // is 0.0410 m, which put the 1 kHz cut at δ = 0.085 − 0.041 = 0.044 m.
        // Any δ* at all reproduces the old step here; the criterion is simply
        // no longer consulted on a blocked ray.
        let ds = 0.040_969;
        let below = maekawa_bands(delta_of(4.1229), &rayleigh_admits(delta_of(4.1229), ds));
        let above = maekawa_bands(delta_of(4.1249), &rayleigh_admits(delta_of(4.1249), ds));
        assert!(
            (above[4] - below[4]).abs() < 0.01,
            "1 kHz stepped {:.3} dB across 2 mm: {:.3} → {:.3}",
            above[4] - below[4],
            below[4],
            above[4]
        );
        assert!(
            below[4] > 7.4 && below[4] < 7.5,
            "the formula's own value at δ ≈ 0.044 m is ≈ 7.47 dB, got {:.3}",
            below[4]
        );
    }

    /// Mixed attenuation is never above homogeneous and never below favourable
    /// (per band), across a sweep of edge geometries including a two-bump-like
    /// tall/late edge — the property G1 pins for the ON state.
    #[test]
    fn mixed_bands_bounded_by_states() {
        for (d_sg, d_rg, rise) in [
            (1000.0_f64, 1172.0_f64, 25.0_f64), // Kytín-shaped
            (300.0, 1900.0, 60.0),              // tall late edge (two-bump winner shape)
            (50.0, 150.0, 8.0),                 // short urban path
        ] {
            let d_sb = (d_sg * d_sg + rise * rise).sqrt();
            let d_br = (d_rg * d_rg + rise * rise).sqrt();
            let dsr = d_sg + d_rg;
            let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
            let delta_h = d_sb + d_br - dsr;
            let delta_f = curved_path_difference(d_sb, d_br, dsr, gamma);
            assert!(delta_f < delta_h);
            let admits = rayleigh_admits(delta_h, 0.0);
            let hom = maekawa_bands(delta_h, &admits);
            let fav = maekawa_bands(delta_f, &admits);
            let mixed = mix_fav_hom(&hom, &fav, P_FAV);
            for i in 0..NUM_BANDS {
                assert!(mixed[i] <= hom[i] + 1e-9 && mixed[i] >= fav[i] - 1e-9);
            }
        }
    }
}

#[cfg(test)]
mod delta_reject_tests {
    use super::*;
    use crate::constants::PENUMBRA_DELTA_FLOOR_M;

    /// The tightest EXACT rejection floor once a ray's Rayleigh δ\* is known.
    ///
    /// A band is silent either because the 2021/1226 criterion rejects it
    /// (δ < 0 only, since 2026-08-05) or because it is past the (2.5.21) bound,
    /// so on the negative side its threshold is `max(λ_i/4 − δ*, −λ_i/20)`. A
    /// non-negative δ is never silent — `n = 3` at worst — which is what the
    /// `min(…, 0)` records: for the small δ\* of ordinary geometry the criterion
    /// swallows the whole penumbra and the floor is the sight line itself.
    ///
    /// NOTE the direction of safety: this is DECREASING in δ\*, so a caller may
    /// only ever pass a LOWER bound on δ\*. Passing a δ\* larger than the true
    /// one prunes obstacles the kernel would have attenuated.
    fn delta_reject_for(delta_star: f64) -> f64 {
        let mut floor = f64::INFINITY;
        for &f in BAND_FREQ.iter() {
            let lambda = SPEED_OF_SOUND / f;
            floor = floor.min((lambda / 4.0 - delta_star).max(-lambda / 20.0));
        }
        floor.min(0.0)
    }

    /// [`PENUMBRA_DELTA_FLOOR_M`] is a FLOOR, not a guess — pinned in both
    /// directions, plus the trap that made it worth pinning.
    #[test]
    fn penumbra_floor_is_the_tightest_delta_star_free_floor() {
        assert!(
            (PENUMBRA_DELTA_FLOOR_M - -0.269_841_269_8).abs() < 1e-9,
            "{PENUMBRA_DELTA_FLOOR_M}"
        );
        // At or below it: silent for every δ* a fit could produce.
        for &ds in &[0.0, 0.001, 0.01, 0.1, 1.0, 10.0, 1e4] {
            for &d in &[PENUMBRA_DELTA_FLOOR_M, PENUMBRA_DELTA_FLOOR_M - 1e-6, -50.0] {
                let bands = maekawa_bands(d, &rayleigh_admits(d, ds));
                assert!(
                    bands.iter().all(|&b| b == 0.0),
                    "δ={d} δ*={ds} produced {bands:?}"
                );
            }
        }
        // THE TRAP: −λ/20 of the HIGHEST band is not a floor at all. A prune
        // written against it drops real attenuation in the mid bands.
        let trap = -(SPEED_OF_SOUND / 8000.0) / 20.0;
        let bands = maekawa_bands(trap, &rayleigh_admits(trap, 0.1));
        assert!(
            bands.iter().any(|&b| b > 0.0),
            "λ_8k/20 must NOT be silent — that is the trap this constant replaces"
        );
    }

    /// The per-ray floor is exact for its own δ\*: silent below it, not silent
    /// just above it, and never looser than the δ\*-free constant.
    ///
    /// The `+λ_8k/4 = +0.0106 m` this used to return for a small δ\* was the
    /// blocked-side gate talking, and it was WRONG in the direction that
    /// deletes signal — a prune written against it would have dropped every
    /// obstacle with `0 < δ < 0.0106 m`, which the kernel attenuates by up to
    /// 4.8 dB. With the criterion scoped to unblocked rays the floor can never
    /// exceed 0.
    #[test]
    fn delta_reject_for_is_exact_per_ray() {
        for &ds in &[0.0, 0.005, 0.05, 0.5, 5.0] {
            let floor = delta_reject_for(ds);
            assert!(
                (PENUMBRA_DELTA_FLOOR_M..=0.0).contains(&floor),
                "δ*={ds} floor={floor}"
            );
            for &d in &[floor - 1e-9, floor - 1.0] {
                assert!(
                    maekawa_bands(d, &rayleigh_admits(d, ds))
                        .iter()
                        .all(|&b| b == 0.0),
                    "δ*={ds} floor={floor} δ={d}"
                );
            }
            assert!(
                maekawa_bands(floor + 1e-6, &rayleigh_admits(floor + 1e-6, ds))
                    .iter()
                    .any(|&b| b > 0.0),
                "δ*={ds} floor={floor} is timid"
            );
        }
        // Small δ*: the criterion owns the whole penumbra, so nothing below the
        // sight line survives — and nothing above it is ever rejected.
        assert_eq!(delta_reject_for(0.0), 0.0);
        // Large δ*: the criterion is inert and the (2.5.21) bound governs.
        assert!((delta_reject_for(5.0) - PENUMBRA_DELTA_FLOOR_M).abs() < 1e-12);
    }

    /// δ-space continuity, pinned against the analytic Lipschitz bound
    /// [`super::tests::MAX_DB_PER_M_OF_DELTA`] × step. Two regimes, and the
    /// boundary between them is the whole point of this file:
    ///
    /// * **δ\* past `0.3·λ₆₃`** — the criterion is inert in every band (it is
    ///   weaker than the (2.5.21) bound) and the curve is continuous over its
    ///   entire domain, penumbra floor included.
    /// * **any δ\*** — above the sight line there is NO admission test left, so
    ///   the blocked branch is continuous for every δ\*. This is the assertion
    ///   that fails the moment a λ/4 cut is reintroduced on a blocked ray, and
    ///   it is the one the reported defect broke.
    #[test]
    fn attenuation_is_continuous_in_delta() {
        let step = 1.0e-5_f64;
        let bound = super::tests::MAX_DB_PER_M_OF_DELTA * step + 1.0e-9;
        for &(lo, hi, ds) in &[
            // Criterion inert (δ* > 0.3·λ₆₃ = 1.619 m): continuous everywhere.
            (-0.5_f64, 1.5_f64, 2.0_f64),
            // Criterion active, blocked branch only: continuous from the sight
            // line up, for a δ* that rejects the entire penumbra.
            (0.0, 1.5, 0.0),
            (0.0, 1.5, 0.041),
        ] {
            let steps = ((hi - lo) / step) as usize;
            let mut prev = maekawa_bands(lo, &rayleigh_admits(lo, ds));
            for k in 1..=steps {
                let d = lo + k as f64 * step;
                let bands = maekawa_bands(d, &rayleigh_admits(d, ds));
                for i in 0..NUM_BANDS {
                    assert!(
                        (bands[i] - prev[i]).abs() <= bound,
                        "band {i} stepped {:.4} dB at δ = {d:.6} m (δ*={ds}) past the \
                         {bound:.4} dB bound: {:.3} → {:.3}",
                        bands[i] - prev[i],
                        prev[i],
                        bands[i]
                    );
                }
                prev = bands;
            }
        }
    }

    /// The one step the standard leaves behind, PINNED so it cannot grow.
    ///
    /// A band whose `δ* ≤ λ/4` is rejected by the criterion at 0⁻ and takes the
    /// blocked branch's `10·lg 3` at 0⁺. That is 2021/1226's own edge — its
    /// "otherwise" branch swaps the two split mean ground planes for one common
    /// plane and returns `Aground` instead (2.5.30–2.5.32), which this engine
    /// does not implement. Bounded by `10·lg 3 = 4.771 dB` on the homogeneous
    /// arm and, after the (2.5.9) mix, 1.76 dB on the shipped path.
    #[test]
    fn the_sight_line_step_is_the_standards_own_and_bounded() {
        for &ds in &[0.0_f64, 0.041, 0.084] {
            let below = maekawa_bands(-1.0e-12, &rayleigh_admits(-1.0e-12, ds));
            let above = maekawa_bands(1.0e-12, &rayleigh_admits(1.0e-12, ds));
            for i in 0..NUM_BANDS {
                let lambda = SPEED_OF_SOUND / BAND_FREQ[i];
                let gated = -1.0e-12 <= lambda / 4.0 - ds;
                assert_eq!(below[i] == 0.0, gated, "band {i} δ*={ds}");
                assert!(
                    (above[i] - below[i]).abs() <= 10.0 * 3.0_f64.log10() + 1e-9,
                    "band {i} δ*={ds} stepped {:.4} dB across the sight line",
                    above[i] - below[i]
                );
            }
        }
    }
}
