//! Shared path effect computation for popup and pipeline.
//!
//! DEM, WorldCover forest, and IMD imperviousness are sampled by
//! [`RasterSampler::build_path_profile`] into one [`PathProfile`]. Exact vector
//! building and barrier crossings are evaluated separately against that profile.
//!
//! See [`super::path_profile`] for the canonical cadence and docs.

use super::diffraction;
use super::diffraction::DiffractionResult;
use super::horizon::single_edge_atten;
use super::iso9613::GroundPath;
use super::obstacle_index::{CrossingCandidate, ObstacleKind};
use super::path_profile::{
    clamp_source_platform, path_integral_u8, source_platform_clamped, vegetation_run_length,
    PathProfile,
};
use super::vegetation;
use crate::types::{EdgePoint, ObstacleEdge, ScreeningObstacleTrace, TerrainTrace, NUM_BANDS};

/// Terrain diffraction attenuation per band from a `PathProfile`.
///
/// `src_elev` and `rcv_alt` are absolute altitudes (metres above sea level)
/// of the source and receiver, including their respective heights above ground.
///
/// Applies a cheap in-profile "any sample above LoS" scan to short-circuit when
/// no obstruction exists (the previous sparse 3-point gate is gone — it shared
/// the bilateral cadence's blind zones near endpoints).
///
/// Takes `&mut PathProfile` so an internal f64 scratch buffer can be reused
/// across calls instead of allocating per path.
///
/// Fast path: no trace metadata built, no `Vec<EdgePoint>` allocation.
/// Pipeline-worker's hot loop uses this; popup uses `_with_meta`.
/// Shortest path (m) that carries a terrain or screening term: below it the
/// diffraction geometry has no samples to stand on and both passes return zero.
/// The CUDA lane mirrors the generated copy.
pub const SCREENING_MIN_PATH_M: f64 = 30.0;
/// Floor on the source height above bare earth (m) in the diffraction geometry.
pub const SOURCE_HEIGHT_FLOOR_M: f64 = 0.05;
/// Floor on the receiver height above bare earth (m) in the diffraction geometry.
pub const RECEIVER_HEIGHT_FLOOR_M: f64 = 0.5;

/// Bare-earth terrain diffraction bands; geometry is exposed by the meta variant.
pub fn terrain_attenuation(
    profile: &mut PathProfile,
    src_elev: f64,
    rcv_alt: f64,
) -> [f64; NUM_BANDS] {
    match compute_terrain_diffraction(profile, src_elev, rcv_alt) {
        None => [0.0; NUM_BANDS],
        Some(res) => res.bands,
    }
}

#[inline]
fn empty_terrain_trace() -> TerrainTrace {
    TerrainTrace {
        delta_m: 0.0,
        attenuation_bands: [0.0; NUM_BANDS],
        edges: Vec::new(),
        delta_star_m: 0.0,
    }
}

/// Shared intermediate between `terrain_attenuation` and `_with_meta`: the
/// precomputed bands + the single-edge `DiffractionResult`, with the f64
/// profile / `t` borrow the meta path indexes for the edge `EdgePoint`.
struct TerrainDiffraction<'a> {
    bands: [f64; NUM_BANDS],
    diff: DiffractionResult,
    /// Profile passed through as f64 — valid only for the duration of the
    /// caller's borrow of `profile.elevation_f64_scratch`.
    prof_f64: &'a [f64],
    t: &'a [f64],
    n: usize,
}

fn compute_terrain_diffraction<'a>(
    profile: &'a mut PathProfile,
    src_elev: f64,
    rcv_alt: f64,
) -> Option<TerrainDiffraction<'a>> {
    if profile.t.len() < 3 || profile.dist_m < SCREENING_MIN_PATH_M {
        return None;
    }
    let dz_total = rcv_alt - src_elev;
    let hill = profile
        .t
        .iter()
        .zip(profile.elevation_m.iter())
        .any(|(&t, &e)| (e as f64) > src_elev + dz_total * t);
    if !hill {
        return None;
    }

    // Absolute altitudes are what diffraction integrates against; per-end
    // heights above ground feed the mirror-fit δ* computation.
    let n = profile.t.len();
    let src_ground = profile.elevation_m[0] as f64;
    let src_h = (src_elev - src_ground).max(SOURCE_HEIGHT_FLOOR_M);
    let rcv_ground = profile.elevation_m[n - 1] as f64;
    let rcv_h = (rcv_alt - rcv_ground)
        .max(crate::constants::DEFAULT_RECEIVER_HEIGHT.min(RECEIVER_HEIGHT_FLOOR_M));
    let dist_m = profile.dist_m;

    let PathProfile {
        t,
        elevation_m,
        elevation_f64_scratch,
        ..
    } = profile;
    let prof_f64 = PathProfile::elevation_f64_from_mut(elevation_f64_scratch, elevation_m);
    // Source-platform clamp: the phantom near-source hump must not diffract
    // (SPEC §4.2). The scratch is shared with the screening pass below, so a
    // candidate's LERPed terrain inherits the same carved earth by construction.
    clamp_source_platform(t, prof_f64, dist_m);
    // Single-edge δ over bare-earth (was the multi-edge hull compute_path_difference).
    let (bands, diff) = single_edge_atten(t, prof_f64, prof_f64, dist_m, src_h, rcv_h);
    Some(TerrainDiffraction {
        bands,
        diff: diff?,
        prof_f64,
        t,
        n,
    })
}

/// Max-δ over a caller-supplied SUBSET of a ray's own cadence, plus the direct
/// slant distance — the sound inputs of the M3b byte-stop terrain bound
/// (`scatter_band`'s doc block option (b)).
///
/// `t`/`elevation_m` must be samples of the SAME ray the exact march would
/// walk (bit-identical elevation values at shared `t` — the caller samples
/// them through the same sampling path), with BOTH endpoints included so
/// `src_h`/`rcv_h` (and hence `dsr`) match the exact evaluation exactly. A
/// subset's max-δ edge can only be ≤ the full cadence's max-δ edge, so with
/// `dsr` the caller derives both δ lower bounds the sound mixed-band bound
/// needs ([`diffraction::diffraction_mixed_lower_bound`]).
///
/// Returns `None` when the subset shows no sample above the line of sight
/// (no terrain term to bound). Sound ONLY for a single-cp-ray exact path,
/// never under the angular quadrature (each bucket marches its own terrain).
pub fn terrain_subset_delta_lower_bound(
    t: &[f64],
    elevation_m: &[f32],
    dist_m: f64,
    src_elev: f64,
    rcv_alt: f64,
) -> Option<(f64, f64)> {
    let n = t.len();
    if n < 3 || dist_m < SCREENING_MIN_PATH_M || elevation_m.len() != n {
        return None;
    }
    let dz_total = rcv_alt - src_elev;
    let e0 = elevation_m[0] as f64;
    // Source-platform clamp, read-time form (SPEC §4.2): the exact march
    // carves the same samples, so a subset clamped by the same rule stays a
    // sound lower bound of the carved full march (subset-of-carved =
    // carved-of-subset — the rule is pointwise in (t, e) given shared e0).
    if !t.iter().zip(elevation_m.iter()).any(|(&ti, &e)| {
        source_platform_clamped(ti, dist_m, e as f64, e0) > src_elev + dz_total * ti
    }) {
        return None;
    }
    let src_h = (src_elev - e0).max(SOURCE_HEIGHT_FLOOR_M);
    let rcv_h = (rcv_alt - elevation_m[n - 1] as f64)
        .max(crate::constants::DEFAULT_RECEIVER_HEIGHT.min(RECEIVER_HEIGHT_FLOOR_M));
    let src_e = e0 + src_h;
    let rcv_e = elevation_m[n - 1] as f64 + rcv_h;
    let dsr = (dist_m * dist_m + (rcv_e - src_e).powi(2)).sqrt();
    let mut best = 0.0f64;
    let mut any = false;
    for i in 1..n - 1 {
        let top = source_platform_clamped(t[i], dist_m, elevation_m[i] as f64, e0);
        let los = src_e + (rcv_e - src_e) * t[i];
        if top <= los {
            continue;
        }
        let d_sg = t[i] * dist_m;
        let d_rg = (1.0 - t[i]) * dist_m;
        let delta = ((d_sg * d_sg + (top - src_e).powi(2)).sqrt()
            + (d_rg * d_rg + (top - rcv_e).powi(2)).sqrt())
            - dsr;
        if delta > best {
            best = delta;
            any = true;
        }
    }
    any.then_some((best, dsr))
}

