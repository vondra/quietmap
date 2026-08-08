//! Single-shot per-segment SEL wrappers.
//!
//! Bridges the popup / test entry points to `segment_energy_kernel`.
//! Hot-path callers (popup per-segment loop, pipeline scatter) hoist the
//! `&NpdLuts` reference and pre-sample terrain via `SegmentTerrain` to
//! skip the per-call `OnceLock` Acquire load and raster lookups.

use crate::types::{AircraftSegment, RasterSampler};

use super::doc29::{
    delta_i_constants, delta_v, segment_energy_kernel, AircraftKernelResult, CpaResult,
    M_PER_DEG_LAT,
};
use super::horizon::ReceiverHorizon;
use super::npd::{
    noise_class_of, Installation, NpdLuts, CLASS_REP_PROFILE_IDX, PROFILES, REACH_SQ_TABLE,
};
use super::segment_filters::SegmentTerrain;

/// Compute SEL for a single aircraft segment at a receiver point.
/// Returns (SEL_dB, CpaResult) or None if segment is too far / inaudible.
///
/// Convenience entry point for low-frequency callers (tests, single-shot
/// queries). Pays one `NpdLuts::shared()` Acquire load per call. Hot
/// users (popup per-segment loop, pipeline scatter) should hoist the
/// `&NpdLuts` reference and call `segment_sel_with_luts` directly.
pub fn segment_sel(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    rasters: &dyn RasterSampler,
) -> Option<(f64, CpaResult)> {
    let terrain_start_cut_m = rasters.elevation(seg.start_lat, seg.start_lon) - 30.0;
    let terrain_end_cut_m = rasters.elevation(seg.end_lat, seg.end_lon) - 30.0;
    segment_sel_with_overrides::<true>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        seg.start_alt_m as f64,
        seg.end_alt_m as f64,
        false,
        terrain_start_cut_m,
        terrain_end_cut_m,
        NpdLuts::shared(),
        None,
    )
}

/// Hot-path variant of `segment_sel` that accepts a caller-hoisted
/// `&NpdLuts`. Use this from inside per-segment loops to avoid an
/// `OnceLock::get_or_init` Acquire load on every call.
pub fn segment_sel_with_luts(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    rasters: &dyn RasterSampler,
    npd_luts: &NpdLuts,
) -> Option<(f64, CpaResult)> {
    let terrain_start_cut_m = rasters.elevation(seg.start_lat, seg.start_lon) - 30.0;
    let terrain_end_cut_m = rasters.elevation(seg.end_lat, seg.end_lon) - 30.0;
    segment_sel_with_overrides::<true>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        seg.start_alt_m as f64,
        seg.end_alt_m as f64,
        false,
        terrain_start_cut_m,
        terrain_end_cut_m,
        npd_luts,
        None,
    )
}

/// Hottest-path variant: terrain cuts are derived from the caller's
/// pre-sampled `SegmentTerrain` cache, skipping the two `rasters.elevation()`
/// calls that `segment_sel_with_luts` pays per segment. Use when the caller
/// already needed `SegmentTerrain` for predicate evaluation.
///
/// CRUISE-ONLY today (popup `cruise.rs` + heatmap cruise scatter), so the
/// C2 horizon is hard-wired to `None`: cruise is structurally exempt from
/// terrain screening — its 7 200 m AGL hysteresis floor and 16 km slant
/// cap put β ≥ 26.6°, above any terrain horizon outside cliff faces.
pub fn segment_sel_with_terrain(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    terrain: &SegmentTerrain,
    npd_luts: &NpdLuts,
) -> Option<(f64, CpaResult)> {
    segment_sel_with_overrides::<true>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        seg.start_alt_m as f64,
        seg.end_alt_m as f64,
        false,
        terrain.start_elev - 30.0,
        terrain.end_elev - 30.0,
        npd_luts,
        None,
    )
}