/// Terrain attenuation + single-edge trace for popup tooltips.
///
/// Returns `(trace, profile_points)` where `trace` carries per-band attenuation,
/// the diffraction δ, the Rayleigh δ\*, and the single max-δ edge over bare earth
/// (`edges` is empty for a clear path; see `horizon::single_edge_atten`).
/// `profile_points` is the raw sample count the engine scanned — surfaced to
/// popup as transparency metadata.
pub fn terrain_attenuation_with_meta(
    profile: &mut PathProfile,
    src_elev: f64,
    rcv_alt: f64,
) -> (TerrainTrace, u32) {
    let Some(res) = compute_terrain_diffraction(profile, src_elev, rcv_alt) else {
        return (empty_terrain_trace(), 0);
    };
    let TerrainDiffraction {
        bands,
        diff,
        prof_f64,
        t,
        n,
    } = res;
    let idx = diff.edge_idx;

    let trace = TerrainTrace {
        delta_m: diff.delta,
        attenuation_bands: bands,
        edges: vec![EdgePoint {
            t: t[idx],
            elevation_m: prof_f64[idx],
        }],
        delta_star_m: diff.delta_star,
    };
    (trace, n as u32)
}

/// Building + barrier screening attenuation per band from a `PathProfile`.
///
/// Retains the strongest computed attenuation in each band across exact vector
/// crossings and bare terrain, returning only the increment over terrain.
///
/// `exclusion_radius_m`: ignore building crossings closer than this distance to
/// the source — the source polygon's own buildings are not real obstacles. Never
/// applied to barriers: an explicit wall is always a real obstacle.
pub fn screening_attenuation(
    profile: &mut PathProfile,
    obstacles: ObstacleInput<'_>,
    src_elev: f64,
    rcv_alt: f64,
    exclusion_radius_m: f64,
    terrain_atten: &[f64; NUM_BANDS],
) -> [f64; NUM_BANDS] {
    // Keep the tile-hot band-only path on its small return ABI instead of
    // entering the much larger metadata routine for the rural majority.
    if obstacles.candidates.is_empty() {
        return [0.0; NUM_BANDS];
    }
    screening_attenuation_with_meta(
        profile,
        obstacles,
        src_elev,
        rcv_alt,
        exclusion_radius_m,
        terrain_atten,
    )
    .0
}

/// Vector-obstacle input for screening: the exact ray×obstacle crossings.
///
/// Buildings and noise barriers arrive ONLY this way. There is no raster
/// building channel and no switch to one — a region whose vector obstacles are
/// missing fails at the loader, it does not quietly screen with something else.
#[derive(Clone, Copy)]
pub struct ObstacleInput<'a> {
    pub candidates: &'a [CrossingCandidate],
}

/// Screening attenuation + obstacle trace for popup tooltips.
///
/// Each crossing keeps its own Fresnel geometry and bare-earth Rayleigh fit.
/// The per-band maximum cannot lose a stronger screen when another edge's δ
/// overtakes it. This is the existing single-edge approximation's attenuation
/// envelope, not a multiple-diffraction construction. `terrain_atten` comes from
/// `terrain_attenuation[_with_meta]` on the same profile/source/receiver;
/// retaining it avoids recomputation and double-counting.
/// The singular trace identifies a real representative crossing with the
/// largest incremental attenuation in any band, not the whole envelope's cause.
///
/// The δ* Rayleigh gate uses **bare-earth** elevation for the OLS mean-ground
/// fit (CNOSSOS §2.5.6(c)). Feeding obstacle tops to OLS would drag the
/// mean-ground plane up to rooftops, silently breaking ground-reflection
/// physics.
pub fn screening_attenuation_with_meta(
    profile: &mut PathProfile,
    obstacles: ObstacleInput<'_>,
    src_elev: f64,
    rcv_alt: f64,
    exclusion_radius_m: f64,
    terrain_atten: &[f64; NUM_BANDS],
) -> ([f64; NUM_BANDS], ScreeningObstacleTrace) {
    let excl_limit = exclusion_radius_m.max(0.0);
    let dist_m = profile.dist_m;
    let n = profile.t.len();
    // Copy scalars before the later split-borrow of `profile` via destructure.
    let step_m_med = profile.step_m_med as f64;

    let make_empty = || ScreeningObstacleTrace {
        delta_m: 0.0,
        step_m: step_m_med,
        edge: None,
    };

    if n < 3 || dist_m < SCREENING_MIN_PATH_M {
        return ([0.0; NUM_BANDS], make_empty());
    }

    if obstacles.candidates.is_empty() {
        return ([0.0; NUM_BANDS], make_empty());
    }

    // 1. Bare-earth elevation as f64 (reuses amortized scratch buffer).
    //    Split-borrow pattern per terrain_attenuation_with_meta. No copy:
    //    we hold the scratch slice for the rest of the function.
    let PathProfile {
        t,
        elevation_m,
        elevation_f64_scratch,
        ..
    } = profile;
    let elevation_f64_mut = PathProfile::elevation_f64_from_mut(elevation_f64_scratch, elevation_m);
    // Same source-platform clamp as the terrain pass (idempotent when that pass
    // already ran on this profile — the shared scratch stays carved): a
    // candidate's terrain is LERPed from these samples, so a phantom hump the
    // terrain pass carved away must not re-enter under an obstacle (SPEC §4.2).
    clamp_source_platform(t, elevation_f64_mut, dist_m);
    let elevation_f64: &[f64] = elevation_f64_mut;

    // 2. Per-end heights above bare-earth for the diffraction API. Nothing is
    //    folded onto the sample profile any more: buildings and barriers alike
    //    retain their exact crossing geometry.
    let src_h = (src_elev - elevation_f64[0]).max(SOURCE_HEIGHT_FLOOR_M);
    let rcv_h = (rcv_alt - elevation_f64[n - 1]).max(RECEIVER_HEIGHT_FLOOR_M);

    let src_e = elevation_f64[0] + src_h;
    let rcv_e = elevation_f64[n - 1] + rcv_h;
    let dsr = (dist_m * dist_m + (rcv_e - src_e).powi(2)).sqrt();
    let mut atten_screen = [0.0_f64; NUM_BANDS];
    let mut representative: Option<(f64, DiffractionResult, CrossingCandidate, f64)> = None;
    for cand in obstacles.candidates.iter().copied() {
        if matches!(cand.kind, ObstacleKind::Building)
            && excl_limit > 0.0
            && cand.t * dist_m < excl_limit
        {
            continue; // source's own footprint — same gate as the sample path
        }
        let p = t.partition_point(|&x| x <= cand.t).clamp(1, n - 1);
        let (t0, t1) = (t[p - 1], t[p]);
        let frac = if t1 > t0 {
            (cand.t - t0) / (t1 - t0)
        } else {
            0.0
        };
        let terr = elevation_f64[p - 1] + frac * (elevation_f64[p] - elevation_f64[p - 1]);
        let top = terr + cand.height_m as f64;
        let cres = diffraction::compute_single_edge_at(
            t,
            elevation_f64,
            cand.t,
            top,
            dist_m,
            src_e,
            rcv_e,
            dsr,
            src_h,
            rcv_h,
        );
        let combined = diffraction::diffraction_attenuation_mixed(&cres);
        let mut largest_increment = 0.0_f64;
        for i in 0..NUM_BANDS {
            let increment = (combined[i] - terrain_atten[i]).max(0.0);
            atten_screen[i] = atten_screen[i].max(increment);
            largest_increment = largest_increment.max(increment);
        }
        if largest_increment > 0.0
            && representative
                .as_ref()
                .is_none_or(|(best, previous, _, _)| {
                    largest_increment > *best
                        || (largest_increment == *best && cres.delta > previous.delta)
                })
        {
            representative = Some((largest_increment, cres, cand, top));
        }
    }

    if let Some((_, cres, cand, top)) = representative {
        let kind: &'static str = match cand.kind {
            ObstacleKind::Building => "building",
            ObstacleKind::Barrier => "barrier",
        };
        let los_edge = src_e + (rcv_e - src_e) * cand.t;
        let trace = ScreeningObstacleTrace {
            delta_m: cres.delta,
            step_m: step_m_med,
            edge: Some(ObstacleEdge {
                kind,
                t: cand.t,
                height_m: cand.height_m as f64,
                screen_h_m: top - los_edge,
                obstacle_id: cand.id,
            }),
        };
        return (atten_screen, trace);
    }

    ([0.0; NUM_BANDS], make_empty())
}