/// Purely geometric reach gate: `false` guarantees
/// [`segment_sel_with_terrain`] returns `None` for this segment and
/// receiver, **whatever the terrain is**.
///
/// It reproduces the kernel's first gate (`slant_sq > reach_sq` at the top
/// of [`crate::emission::aircraft::doc29::segment_energy_kernel`]) with the
/// same operations in the same order, so the two agree bit for bit. That
/// gate reads no raster and no terrain cut — only the segment endpoints,
/// the receiver, and the class reach table — which is what makes it usable
/// *before* terrain exists.
///
/// Cruise needs this because its `SegmentTerrain` is sampled from the DEM
/// (five probes, each through the tile cache's per-tile lock) while the
/// airborne path reads pre-sampled elevations off the arrow. Measured at
/// Dobříš 2026-08-05: of 9 622 buckets that clear the R7-centre prefilter,
/// 7 611 are then rejected here — they were paying for terrain first.
///
/// The duplicated geometry is deliberate: the kernel's copy works on
/// hoisted per-row scalars for the heatmap fast path and cannot be called
/// with a `&AircraftSegment`. Parity is pinned by
/// `tests::reach_gate_matches_kernel_rejection`.
pub fn within_kernel_reach(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
) -> bool {
    let class_idx = noise_class_of(seg.profile_idx) as usize;
    let reach_sq = REACH_SQ_TABLE[class_idx][seg.is_departure as usize];
    let cos_lat = rx_lat.to_radians().cos().max(0.2);
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;
    let ax = (seg.start_lon - rx_lon) * m_per_deg_lon;
    let ay = (seg.start_lat - rx_lat) * M_PER_DEG_LAT;
    let bx = (seg.end_lon - rx_lon) * m_per_deg_lon;
    let by = (seg.end_lat - rx_lat) * M_PER_DEG_LAT;
    let sdx = bx - ax;
    let sdy = by - ay;
    let seg_len_sq = sdx * sdx + sdy * sdy;
    let inv_lsq = if seg_len_sq > 1e-6 {
        1.0 / seg_len_sq
    } else {
        0.0
    };
    let start_alt_m = seg.start_alt_m as f64;
    let sdz = seg.end_alt_m as f64 - start_alt_m;
    let t = -(ax * sdx + ay * sdy) * inv_lsq;
    let cpx = ax + t * sdx;
    let cpy = ay + t * sdy;
    let rel_alt = start_alt_m + t * sdz - rx_elev_m;
    cpx * cpx + cpy * cpy + rel_alt * rel_alt <= reach_sq
}

/// Energy-only `segment_sel_with_terrain` for the CRUISE heatmap (which discards
/// the CPA). SEL is bit-identical — `WANT_CPA = false` skips the kernel's CPA-only
/// `lateral_m` sqrt. (Cruise is always the CFFK fast path, so the full-path
/// `beta_deg` atan the airborne energy path also skips never runs here.)
/// C2 horizon hard-wired `None` — cruise structural exemption, see
/// [`segment_sel_with_terrain`].
#[inline]
pub fn segment_sel_with_terrain_energy(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    terrain: &SegmentTerrain,
    npd_luts: &NpdLuts,
) -> Option<f64> {
    segment_sel_with_overrides::<false>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        seg.start_alt_m as f64,
        seg.end_alt_m as f64,
        false,
        terrain.start_elev - 30.0,
        terrain.end_elev - 30.0,
        npd_luts,
        None,
    )
    .map(|(sel, _cpa)| sel)
}

/// Pre-computed-cuts variant: the caller already has Filter D's
/// `terrain_*_cut_m` (typically `terrain_*_elev_m - 30.0`, where the
/// endpoint elevations come from v16 `airborne.arrow` sub-segment
/// columns or any other pre-sampled source). Skips the `SegmentTerrain`
/// construct entirely — used by the airborne popup + heatmap hot paths
/// once Stage 1 / Stage 2A have absorbed the mountain-peak validity
/// check. `horizon` enables C2 terrain screening (airborne popup,
/// `QM_AIRBORNE_HORIZON=1`); `None` is byte-identical to pre-C2.
pub fn segment_sel_with_cuts(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    npd_luts: &NpdLuts,
    horizon: Option<&ReceiverHorizon>,
) -> Option<(f64, CpaResult)> {
    segment_sel_with_overrides::<true>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        seg.start_alt_m as f64,
        seg.end_alt_m as f64,
        false,
        terrain_start_cut_m,
        terrain_end_cut_m,
        npd_luts,
        horizon,
    )
}