/// Vegetation (forest) attenuation per band from a `PathProfile`.
///
/// Depth = `Σ Δlen × forest[i]/100` (right-endpoint density sampling) over
/// contiguous forested runs, keeping only runs whose PHYSICAL extent is
/// ≥ 10 m (`vegetation_run_length`). Non-uniform t spacing is weighted by
/// interval length so endpoints (dense) don't dominate — fixes the
/// pre-existing FusedGrid bias.
pub fn vegetation_attenuation_path(profile: &PathProfile) -> [f64; NUM_BANDS] {
    let forest_depth = vegetation_run_length(&profile.t, &profile.forest_u8, profile.dist_m);
    vegetation::vegetation_attenuation(forest_depth)
}

/// Path-averaged ground factor G (0 = hard, 1 = soft) from `profile.imd_u8[]`.
/// Trapezoidal weighting — endpoints not oversampled.
pub fn ground_g_from_profile(profile: &PathProfile) -> f64 {
    let avg_imd = path_integral_u8(&profile.t, &profile.imd_u8, profile.dist_m);
    (1.0 - avg_imd / 100.0).clamp(0.0, 1.0)
}

/// Direct CNOSSOS ground input for a sampled ray.
///
/// The OLS calculation is the same bare-earth regression primitive used by
/// diffraction's `δ*` construction.  It deliberately excludes the composite
/// building/barrier profile: those objects belong only to the screen arm of
/// the existing `max(A_ground, A_terrain + A_screen)` composite.  Bridges pass
/// `force_hard_ground=true`, preserving their explicit hard-surface rule.
pub fn cnossos_ground_path_from_profile(
    profile: &mut PathProfile,
    src_alt_m: f64,
    rcv_alt_m: f64,
    force_hard_ground: bool,
) -> GroundPath {
    if profile.t.is_empty() || profile.elevation_m.is_empty() {
        return GroundPath::new(0.0, 0.05, 0.5, 0.0, 0.0);
    }
    let ground_path_g = if force_hard_ground {
        0.0
    } else {
        ground_g_from_profile(profile)
    };
    let source_ground_g = if force_hard_ground {
        0.0
    } else {
        (1.0 - profile.imd_u8[0] as f64 / 100.0).clamp(0.0, 1.0)
    };
    let dist_m = profile.dist_m;
    let PathProfile {
        t,
        elevation_m,
        elevation_f64_scratch,
        ..
    } = profile;
    // FORCE-REFILL, never the amortized reuse: the terrain/screening passes
    // carve this scratch with the source-platform clamp (SPEC §4.2), and the
    // ground mean-plane must fit the RAW profile. Those passes re-apply their
    // clamp unconditionally after every refill, so any call order stays
    // correct (ground → terrain or terrain → ground).
    elevation_f64_scratch.clear();
    elevation_f64_scratch.extend(elevation_m.iter().map(|&e| e as f64));
    let elevation_m: &[f64] = elevation_f64_scratch;
    let (slope, intercept) = diffraction::fit_mean_ground_plane(t, elevation_m, 0.0, dist_m);
    let src_plane_m = intercept;
    let rcv_plane_m = slope * dist_m + intercept;
    GroundPath::new(
        dist_m,
        src_alt_m - src_plane_m,
        rcv_alt_m - rcv_plane_m,
        ground_path_g,
        source_ground_g,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleSet};
    use crate::propagation::path_profile::{fill_t_values, CELL_M};

    fn build_flat_profile(dist_m: f64, ground_elev_m: f32) -> PathProfile {
        let mut p = PathProfile::new();
        p.dist_m = dist_m;
        p.src_lat = 0.0;
        p.src_lon = 0.0;
        p.rcv_lat = 0.0;
        p.rcv_lon = dist_m / 111_320.0;
        fill_t_values(dist_m, &mut p.t);
        let n = p.t.len();
        p.elevation_m = vec![ground_elev_m; n];
        p.forest_u8 = vec![0; n];
        p.imd_u8 = vec![50; n];
        p.step_m_med = if n > 1 {
            ((p.t[1] - p.t[0]) * dist_m) as f32
        } else {
            0.0
        };
        p
    }

    fn screening_edge(trace: &ScreeningObstacleTrace) -> &ObstacleEdge {
        trace.edge.as_ref().expect("expected a screening obstacle")
    }

    #[test]
    fn flat_terrain_returns_zero_attenuation() {
        let mut p = build_flat_profile(1000.0, 10.0);
        let src_elev = 10.05;
        let rcv_alt = 11.5;
        let (trace, _) = terrain_attenuation_with_meta(&mut p, src_elev, rcv_alt);
        assert_eq!(trace.delta_m, 0.0, "flat profile should not diffract");
        assert!(trace.edges.is_empty());
        assert!(trace.attenuation_bands.iter().all(|&a| a == 0.0));
    }

    #[test]
    fn cnossos_ground_path_uses_bare_earth_ols_and_path_mean_g() {
        let mut p = build_flat_profile(1_000.0, 10.0);
        // Make the endpoint distinguishable from the path mean: trapezoidal
        // integration stays 0.5 in this symmetric profile while §2.5.14 sees
        // the hard source endpoint separately.
        p.imd_u8[0] = 100;
        let last = p.imd_u8.len() - 1;
        p.imd_u8[last] = 0;
        let got = cnossos_ground_path_from_profile(&mut p, 11.0, 14.0, false);
        assert!((got.dp_m - 1_000.0).abs() < 1e-9);
        assert!((got.zs_h_m - 1.0).abs() < 1e-9);
        assert!((got.zr_h_m - 4.0).abs() < 1e-9);
        assert!((got.source_ground_g - 0.0).abs() < 1e-9);
        assert!((got.ground_path_g - 0.5).abs() < 1e-9);

        let bridge = cnossos_ground_path_from_profile(&mut p, 11.0, 14.0, true);
        assert_eq!(bridge.ground_path_g, 0.0);
        assert_eq!(bridge.source_ground_g, 0.0);
    }

    #[test]
    fn hill_at_mid_path_ridge_catches() {
        // Narrow ridge at t=0.35 — old 3-probe at t=0.25/0.5/0.75 would miss it
        // (profile is flat at those t values); new scan catches it in the bilateral
        // cadence's middle samples.
        let mut p = build_flat_profile(1000.0, 10.0);
        // Insert a spike at the sample closest to t=0.35.
        let (spike_idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.35).abs()).partial_cmp(&((b - 0.35).abs())).unwrap())
            .unwrap();
        p.elevation_m[spike_idx] = 40.0;

        let src_elev = 10.05;
        let rcv_alt = 11.5;
        let (trace, _) = terrain_attenuation_with_meta(&mut p, src_elev, rcv_alt);
        assert!(
            trace.delta_m > 0.0,
            "ridge at t=0.35 must trigger diffraction"
        );
        assert_eq!(
            trace.edges.len(),
            1,
            "expected exactly the one dominant diffraction edge"
        );
    }

    /// The ground mean-plane must read the RAW profile even after the
    /// terrain pass carved the shared scratch (SPEC §4.2): ground result is
    /// identical whether or not terrain ran first, and repeats are stable.
    #[test]
    fn ground_path_is_blind_to_the_platform_clamp() {
        let build = || {
            let mut p = PathProfile::new();
            p.dist_m = 50.9;
            p.t = vec![0.0, 0.1963, 0.5, 0.8037, 1.0];
            p.elevation_m = vec![375.28, 375.80, 371.0, 369.0, 366.34];
            p.forest_u8 = vec![0; 5];
            p.imd_u8 = vec![50; 5];
            p
        };
        let mut fresh = build();
        let raw = cnossos_ground_path_from_profile(&mut fresh, 375.33, 370.34, false);
        let mut used = build();
        let _ = terrain_attenuation(&mut used, 375.33, 370.34);
        let after_terrain = cnossos_ground_path_from_profile(&mut used, 375.33, 370.34, false);
        let after_repeat = cnossos_ground_path_from_profile(&mut used, 375.33, 370.34, false);
        assert_eq!(raw.dp_m, after_terrain.dp_m);
        assert_eq!(raw.zs_h_m, after_terrain.zs_h_m, "source plane moved");
        assert_eq!(raw.zr_h_m, after_terrain.zr_h_m, "receiver plane moved");
        assert_eq!(raw.zs_h_m, after_repeat.zs_h_m, "repeat not stable");
    }

    /// The defect SPEC §4.2 prevents: a phantom shoulder hump one sample
    /// (~10 m) from the source on a downhill embankment path must NOT dominate
    /// the terrain term — after the platform clamp, only the genuine plateau
    /// edge (source cell's own elevation) may diffract. Geometry measured on
    /// the D4 at Voznice (owner report 2026-08-20): src cell 375.28, phantom
    /// 375.80 at 10 m, receiver 51 m downhill at 366.34.
    #[test]
    fn phantom_shoulder_hump_is_carved_to_the_platform() {
        let dist = 50.9;
        let mut p = PathProfile::new();
        p.dist_m = dist;
        p.t = vec![0.0, 0.1963, 0.5, 0.8037, 1.0];
        p.elevation_m = vec![375.28, 375.80, 371.0, 369.0, 366.34];
        p.forest_u8 = vec![0; 5];
        p.imd_u8 = vec![50; 5];
        let src_elev = 375.28 + 0.05;
        let rcv_alt = 366.34 + 4.0;
        let (trace, _) = terrain_attenuation_with_meta(&mut p, src_elev, rcv_alt);
        // The dominant edge still sits at 10 m — but at the CLAMPED platform
        // elevation, so δ is the plateau-edge 0.053 m, not the phantom 0.13 m
        // (values from the offline harness on the live DEM at this geometry).
        assert_eq!(trace.edges.len(), 1);
        assert!((trace.edges[0].t - 0.1963).abs() < 1e-9);
        assert!(
            (trace.edges[0].elevation_m - 375.28).abs() < 1e-3,
            "edge elevation must be the clamped platform, got {}",
            trace.edges[0].elevation_m
        );
        assert!(
            (trace.delta_m - 0.0533).abs() < 0.002,
            "plateau-edge δ, got {}",
            trace.delta_m
        );
        // High-band attenuation drops from the phantom's ~18 dB to ~14.4 dB;
        // low bands stay ~5 dB (the genuine shoulder graze is kept).
        assert!(
            (trace.attenuation_bands[7] - 14.39).abs() < 0.2,
            "8 kHz band, got {}",
            trace.attenuation_bands[7]
        );
    }

    /// A real cut slope rising WITHIN one cell of the source is also clamped —
    /// the price of the rule, pinned so the trade-off is explicit in review:
    /// shielding from intra-cell cut geometry is surrendered; slopes beyond
    /// one cell (the resolvable kind) screen exactly as before.
    #[test]
    fn cut_slope_beyond_one_cell_still_screens() {
        let dist = 200.0;
        let mut p = PathProfile::new();
        p.dist_m = dist;
        // Cut floor at 100, slope crest +6 m at 40 m (beyond CELL_M).
        p.t = vec![0.0, 0.05, 0.2, 0.5, 1.0];
        p.elevation_m = vec![100.0, 101.0, 106.0, 103.0, 100.0];
        p.forest_u8 = vec![0; 5];
        p.imd_u8 = vec![50; 5];
        let (trace, _) = terrain_attenuation_with_meta(&mut p, 100.05, 104.0);
        assert_eq!(trace.edges.len(), 1, "the crest must remain the edge");
        assert!(
            (trace.edges[0].elevation_m - 106.0).abs() < 1e-9,
            "beyond-cell crest is untouched, got {}",
            trace.edges[0].elevation_m
        );
        assert!(trace.delta_m > 0.0);
    }

    /// Landing-review adjudication (2026-08-21, Grok WARNING vs Qwen F3): on
    /// the annulus 30 <= dist < CELL_M the receiver sample (t=1) sits INSIDE
    /// the clamp zone, so the exact march carves an uphill receiver's ground
    /// to e0 while its `rcv_h` was derived from the RAW sample — the rebuilt
    /// `rcv_elev = carved[n-1] + rcv_h` then sits below `rcv_alt`, unlike
    /// screening (heights post-carve) and the CUDA rebuild, which recover
    /// `rcv_alt`. The value-level fork is real but cannot reach an output:
    /// dist < CELL_M puts EVERY sample in-zone, the carve flattens the whole
    /// profile to e0 (never raises), and both chord variants stay above that
    /// bed (src_e >= e0+0.05; the march's frozen rcv_e >= e0+0.5 via the
    /// height floor; screening's rcv_e = rcv_alt > e0) — no edge, no terrain
    /// term, march == screening == CUDA on the annulus. Pin the agreement at
    /// the worst case (deepest carve + the 0.5 m floor): any reorder that
    /// lets the frozen `rcv_h` manufacture or kill an edge here must fail.
    #[test]
    fn annulus_receiver_carve_cannot_fork_the_terrain_term() {
        // 30.5 m ray: terrain runs (>= 30 m) and the whole ray is in-zone.
        let dist = 30.5;
        assert!((30.0..CELL_M).contains(&dist), "the annulus this test pins");
        let t = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0];
        // Phantom mid humps + an UPHILL receiver 12 m above the source cell —
        // the deepest carve the zone can produce on this ray.
        let raw: [f32; 4] = [100.0, 108.0, 106.0, 112.0];
        let src_elev = 100.05;
        let rcv_alt = 112.5; // receiver exactly at the 0.5 m height floor

        // As shipped: raw `rcv_h` frozen, then the carve (the march's order).
        let mut p = PathProfile::new();
        p.dist_m = dist;
        p.t = t.to_vec();
        p.elevation_m = raw.to_vec();
        p.forest_u8 = vec![0; 4];
        p.imd_u8 = vec![50; 4];
        let march = terrain_attenuation(&mut p, src_elev, rcv_alt);
        assert_eq!(
            march, [0.0; NUM_BANDS],
            "carved-flat annulus must be silent"
        );

        // Both height orders by hand over the same carved bed:
        let mut carved: Vec<f64> = raw.iter().map(|&e| e as f64).collect();
        clamp_source_platform(&t, &mut carved, dist);
        assert!(
            carved.iter().all(|&e| e == 100.0),
            "dist < CELL_M flattens the whole ray, got {carved:?}"
        );
        let rcv_h_frozen = (rcv_alt - raw[3] as f64).max(0.5); // march order
        let rcv_h_carved = (rcv_alt - carved[3]).max(0.5); // screening/CUDA order
        assert!(rcv_h_frozen < rcv_h_carved, "the fork under test");
        let (bands_frozen, diff_frozen) =
            single_edge_atten(&t, &carved, &carved, dist, 0.05, rcv_h_frozen);
        let (bands_carved, diff_carved) =
            single_edge_atten(&t, &carved, &carved, dist, 0.05, rcv_h_carved);
        assert_eq!(
            bands_frozen, bands_carved,
            "the annulus fork is unobservable in the bands"
        );
        assert!(diff_frozen.is_none() && diff_carved.is_none());
        assert_eq!(bands_frozen, march);
    }

    #[test]
    fn endpoint_near_cliff_catches() {
        // Cliff at t≈0.03 (right next to receiver) — old 3-probe starts at t=0.25
        // so it totally misses. Bilateral cadence has a sample at t≈0.03 at 1 km path
        // (30m/1000m = 0.03).
        let mut p = build_flat_profile(1000.0, 10.0);
        let (cliff_idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.03).abs()).partial_cmp(&((b - 0.03).abs())).unwrap())
            .unwrap();
        p.elevation_m[cliff_idx] = 40.0;

        let src_elev = 10.05;
        let rcv_alt = 11.5;
        let (trace, _) = terrain_attenuation_with_meta(&mut p, src_elev, rcv_alt);
        assert!(trace.delta_m > 0.0, "cliff at t=0.03 must be caught");
    }

    #[test]
    fn terrace_before_source_catches() {
        // Rise at t≈0.97 — old 3-probe ends at t=0.75 so it misses. Bilateral has a
        // sample at t≈0.97 thanks to the receiver-side densification.
        let mut p = build_flat_profile(1000.0, 10.0);
        let (spike_idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.97).abs()).partial_cmp(&((b - 0.97).abs())).unwrap())
            .unwrap();
        p.elevation_m[spike_idx] = 40.0;

        let src_elev = 10.05;
        let rcv_alt = 11.5;
        let (trace, _) = terrain_attenuation_with_meta(&mut p, src_elev, rcv_alt);
        assert!(trace.delta_m > 0.0, "terrace at t=0.97 must be caught");
    }

    /// THE M3b soundness property, on terrain the full cadence actually
    /// diffracts: the K-sample subset bound — max-δ over the subset, then the
    /// provable mixed-band bound at (δ_sub, δ_sub − κ) with κ the favourable
    /// arc correction of the direct ray — never exceeds the full profile's
    /// exact `terrain_attenuation` bands. Sweeps hill positions, heights and
    /// distances, including hills INSIDE the clamp zone (landing-review
    /// hardening 2026-08-21): march/bound clamp agreement is pinned where the
    /// carve actually removes the hill, not just structurally guaranteed.
    #[test]
    fn terrain_subset_bound_never_exceeds_the_full_cadence_bands() {
        use super::super::diffraction::diffraction_mixed_lower_bound;
        use crate::constants::{FAV_RAY_CURVATURE_MIN_M, FAV_RAY_CURVATURE_PER_DSR};
        let src_elev = 10.05;
        let rcv_alt = 11.5;
        // One case: plant `hill_h` on the sample nearest `hill_t` (never the
        // source cell itself — that would move the shared e0), run the full
        // cadence and the K-sample subset bound, assert bound <= full per
        // band. Returns the hill's distance so callers can assert zone side.
        let check = |dist: f64, hill_t: f64, hill_h: f32| -> f64 {
            let mut p = build_flat_profile(dist, 10.0);
            let (idx, _) =
                p.t.iter()
                    .enumerate()
                    .skip(1)
                    .min_by(|(_, &a), (_, &b)| {
                        ((a - hill_t).abs())
                            .partial_cmp(&((b - hill_t).abs()))
                            .unwrap()
                    })
                    .unwrap();
            p.elevation_m[idx] = hill_h;
            let hill_d = p.t[idx] * dist;
            let full = terrain_attenuation(&mut p, src_elev, rcv_alt);
            let n = p.t.len();
            let k = 8usize;
            let subset: Vec<usize> = (0..k)
                .map(|j| ((j as f64) * (n - 1) as f64 / (k - 1) as f64).round() as usize)
                .collect();
            let t_sub: Vec<f64> = subset.iter().map(|&i| p.t[i]).collect();
            let e_sub: Vec<f32> = subset.iter().map(|&i| p.elevation_m[i]).collect();
            // No hill in the subset ⇒ no bound ⇒ sound.
            if let Some((delta_sub, dsr)) =
                terrain_subset_delta_lower_bound(&t_sub, &e_sub, dist, src_elev, rcv_alt)
            {
                let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
                let kappa = 2.0 * gamma * (dsr / (2.0 * gamma)).asin() - dsr;
                let bound = diffraction_mixed_lower_bound(delta_sub, delta_sub - kappa);
                for b in 0..NUM_BANDS {
                    assert!(
                        bound[b] <= full[b] + 1e-9,
                        "d={dist} hill_t={hill_t} hill_h={hill_h} band {b}: bound {:.6} > full {:.6}",
                        bound[b],
                        full[b]
                    );
                }
            }
            hill_d
        };
        for &dist in &[600.0, 1_000.0, 3_000.0] {
            for &hill_t in &[0.2, 0.35, 0.5, 0.65, 0.8] {
                for &hill_h in &[14.0, 20.0, 30.0, 45.0, 70.0] {
                    check(dist, hill_t, hill_h);
                }
            }
        }
        // Near-source arm: at 600 m the 10 m near-endpoint probe sits INSIDE
        // the carve zone (hill_d < CELL_M) — the clamp removes the hill in
        // both the full march (in-place) and the bound (read-time), and the
        // bound property must still hold.
        for &hill_h in &[14.0, 20.0, 30.0, 45.0, 70.0] {
            let hill_d = check(600.0, 0.02, hill_h);
            assert!(
                hill_d < CELL_M,
                "near-source arm must land inside the carve zone, got {hill_d} m"
            );
        }
    }

    /// The subset march degenerates exactly like the exact path: under 3
    /// samples or under 30 m there is no terrain term to bound, and a flat
    /// subset yields no edge above the line of sight.
    #[test]
    fn terrain_subset_march_degenerate_shapes() {
        assert!(terrain_subset_delta_lower_bound(
            &[0.0, 0.5, 1.0],
            &[10.0, 30.0, 10.0],
            25.0,
            12.0,
            14.0
        )
        .is_none());
        assert!(
            terrain_subset_delta_lower_bound(&[0.0, 1.0], &[10.0, 10.0], 100.0, 12.0, 14.0)
                .is_none()
        );
        // Flat subset: no edge above LOS ⇒ no bound.
        assert!(terrain_subset_delta_lower_bound(
            &[0.0, 0.25, 0.5, 0.75, 1.0],
            &[10.0; 5],
            1000.0,
            12.0,
            14.0
        )
        .is_none());
    }

    #[test]
    fn screening_finds_midpath_building() {
        // Tall building at t=0.4 — should produce screening attenuation.
        let mut p = build_flat_profile(1000.0, 0.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.4).abs()).partial_cmp(&((b - 0.4).abs())).unwrap())
            .unwrap();
        let cands = [CrossingCandidate {
            t: p.t[idx],
            height_m: 20.0,
            kind: ObstacleKind::Building,
            id: 1,
        }];
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.01,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(screening_edge(&trace).kind, "building");
        assert_eq!(screening_edge(&trace).height_m, 20.0);
        assert!(
            atten.iter().any(|&a| a > 0.0),
            "building at t=0.4 should produce screening"
        );
    }

    /// Scene metres (east, north of the flat profile's source at lat/lon 0)
    /// → (lat, lon). `build_flat_profile` puts the receiver due east at
    /// `dist_m / M_PER_DEG_LON_EQ`, so the path is the x axis and a barrier's
    /// crossing chainage is just `x / dist_m`.
    fn barrier_ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            y_m / grid::geo::M_PER_DEG_LAT,
            x_m / crate::constants::M_PER_DEG_LON_EQ,
        )
    }

    /// A wall through the production route: an `ObstacleKind::Barrier`
    /// polyline in a small `ObstacleIndex`, its crossings of the test ray
    /// collected through `ObstacleSet::crossings` exactly as the kernels'
    /// `obstacle_input_for_ray` collects them. `id` is the loader's dense
    /// ordinal — the index's obstacle identity (the deleted barrier slice
    /// carried the OSM way id instead).
    fn wall_crossings(
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        height_m: f32,
        path_len_m: f64,
        id: u32,
    ) -> Vec<CrossingCandidate> {
        let mut builder = ObstacleIndex::builder(0.0, 0.0);
        builder.add_polyline(
            &[barrier_ll(x0, y0), barrier_ll(x1, y1)],
            height_m,
            ObstacleKind::Barrier,
            id,
        );
        let set = ObstacleSet {
            indexes: vec![std::sync::Arc::new(builder.build())],
        };
        let mut cands = Vec::new();
        set.crossings(
            0.0,
            0.0,
            0.0,
            path_len_m / crate::constants::M_PER_DEG_LON_EQ,
            &mut cands,
        );
        cands
    }

    /// Mid-path 3 m barrier on a flat profile must screen, and the band-only
    /// wrapper must agree with `_with_meta` (the heatmap kernels call the
    /// wrapper; popup calls `_with_meta` — parity by construction).
    #[test]
    fn screening_finds_midpath_barrier() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 60 m of wall straddling the path at t = 0.5.
        let cands = wall_crossings(100.0, -30.0, 100.0, 30.0, 3.0, dist_m, 1);
        assert_eq!(cands.len(), 1, "the ray crosses the wall exactly once");
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(screening_edge(&trace).kind, "barrier");
        assert!(
            atten.iter().any(|&a| a > 0.0),
            "3 m wall above the 0.05→1.5 m LOS must screen"
        );
        let mut p2 = build_flat_profile(dist_m, 0.0);
        let bands = screening_attenuation(
            &mut p2,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(bands, atten, "band-only wrapper == _with_meta bands");
    }

    /// Early-out refinement: a wall the ray cannot touch yields NO crossings
    /// from the index walk, and with an empty candidate list both screening
    /// entry points return exactly the empty-input result — this keeps the
    /// rural fast path alive for heatmaps and traced popup fan rays alike.
    /// (The sorted-slice `dist_m` horizon this test used to pin is gone with
    /// the slice; the index answers the same question geometrically.)
    #[test]
    fn far_barrier_never_reaches_the_candidate_list() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 60 m of wall 500 m north of the path — the ray never comes near it.
        let cands = wall_crossings(0.0, 500.0, 60.0, 500.0, 3.0, dist_m, 1);
        assert!(cands.is_empty(), "off-path wall must produce no crossing");
        let mut p = build_flat_profile(dist_m, 0.0);
        let bands = screening_attenuation(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        let mut p2 = build_flat_profile(dist_m, 0.0);
        let empty = screening_attenuation(
            &mut p2,
            ObstacleInput { candidates: &[] },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(bands, empty);
        assert!(bands.iter().all(|&a| a == 0.0));

        let mut p3 = build_flat_profile(dist_m, 0.0);
        let (traced, trace) = screening_attenuation_with_meta(
            &mut p3,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(traced, empty);
        assert!(trace.edge.is_none());
    }

    /// FIX 3, the miss: a long wall crossing the path 100 m away from its own
    /// MIDPOINT. The ±50 m midpoint-proximity heuristic skipped it outright
    /// (perp(midpoint) = 100 m); the exact ray×segment intersection finds the
    /// crossing and screens it.
    #[test]
    fn crossing_far_from_the_wall_midpoint_screens() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 243 m wall from (100, −20) to (140, 220): crosses the path at
        // x = 103.33 (t = 0.5167), midpoint (120, 100) — 100 m off the path.
        let cands = wall_crossings(100.0, -20.0, 140.0, 220.0, 3.0, dist_m, 7);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(screening_edge(&trace).kind, "barrier");
        assert_eq!(
            screening_edge(&trace).obstacle_id,
            7,
            "the index's dense ordinal, not the OSM way id"
        );
        assert!(
            (screening_edge(&trace).t - 103.3333 / dist_m).abs() < 1e-6,
            "t = {}",
            screening_edge(&trace).t
        );
        assert!(
            atten.iter().any(|&a| a > 0.0),
            "the wall the ray actually crosses must screen"
        );
    }

    /// FIX 3, the false positive: a wall running PARALLEL to the path 10 m to
    /// the side, its midpoint projecting to mid-path. The ±50 m heuristic
    /// screened it (perp = 10 m); nothing crosses, so the exact test must
    /// report no obstacle at all.
    #[test]
    fn near_midpoint_pass_without_crossing_does_not_screen() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let cands = wall_crossings(80.0, 10.0, 120.0, 10.0, 3.0, dist_m, 1);
        assert!(cands.is_empty(), "a parallel wall never crosses the ray");
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert!(
            trace.edge.is_none(),
            "a wall the path passes BY is not a screen"
        );
        assert!(atten.iter().all(|&a| a == 0.0), "{atten:?}");
    }

    /// A long wall crossing the path near the SOURCE screens it, wherever its
    /// midpoint lies. The deleted slice channel carried a `dist_m`-sorted early
    /// break (`BARRIER_PATH_HORIZON_M` — path length plus a wall half-length)
    /// precisely so a wall like this one still reached the scan; the index has
    /// no midpoint heuristic at all, so the exact crossing is found by
    /// construction.
    #[test]
    fn long_wall_crossing_near_the_source_screens() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 240 m wall (5, 0.5) → (−235, −60): crosses the path at x ≈ 3.0,
        // midpoint (−115, −29.75) ⇒ 316 m from the receiver at (200, 0).
        let cands = wall_crossings(5.0, 0.5, -235.0, -60.0, 3.0, dist_m, 1);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(screening_edge(&trace).kind, "barrier");
        assert!(atten.iter().any(|&a| a > 0.0), "{atten:?}");
    }

    /// Both obstacle kinds use the same attenuation rule and trace selection.
    #[test]
    fn barrier_and_building_share_screening_rule() {
        let dist_m = 400.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let mut p = build_flat_profile(dist_m, 0.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        // 4 m building at mid-path against a 4 m wall at t = 0.95 — the wall is
        // nearer the receiver and supplies the representative edge. One
        // candidate list, both kinds — production's index walk returns them the
        // same way.
        let building = CrossingCandidate {
            t: p.t[idx],
            height_m: 4.0,
            kind: ObstacleKind::Building,
            id: 1,
        };
        let wall = wall_crossings(380.0, -20.0, 380.0, 20.0, 4.0, dist_m, 2);
        let cands: Vec<CrossingCandidate> = [building].into_iter().chain(wall).collect();
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(screening_edge(&trace).kind, "barrier");
        assert!(
            (screening_edge(&trace).t - 0.95).abs() < 1e-9,
            "t = {}",
            screening_edge(&trace).t
        );
        assert!(atten.iter().any(|&a| a > 0.0));
    }

    /// A competing edge may win different bands, never replace a stronger one.
    #[test]
    fn screening_envelope_preserves_each_band_and_real_trace() {
        for hill_height in [0.0, 20.0] {
            let mut p = build_flat_profile(100.0, 0.0);
            let middle = p.t.iter().position(|&t| t >= 0.5).unwrap();
            p.elevation_m[middle] = hill_height;
            let terrain = terrain_attenuation(&mut p, 0.05, 10.0);
            assert_eq!(terrain.iter().any(|&a| a > 0.0), hill_height > 0.0);
            let roof = CrossingCandidate {
                t: 0.1,
                height_m: 3.0,
                kind: ObstacleKind::Building,
                id: 1,
            };
            let mut previous = [0.0; NUM_BANDS];
            let mut mixed_bands = false;
            for step in 0..=900 {
                let wall = CrossingCandidate {
                    t: 0.25,
                    height_m: 2.0 + step as f32 * 0.01,
                    kind: ObstacleKind::Barrier,
                    id: 2,
                };
                let mut evaluate = |candidates: &[CrossingCandidate]| {
                    screening_attenuation_with_meta(
                        &mut p,
                        ObstacleInput { candidates },
                        0.05,
                        10.0,
                        0.0,
                        &terrain,
                    )
                };
                let singles = [evaluate(&[roof]).0, evaluate(&[wall]).0];
                let (bands, trace) = evaluate(&[roof, wall]);
                assert_eq!(
                    bands,
                    evaluate(&[wall, roof]).0,
                    "order must not move bands"
                );
                for band in 0..NUM_BANDS {
                    assert_eq!(bands[band], singles[0][band].max(singles[1][band]));
                    assert!(
                        bands[band] + 1e-10 >= previous[band],
                        "height decreased band {band}"
                    );
                }
                mixed_bands |= singles[0].iter().zip(singles[1]).any(|(&a, b)| a > b)
                    && singles[0].iter().zip(singles[1]).any(|(&a, b)| a < b);
                if let Some(edge) = trace.edge {
                    let candidate = if edge.obstacle_id == roof.id {
                        roof
                    } else {
                        wall
                    };
                    assert_eq!(edge.t, candidate.t);
                    assert_eq!(edge.height_m, candidate.height_m as f64);
                    let single = evaluate(&[candidate]);
                    assert_eq!(trace.delta_m, single.1.delta_m);
                    assert_eq!(
                        single.0.iter().copied().fold(0.0, f64::max),
                        bands.iter().copied().fold(0.0, f64::max)
                    );
                } else {
                    assert_eq!(bands, [0.0; NUM_BANDS]);
                }
                previous = bands;
            }
            if hill_height == 0.0 {
                assert!(
                    mixed_bands,
                    "fixture must require more than one band's winning edge"
                );
            }
        }
    }

    #[test]
    fn naked_hill_is_not_a_screening_obstacle() {
        // A bare-earth hill at t=0.5 with no buildings: terrain_attenuation owns
        // the diffraction. Screening must report "none" (the composite top equals
        // the DEM, so the increment over terrain is zero — a terrain hill is not a
        // building/barrier screening obstacle).
        let mut p = build_flat_profile(1500.0, 10.0);
        let (spike, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p.elevation_m[spike] = 40.0;
        let (terrain_trace, _) = terrain_attenuation_with_meta(&mut p, 10.05, 11.5);
        let (atten, screening_trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &[] },
            10.05,
            11.5,
            0.0,
            &terrain_trace.attenuation_bands,
        );
        assert!(
            screening_trace.edge.is_none(),
            "bare hill is not a screening obstacle"
        );
        assert!(
            atten.iter().all(|&a| a == 0.0),
            "no screening increment over terrain"
        );
    }

    /// NO DOUBLE-COUNT at the production level: hill + building at the SAME
    /// sample — `terrain + screen` must equal `max(terrain, combined)` per
    /// band, where `combined` is independently recomputed over the composite
    /// profile with this module's exact height floors. Guards the increment
    /// contract (`screen = (combined − terrain).max(0)`); ported from the
    /// deleted `solve_single_edge` harness (geodata-v2 D11).
    #[test]
    fn hill_plus_building_does_not_double_count() {
        let mut p = build_flat_profile(500.0, 100.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p.elevation_m[idx] = 110.0; // 10 m hill
        let src_elev = 100.05;
        let rcv_alt = 104.0;
        // 6 m building standing ON the hill, as the exact crossing it now is.
        let cands = [CrossingCandidate {
            t: p.t[idx],
            height_m: 6.0,
            kind: ObstacleKind::Building,
            id: 1,
        }];

        let terrain = terrain_attenuation(&mut p, src_elev, rcv_alt);
        let (screen, _) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            src_elev,
            rcv_alt,
            0.0,
            &terrain,
        );
        assert!(terrain.iter().any(|&a| a > 0.0), "hill must attenuate");
        assert!(
            screen.iter().any(|&a| a > 0.0),
            "building must add screening"
        );

        // Independent combined pass over the composite, same height floors.
        let n = p.t.len();
        let bare: Vec<f64> = p.elevation_m.iter().map(|&e| e as f64).collect();
        let mut composite = bare.clone();
        composite[idx] += 6.0;
        let src_h = (src_elev - bare[0]).max(0.05);
        let rcv_h = (rcv_alt - bare[n - 1]).max(0.5);
        let (combined, _) = single_edge_atten(&p.t, &composite, &bare, 500.0, src_h, rcv_h);

        #[allow(clippy::needless_range_loop)]
        for i in 0..NUM_BANDS {
            assert!(
                (terrain[i] + screen[i] - terrain[i].max(combined[i])).abs() < 1e-9,
                "band {i}: terrain+screen {:.4} != max(terrain, combined) {:.4}",
                terrain[i] + screen[i],
                terrain[i].max(combined[i])
            );
        }
    }

    /// Exact vector crossings also screen between the terrain cadence samples.
    #[test]
    fn candidate_between_samples_adds_screening() {
        let mut p = build_flat_profile(500.0, 100.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p.elevation_m[idx] = 104.0; // modest mid-path hill, ON a sample
                                    // Tall candidate near the receiver, deliberately between samples.
        let t_c = (p.t[p.t.len() - 2] + p.t[p.t.len() - 1]) / 2.0;
        let cands = [CrossingCandidate {
            t: t_c,
            height_m: 14.0,
            kind: ObstacleKind::Building,
            id: 7,
        }];
        let terrain = terrain_attenuation(&mut p, 100.05, 104.0);
        assert!(terrain.iter().any(|&a| a > 0.0), "the hill must attenuate");
        let (_, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            100.05,
            104.0,
            0.0,
            &terrain,
        );
        assert_eq!(screening_edge(&trace).kind, "building");
        assert!(
            (screening_edge(&trace).t - t_c).abs() < 1e-12,
            "candidate edge must win: trace.t {} vs t_c {}",
            screening_edge(&trace).t,
            t_c
        );
    }

    /// The exclusion radius gates Building candidates (source's own
    /// footprint) but never Barrier candidates — same asymmetry as the
    /// sample path.
    #[test]
    fn exclusion_gates_building_candidates_not_barriers() {
        let terrain = [0.0_f64; NUM_BANDS];
        for (kind, expect_screen) in [
            (ObstacleKind::Building, false),
            (ObstacleKind::Barrier, true),
        ] {
            let mut p = build_flat_profile(500.0, 100.0);
            let cands = [CrossingCandidate {
                t: 0.05, // 25 m from the source
                height_m: 10.0,
                kind,
                id: 1,
            }];
            let (atten, _) = screening_attenuation_with_meta(
                &mut p,
                ObstacleInput { candidates: &cands },
                100.05,
                104.0,
                60.0, // exclusion covers the candidate
                &terrain,
            );
            let screened = atten.iter().any(|&a| a > 0.0);
            assert_eq!(
                screened, expect_screen,
                "kind {kind:?}: exclusion must gate buildings only"
            );
        }
    }

    /// A candidate must still screen on a clear path — the envelope
    /// cannot early-return just because bare terrain has no edge.
    #[test]
    fn candidate_screens_on_otherwise_clear_path() {
        let mut p = build_flat_profile(400.0, 50.0);
        let cands = [CrossingCandidate {
            t: 0.7,
            height_m: 9.0,
            kind: ObstacleKind::Building,
            id: 3,
        }];
        let terrain = [0.0_f64; NUM_BANDS];
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            50.05,
            51.5,
            0.0,
            &terrain,
        );
        assert!(atten.iter().any(|&a| a > 0.0), "candidate must screen");
        assert_eq!(screening_edge(&trace).kind, "building");
        assert!((screening_edge(&trace).t - 0.7).abs() < 1e-12);
    }

    /// δ* continuity: two candidates straddling a cadence sample by ±ε yield
    /// nearly identical bands — no discontinuity as t_e crosses a sample.
    #[test]
    fn candidate_bands_continuous_across_sample() {
        let terrain = [0.0_f64; NUM_BANDS];
        let mut bands = Vec::new();
        for eps in [-1e-6, 1e-6] {
            let mut p = build_flat_profile(500.0, 100.0);
            let n = p.t.len();
            for i in 0..n {
                p.elevation_m[i] = 100.0 + (8.0 * p.t[i]) as f32;
            }
            let t_s = p.t[n / 2] + eps;
            let cands = [CrossingCandidate {
                t: t_s,
                height_m: 12.0,
                kind: ObstacleKind::Building,
                id: 1,
            }];
            let (a, _) = screening_attenuation_with_meta(
                &mut p,
                ObstacleInput { candidates: &cands },
                100.05,
                112.0,
                0.0,
                &terrain,
            );
            bands.push(a);
        }
        #[allow(clippy::needless_range_loop)]
        for i in 0..NUM_BANDS {
            assert!(
                (bands[0][i] - bands[1][i]).abs() < 1e-3,
                "band {i} jumps across the sample: {} vs {}",
                bands[0][i],
                bands[1][i]
            );
        }
    }

    /// Penumbra (fix-pack Fix 2): a wall that misses the sight line by
    /// centimetres still attenuates the short wavelengths — CNOSSOS §2.5.6(c)
    /// computes diffraction down to δ = −λ/20 — while the Rayleigh δ\* gate
    /// keeps the long ones out. Before this branch the same wall dropped from
    /// ~4.8 dB to exactly zero as it sank 1 mm below the line of sight.
    ///
    /// THE EXPECTED VALUE MOVED, AND THE OLD ONE WAS PINNING A DEFECT. This
    /// test used to ask only for `> 3 dB`, which recorded 4.77 dB — the
    /// HOMOGENEOUS value, reached because the favourable arm was computed on
    /// STRAIGHT rays below the sight line and so returned the same ≈0 path
    /// difference. CNOSSOS-EU (2.5.27) puts that arm on its arc like every
    /// other branch: δ_F = −0.098 m on this geometry, past −λ/20 in every band,
    /// so the favourable half of the `P_FAV` mix contributes nothing and the
    /// mixed value is 1.76 dB. 4.77 was exactly the top of the step that made a
    /// TALLER screen come out LOUDER
    /// (`arc_screening::taller_screen_never_makes_the_receiver_louder`, 47/108
    /// wall geometries) — this assertion was holding it in place. Recomputed
    /// against the standard, the same way the hard-ground −3 dB was.
    #[test]
    fn near_miss_candidate_screens_short_wavelengths_only() {
        // 200 m flat path, source 0.05 m, receiver 4 m → LOS at t=0.5 is
        // 2.025 m; a 2 m wall there sits 25 mm below it.
        let mut p = build_flat_profile(200.0, 0.0);
        let cands = [CrossingCandidate {
            t: 0.5,
            height_m: 2.0,
            kind: ObstacleKind::Building,
            id: 1,
        }];
        let terrain = [0.0_f64; NUM_BANDS];
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            4.0,
            0.0,
            &terrain,
        );
        // 8 kHz (λ = 4 cm) and 4 kHz diffract around a 2.5 cm clearance; from
        // 2 kHz down the δ* mean-ground gate (δ* = 4.1 cm here) owns the band.
        assert!(
            (atten[7] - 1.760).abs() < 0.01,
            "8 kHz near miss must mix to 1.76 dB: {atten:?}"
        );
        assert!((atten[6] - 1.759).abs() < 0.01, "4 kHz likewise: {atten:?}");
        for (i, &a) in atten.iter().enumerate().take(6) {
            assert_eq!(a, 0.0, "band {i} stays gated by δ*: {atten:?}");
        }
        assert_eq!(screening_edge(&trace).kind, "building");
        assert!(trace.delta_m < 0.0, "near miss carries a negative δ");
    }

    /// …and an obstacle well below the sight line stays silent in every band,
    /// with no obstacle reported: the penumbra must not turn every low fence
    /// into a barrier.
    #[test]
    fn far_below_candidate_stays_silent() {
        let mut p = build_flat_profile(200.0, 0.0);
        let cands = [CrossingCandidate {
            t: 0.5,
            height_m: 0.5,
            kind: ObstacleKind::Building,
            id: 1,
        }];
        let terrain = [0.0_f64; NUM_BANDS];
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            4.0,
            0.0,
            &terrain,
        );
        assert!(atten.iter().all(|&a| a == 0.0), "{atten:?}");
        assert!(trace.edge.is_none(), "nothing to report to the popup");
    }

    /// A real blocker beats a near miss: signed-δ ranking must not let a
    /// below-LOS candidate hide the wall that actually breaks the path.
    #[test]
    fn blocking_candidate_beats_near_miss() {
        let mut p = build_flat_profile(200.0, 0.0);
        let cands = [
            CrossingCandidate {
                t: 0.5,
                height_m: 2.0, // near miss
                kind: ObstacleKind::Building,
                id: 1,
            },
            CrossingCandidate {
                t: 0.7,
                height_m: 6.0, // real blocker
                kind: ObstacleKind::Building,
                id: 2,
            },
        ];
        let terrain = [0.0_f64; NUM_BANDS];
        let (_, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            0.05,
            4.0,
            0.0,
            &terrain,
        );
        assert_eq!(screening_edge(&trace).obstacle_id, 2);
        assert!(trace.delta_m > 0.0);
    }

    /// A strong near-receiver edge represents the result among weaker crossings.
    #[test]
    fn near_receiver_edge_represents_strong_screening() {
        let mut p = build_flat_profile(500.0, 100.0);
        let cands = [
            CrossingCandidate {
                t: 0.5,
                height_m: 6.0,
                kind: ObstacleKind::Building,
                id: 1,
            },
            CrossingCandidate {
                t: 0.9, // near receiver → larger δ at same height class
                height_m: 6.0,
                kind: ObstacleKind::Building,
                id: 2,
            },
            CrossingCandidate {
                t: 0.3,
                height_m: 0.5, // below both endpoint heights → below LOS
                kind: ObstacleKind::Building,
                id: 3,
            },
        ];
        let terrain = [0.0_f64; NUM_BANDS];
        let (_, trace) = screening_attenuation_with_meta(
            &mut p,
            ObstacleInput { candidates: &cands },
            102.0,
            102.0,
            0.0,
            &terrain,
        );
        assert!(
            (screening_edge(&trace).t - 0.9).abs() < 1e-12,
            "near-receiver strong candidate must represent the result"
        );
    }
}