pub fn segment_sel_airport_ground(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    rasters: &dyn RasterSampler,
) -> Option<(f64, CpaResult)> {
    let start_alt_m =
        (seg.start_alt_m as f64).max(rasters.elevation(seg.start_lat, seg.start_lon) + 4.0);
    let end_alt_m = (seg.end_alt_m as f64).max(rasters.elevation(seg.end_lat, seg.end_lon) + 4.0);
    // Filter D bypass: airport-ground segments are validated by
    // ground-context metadata, so the kernel sees `f64::MIN` cuts.
    // C2 horizon `None` always: airport-ground geometry is on-airport
    // flat apron/runway terrain — no receiver terrain horizon applies.
    segment_sel_with_overrides::<true>(
        seg,
        rx_lat,
        rx_lon,
        rx_elev_m,
        start_alt_m,
        end_alt_m,
        true,
        f64::MIN,
        f64::MIN,
        NpdLuts::shared(),
        None,
    )
}

fn segment_sel_with_overrides<const WANT_CPA: bool>(
    seg: &AircraftSegment,
    rx_lat: f64,
    rx_lon: f64,
    rx_elev_m: f64,
    start_alt_m: f64,
    end_alt_m: f64,
    airport_ground_mode: bool,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
    npd_luts: &NpdLuts,
    horizon: Option<&ReceiverHorizon>,
) -> Option<(f64, CpaResult)> {
    // SEL/v_ref/d_bar/installation come from the class's Voronoi anchor.
    // Per-segment acoustic error vs the segment's own per-typecode profile
    // is bounded by class spread (avg 0.76 dB across global traffic).
    let class_idx = noise_class_of(seg.profile_idx) as usize;
    let anchor_profile = &PROFILES[CLASS_REP_PROFILE_IDX[class_idx] as usize];

    let cos_lat = rx_lat.to_radians().cos().max(0.2);
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;
    let ax = (seg.start_lon - rx_lon) * m_per_deg_lon;
    let ay = (seg.start_lat - rx_lat) * M_PER_DEG_LAT;
    let bx = (seg.end_lon - rx_lon) * m_per_deg_lon;
    let by = (seg.end_lat - rx_lat) * M_PER_DEG_LAT;
    let sdx = bx - ax;
    let sdy = by - ay;
    let seg_len_sq = sdx * sdx + sdy * sdy;
    let slen = seg_len_sq.sqrt().max(1.0);
    let inv_lsq = if seg_len_sq > 1e-6 {
        1.0 / seg_len_sq
    } else {
        0.0
    };
    let sdz = end_alt_m - start_alt_m;

    let (inst_code, di_a, di_b, di_c) = delta_i_constants(anchor_profile.installation);
    let dv = delta_v(seg.speed_kt as f64, anchor_profile);

    // REACH_SQ_TABLE uses the class's loudest-member reach (not the
    // anchor's), so the pre-filter envelope covers Voronoi-assigned
    // outliers like B752 in WING_A320 — a louder member must never be
    // dropped at long range before the kernel sees it.
    let reach_sq = REACH_SQ_TABLE[class_idx][seg.is_departure as usize];

    let kernel: AircraftKernelResult = segment_energy_kernel::<WANT_CPA>(
        ax,
        ay,
        sdx,
        sdy,
        sdz,
        start_alt_m,
        inv_lsq,
        slen,
        rx_elev_m,
        npd_luts,
        class_idx,
        seg.is_departure,
        dv,
        anchor_profile.d_bar_m,
        inst_code,
        di_a,
        di_b,
        di_c,
        airport_ground_mode,
        reach_sq,
        terrain_start_cut_m,
        terrain_end_cut_m,
        horizon,
    )?;

    let cpa = CpaResult {
        q_m: kernel.q_m,
        d_p_m: kernel.d_p_m,
        lateral_m: kernel.lateral_m,
        relative_alt_m: kernel.rel_alt_m,
        beta_deg: kernel.beta_deg,
        seg_len_m: kernel.seg_len_m,
        t: kernel.t,
    };

    Some((kernel.sel, cpa))
}

// Hoisted three-stage variant of `segment_sel_with_cuts` for the heatmap
// fast path (one segment × 262 144 pixels per tile). The split exists
// because `m_per_deg_lon = M_PER_DEG_LAT · cos(rx_lat)` is row-level —
// `sdx`, `slen`, `inv_lsq` all derive from it, so they shift from
// per-pixel work in `segment_sel_with_overrides` to per-row work here.
// `sdy = (end_lat − start_lat) · M_PER_DEG_LAT` is sub-seg-level because
// the lat difference cancels `rx_lat`. Always implies
// `airport_ground_mode = false`; the runway-ground variant is popup-only.
// Parity: `tests::hoisted_matches_segment_sel_with_cuts`.

/// Sub-segment-constant slice of the hoisted aircraft kernel state.
/// Reused across every receiver row the sub-segment touches.
#[derive(Debug, Clone, Copy)]
pub struct SegmentPrepared {
    pub start_lat: f64,
    pub start_lon: f64,
    pub start_alt_m: f64,
    pub d_lon: f64,
    pub sdy: f64,
    pub sdz: f64,
    pub is_departure: bool,
    pub class_idx: usize,
    pub d_bar_m: f64,
    pub dv: f64,
    pub inst: Installation,
    pub di_a: f64,
    pub di_b: f64,
    pub di_c: f64,
    pub reach_sq: f64,
    pub terrain_start_cut_m: f64,
    pub terrain_end_cut_m: f64,
}

/// Row-constant slice of the hoisted aircraft kernel state. Built once
/// per `rx_lat`, reused across every pixel in the row.
#[derive(Debug, Clone, Copy)]
pub struct SegmentRowState {
    pub m_per_deg_lon: f64,
    pub sdx: f64,
    pub slen: f64,
    pub inv_lsq: f64,
    pub ay: f64,
}

/// Hoist of the sub-segment-constant work from
/// `segment_sel_with_overrides`. `terrain_*_cut_m` are typically
/// `terrain_*_elev_m − 30`.
#[inline]
pub fn prepare_segment(
    seg: &AircraftSegment,
    terrain_start_cut_m: f64,
    terrain_end_cut_m: f64,
) -> SegmentPrepared {
    let class_idx = noise_class_of(seg.profile_idx) as usize;
    let anchor_profile = &PROFILES[CLASS_REP_PROFILE_IDX[class_idx] as usize];
    let (inst, di_a, di_b, di_c) = delta_i_constants(anchor_profile.installation);
    let dv = delta_v(seg.speed_kt as f64, anchor_profile);
    let reach_sq = REACH_SQ_TABLE[class_idx][seg.is_departure as usize];
    let d_lon = seg.end_lon - seg.start_lon;
    let sdy = (seg.end_lat - seg.start_lat) * M_PER_DEG_LAT;
    let sdz = (seg.end_alt_m as f64) - (seg.start_alt_m as f64);

    SegmentPrepared {
        start_lat: seg.start_lat,
        start_lon: seg.start_lon,
        start_alt_m: seg.start_alt_m as f64,
        d_lon,
        sdy,
        sdz,
        is_departure: seg.is_departure,
        class_idx,
        d_bar_m: anchor_profile.d_bar_m,
        dv,
        inst,
        di_a,
        di_b,
        di_c,
        reach_sq,
        terrain_start_cut_m,
        terrain_end_cut_m,
    }
}

/// Hoist of the per-row work from `segment_sel_with_overrides` (minus
/// the per-pixel `ax`).
#[inline]
pub fn prepare_row(prepared: &SegmentPrepared, rx_lat: f64, m_per_deg_lon: f64) -> SegmentRowState {
    let sdx = prepared.d_lon * m_per_deg_lon;
    let seg_len_sq = sdx * sdx + prepared.sdy * prepared.sdy;
    let slen = seg_len_sq.sqrt().max(1.0);
    let inv_lsq = if seg_len_sq > 1e-6 {
        1.0 / seg_len_sq
    } else {
        0.0
    };
    let ay = (prepared.start_lat - rx_lat) * M_PER_DEG_LAT;
    SegmentRowState {
        m_per_deg_lon,
        sdx,
        slen,
        inv_lsq,
        ay,
    }
}

/// Per-pixel call. Returns the same `(sel, CpaResult)` as
/// `segment_sel_with_cuts` would for the same inputs, modulo a few ULPs
/// of f64 non-associative drift (see the parity test). `horizon`: all
/// current heatmap callers pass `None`; P3 wires the per-pixel horizon
/// grid through here.
#[inline]
pub fn segment_sel_at_pixel(
    prepared: &SegmentPrepared,
    row_state: &SegmentRowState,
    rx_lon: f64,
    rx_elev_m: f64,
    npd_luts: &NpdLuts,
    horizon: Option<&ReceiverHorizon>,
) -> Option<(f64, CpaResult)> {
    let ax = (prepared.start_lon - rx_lon) * row_state.m_per_deg_lon;
    let kernel: AircraftKernelResult = segment_energy_kernel::<true>(
        ax,
        row_state.ay,
        row_state.sdx,
        prepared.sdy,
        prepared.sdz,
        prepared.start_alt_m,
        row_state.inv_lsq,
        row_state.slen,
        rx_elev_m,
        npd_luts,
        prepared.class_idx,
        prepared.is_departure,
        prepared.dv,
        prepared.d_bar_m,
        prepared.inst,
        prepared.di_a,
        prepared.di_b,
        prepared.di_c,
        false,
        prepared.reach_sq,
        prepared.terrain_start_cut_m,
        prepared.terrain_end_cut_m,
        horizon,
    )?;

    let cpa = CpaResult {
        q_m: kernel.q_m,
        d_p_m: kernel.d_p_m,
        lateral_m: kernel.lateral_m,
        relative_alt_m: kernel.rel_alt_m,
        beta_deg: kernel.beta_deg,
        seg_len_m: kernel.seg_len_m,
        t: kernel.t,
    };
    Some((kernel.sel, cpa))
}

/// Energy-only per-pixel call for the heatmap (which discards the CPA). The SEL
/// is bit-for-bit identical to `segment_sel_at_pixel().0` — `WANT_CPA = false`
/// only skips the CPA-only `beta_deg` atan (full path) and `lateral_m` sqrt
/// (fast path), neither of which feeds `sel`. Returns just the SEL and skips the
/// `CpaResult` build. `horizon`: all current heatmap callers pass `None`;
/// P3 wires the per-pixel horizon grid through here.
#[inline]
pub fn segment_sel_at_pixel_energy(
    prepared: &SegmentPrepared,
    row_state: &SegmentRowState,
    rx_lon: f64,
    rx_elev_m: f64,
    npd_luts: &NpdLuts,
    horizon: Option<&ReceiverHorizon>,
) -> Option<f64> {
    let ax = (prepared.start_lon - rx_lon) * row_state.m_per_deg_lon;
    let kernel = segment_energy_kernel::<false>(
        ax,
        row_state.ay,
        row_state.sdx,
        prepared.sdy,
        prepared.sdz,
        prepared.start_alt_m,
        row_state.inv_lsq,
        row_state.slen,
        rx_elev_m,
        npd_luts,
        prepared.class_idx,
        prepared.is_departure,
        prepared.dv,
        prepared.d_bar_m,
        prepared.inst,
        prepared.di_a,
        prepared.di_b,
        prepared.di_c,
        false,
        prepared.reach_sq,
        prepared.terrain_start_cut_m,
        prepared.terrain_end_cut_m,
        horizon,
    )?;
    Some(kernel.sel)
}

#[cfg(test)]
mod tests;
