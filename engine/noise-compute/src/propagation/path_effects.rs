//! Shared path effect computation for popup and pipeline.
//!
//! All four path-effect rasters (DEM, Overture building, WorldCover forest,
//! IMD imperviousness) are sampled by [`RasterSampler::build_path_profile`]
//! into a single [`PathProfile`]. The six entry points in this module read
//! from that profile; they never walk the path again.
//!
//! See [`super::path_profile`] for the canonical cadence and docs.

use super::diffraction;
use super::diffraction::DiffractionResult;
use super::horizon::single_edge_atten;
use super::iso9613::GroundPath;
use super::obstacle_index::{segment_intersection_t, CrossingCandidate, ObstacleKind};
use super::path_profile::{
    clamp_source_platform, path_integral_u8, source_platform_clamped, vegetation_run_length,
    PathProfile,
};
use super::vegetation;
use crate::constants::{M_PER_DEG_LAT, M_PER_DEG_LON_EQ};
use crate::types::{
    Barrier, EdgePoint, ObstacleEdge, ScreeningObstacleTrace, TerrainTrace, BARRIER_PATH_HORIZON_M,
    NUM_BANDS,
};

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
    if profile.t.len() < 3 || profile.dist_m < 30.0 {
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
    let src_h = (src_elev - src_ground).max(0.05);
    let rcv_ground = profile.elevation_m[n - 1] as f64;
    let rcv_h = (rcv_alt - rcv_ground).max(crate::constants::DEFAULT_RECEIVER_HEIGHT.min(0.5));
    let dist_m = profile.dist_m;

    let PathProfile {
        t,
        elevation_m,
        elevation_f64_scratch,
        ..
    } = profile;
    let prof_f64 = PathProfile::elevation_f64_from_mut(elevation_f64_scratch, elevation_m);
    // Source-platform clamp: the phantom near-source hump must not diffract
    // (SPEC §3.5.1). The scratch is shared with the screening pass below, so
    // the composite profile inherits the same carved bare-earth by construction.
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
/// them through the same raster path), with BOTH endpoints included so
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
    if n < 3 || dist_m < 30.0 || elevation_m.len() != n {
        return None;
    }
    let dz_total = rcv_alt - src_elev;
    let e0 = elevation_m[0] as f64;
    // Source-platform clamp, read-time form (SPEC §3.5.1): the exact march
    // carves the same samples, so a subset clamped by the same rule stays a
    // sound lower bound of the carved full march (subset-of-carved =
    // carved-of-subset — the rule is pointwise in (t, e) given shared e0).
    if !t.iter().zip(elevation_m.iter()).any(|(&ti, &e)| {
        source_platform_clamped(ti, dist_m, e as f64, e0) > src_elev + dz_total * ti
    }) {
        return None;
    }
    let src_h = (src_elev - e0).max(0.05);
    let rcv_h = (rcv_alt - elevation_m[n - 1] as f64)
        .max(crate::constants::DEFAULT_RECEIVER_HEIGHT.min(0.5));
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
/// Scans `building_h_m[]` for the tallest raster obstacle above line-of-sight,
/// and intersects the ray with every explicit noise barrier of `barriers`
/// (walls are vector polylines, never a raster channel — see §1 of
/// [`screening_attenuation_with_meta`]).
///
/// `exclusion_radius_m`: skip building samples closer than this distance from
/// source — the source polygon's own buildings are not real obstacles. Never
/// applied to barriers: an explicit wall is always a real obstacle.
pub fn screening_attenuation(
    profile: &mut PathProfile,
    barriers: &[Barrier],
    obstacles: ObstacleInput<'_>,
    src_elev: f64,
    rcv_alt: f64,
    exclusion_radius_m: f64,
    terrain_atten: &[f64; NUM_BANDS],
) -> [f64; NUM_BANDS] {
    // No building or barrier anywhere on the path ⇒ the composite top profile
    // equals bare earth ⇒ the screening increment over terrain is exactly zero.
    // Skip the per-sample composite scan + OLS Fresnel fit (the dominant
    // screening cost) for the rural majority. Conservative: a building at an
    // endpoint (which the scan itself ignores) still trips the flag, so a real
    // interior obstacle is never skipped.
    //
    // Barrier arm: `dist_m` is sorted-ascending and a lower bound on the
    // receiver→midpoint distance (`types::Barrier` contract), so when even the
    // NEAREST barrier is past the `path_len + BARRIER_PATH_HORIZON_M` horizon
    // the crossing loop below would break on its first iteration and find
    // nothing — result-identical to an empty slice, minus the wasted full
    // screening pass. This keeps the rural fast path alive for the majority of
    // pixels in a tile that carries a wall somewhere (heatmap passes one slice
    // per tile). Vector-obstacle candidates count as obstacles too: a non-empty
    // candidate slice must never take this bypass.
    let barriers_in_reach = barriers
        .first()
        .is_some_and(|b| b.dist_m <= profile.dist_m + BARRIER_PATH_HORIZON_M);
    let sample_buildings_active =
        !obstacles.replace_sample_buildings && profile.building_h_m.iter().any(|&b| b > 0);
    if !barriers_in_reach && !sample_buildings_active && obstacles.candidates.is_empty() {
        return [0.0; NUM_BANDS];
    }
    let (atten, _) = screening_attenuation_with_meta(
        profile,
        barriers,
        obstacles,
        src_elev,
        rcv_alt,
        exclusion_radius_m,
        terrain_atten,
    );
    atten
}

/// Vector-obstacle input for screening (geodata-v2 1.3, `QM_VECTOR_BUILDINGS`).
/// `CANDIDATES_OFF` reproduces the raster path byte-for-byte. When
/// `replace_sample_buildings` is set, the cadence composite drops the raster
/// building channel entirely — buildings arrive ONLY as exact crossings, so
/// the A/B isolates the representation change (barriers stay on their sample
/// path until plan step 1.7).
#[derive(Clone, Copy)]
pub struct ObstacleInput<'a> {
    pub candidates: &'a [CrossingCandidate],
    pub replace_sample_buildings: bool,
}

fn barrier_crossing_candidates(
    barriers: &[Barrier],
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    dist_m: f64,
) -> impl Iterator<Item = CrossingCandidate> + '_ {
    let meters_per_deg_lon = M_PER_DEG_LON_EQ * ((src_lat + rcv_lat) * 0.5).to_radians().cos();
    let path_dx_m = (rcv_lon - src_lon) * meters_per_deg_lon;
    let path_dy_m = (rcv_lat - src_lat) * M_PER_DEG_LAT;
    let barrier_horizon_m = dist_m + BARRIER_PATH_HORIZON_M;
    barriers
        .iter()
        .take_while(move |barrier| barrier.dist_m <= barrier_horizon_m)
        .filter_map(move |barrier| {
            let x0 = (barrier.start_lon - src_lon) * meters_per_deg_lon;
            let y0 = (barrier.start_lat - src_lat) * M_PER_DEG_LAT;
            let x1 = (barrier.end_lon - src_lon) * meters_per_deg_lon;
            let y1 = (barrier.end_lat - src_lat) * M_PER_DEG_LAT;
            let t = segment_intersection_t(0.0, 0.0, path_dx_m, path_dy_m, x0, y0, x1, y1)?;
            Some(CrossingCandidate {
                t,
                height_m: barrier.height_m,
                kind: ObstacleKind::Barrier,
                // Current world OSM way ids fit u32; stable V2 identity remains
                // the separate bit-preserving `(osm_id, segment_idx)` ABI.
                id: barrier.osm_id as u32,
            })
        })
}

/// Whether a V2 H0 line-node ray owns the vector composite in N-11.
///
/// This reports existence of an exact building/wall crossing, not whether its
/// rounded screening increment is positive. It shares the barrier constructor
/// with [`screening_attenuation_with_meta`], so the CPU H0 reference cannot
/// drift from the production path while deciding the composite branch.
#[must_use]
pub fn line_vector_path_present(
    profile: &PathProfile,
    barriers: &[Barrier],
    obstacle_candidates: &[CrossingCandidate],
) -> bool {
    profile.t.len() >= 3
        && profile.dist_m >= 30.0
        && (!obstacle_candidates.is_empty()
            || barrier_crossing_candidates(
                barriers,
                profile.src_lat,
                profile.src_lon,
                profile.rcv_lat,
                profile.rcv_lon,
                profile.dist_m,
            )
            .next()
            .is_some())
}

impl ObstacleInput<'static> {
    pub const CANDIDATES_OFF: ObstacleInput<'static> = ObstacleInput {
        candidates: &[],
        replace_sample_buildings: false,
    };
}

/// Screening attenuation + obstacle trace for popup tooltips.
///
/// Combines terrain+building+barrier diffraction into a single Fresnel
/// computation over a composite top profile, returning the *increment* over
/// pure-terrain diffraction. `terrain_atten` must be the result of a prior
/// `terrain_attenuation[_with_meta]` call on the same profile/source/receiver —
/// reused here so we don't recompute bare-earth diffraction twice. In
/// `iso9613.rs`, `A_terrain + A_screen` then equals the true combined
/// attenuation, with no terrain+screening double-count (the pre-merge
/// implementation could over-attenuate by up to 10 dB when a building sat
/// on a hill — both terms then claimed full Fresnel diffraction).
///
/// The δ* Rayleigh gate uses **bare-earth** elevation for the OLS mean-ground
/// fit (CNOSSOS §2.5.6(c)). Feeding composite heights to OLS would drag the
/// mean-ground plane up to rooftops, silently breaking ground-reflection
/// physics.
pub fn screening_attenuation_with_meta(
    profile: &mut PathProfile,
    barriers: &[Barrier],
    obstacles: ObstacleInput<'_>,
    src_elev: f64,
    rcv_alt: f64,
    exclusion_radius_m: f64,
    terrain_atten: &[f64; NUM_BANDS],
) -> ([f64; NUM_BANDS], ScreeningObstacleTrace) {
    let excl_limit = exclusion_radius_m.max(0.0);
    let dist_m = profile.dist_m;
    let (src_lat, src_lon) = (profile.src_lat, profile.src_lon);
    let (rcv_lat, rcv_lon) = (profile.rcv_lat, profile.rcv_lon);
    let n = profile.t.len();
    // Copy scalars before the later split-borrow of `profile` via destructure.
    let step_m_med = profile.step_m_med as f64;

    let make_empty = || ScreeningObstacleTrace {
        kind: "none",
        height_m: 0.0,
        t: 0.0,
        screen_h_m: 0.0,
        delta_m: 0.0,
        samples_taken: 0,
        step_m: step_m_med,
        n_edges: 0,
        edges: Vec::new(),
        obstacle_id: None,
    };

    if n < 3 || dist_m < 30.0 {
        return ([0.0; NUM_BANDS], make_empty());
    }

    // 1. Barriers — EXACT 2D ray×segment crossings (fix-pack Fix 3).
    //
    //    A noise wall is a polyline element with two endpoints, so "does this
    //    path cross it, and where" is a closed-form intersection — the same
    //    `segment_intersection_t` primitive the vector obstacle index runs on
    //    building edges, in a local metric frame with the source at the origin.
    //    Each hit becomes an ordinary dominant-edge CANDIDATE in §5b: exact
    //    chainage, terrain LERPed under it, δ-ranked against every other
    //    obstacle. Barriers never enter the cadence sample arrays (the MAXT
    //    envelope, the IMD/vegetation integral algebra and the bare-earth δ*
    //    fit stay untouched) and never enter `ObstacleIndex` — they arrive
    //    per-tile as this sorted `types::Barrier` slice.
    //
    //    Replaces a ±50 m MIDPOINT-PROXIMITY test snapped to the nearest
    //    profile sample, which missed a real crossing far from a long wall's
    //    midpoint and falsely screened a near-midpoint pass that never crossed.
    //
    //    `take_while` IS the early break of the `types::Barrier` contract: the
    //    slice is sorted ascending on a lower-bound `dist_m`, so the first
    //    barrier past the horizon ends the scan (see `BARRIER_PATH_HORIZON_M`
    //    for why the horizon is the path length plus a wall half-length).
    let barrier_candidates =
        barrier_crossing_candidates(barriers, src_lat, src_lon, rcv_lat, rcv_lon, dist_m);

    // 2. Bare-earth elevation as f64 (reuses amortized scratch buffer).
    //    Split-borrow pattern per terrain_attenuation_with_meta. No copy:
    //    we hold the scratch slice for the rest of the function.
    let PathProfile {
        t,
        elevation_m,
        building_h_m,
        elevation_f64_scratch,
        composite_h_scratch,
        ..
    } = profile;
    let elevation_f64_mut = PathProfile::elevation_f64_from_mut(elevation_f64_scratch, elevation_m);
    // Same source-platform clamp as the terrain pass (idempotent when that
    // pass already ran on this profile — the shared scratch stays carved):
    // without it a phantom hump the terrain pass carved away would re-enter
    // through the composite as a spurious "screening" increment (SPEC §3.5.1).
    clamp_source_platform(t, elevation_f64_mut, dist_m);
    let elevation_f64: &[f64] = elevation_f64_mut;

    // 3. Composite top profile = elevation + raster building height, with the
    //    exclusion radius zeroing buildings near the source. Barriers are NOT
    //    here: they are exact crossings (§1) competing in the §5b candidate
    //    race, never a height folded onto the nearest sample.
    composite_h_scratch.clear();
    composite_h_scratch.reserve(n);
    let mut samples_taken: u32 = 0;
    for i in 0..n {
        let ti = t[i];
        let mut above_ground = 0.0_f64;
        if ti > 0.0 && ti < 1.0 {
            above_ground = if obstacles.replace_sample_buildings {
                // Vector mode: buildings arrive as exact crossings only.
                0.0
            } else if excl_limit > 0.0 && ti * dist_m < excl_limit {
                0.0
            } else {
                samples_taken += 1;
                building_h_m[i] as f64
            };
        }
        composite_h_scratch.push(elevation_f64[i] + above_ground);
    }

    // 4. Per-end heights above bare-earth for the diffraction API.
    let src_h = (src_elev - elevation_f64[0]).max(0.05);
    let rcv_h = (rcv_alt - elevation_f64[n - 1]).max(0.5);

    // 5. Single dominant-δ edge over the composite, δ* fit on bare-earth (was
    //    the multi-edge hull compute_path_difference_with_ols).
    let (atten_combined, res_opt) =
        single_edge_atten(t, composite_h_scratch, elevation_f64, dist_m, src_h, rcv_h);

    // 5b. Exact-crossing candidates — vector building edges AND noise-barrier
    //     segments, one race — compete with the cadence composite edge on δ,
    //     the actual selection criterion (a lower obstacle nearer the receiver
    //     can carry the larger δ). Terrain under a candidate is LERPed between
    //     its neighbouring bare samples; the candidate NEVER enters the sample
    //     arrays (plan v5: MAXT envelope, integral algebra and the bare-earth
    //     δ* fit stay untouched by construction).
    let src_e = elevation_f64[0] + src_h;
    let rcv_e = elevation_f64[n - 1] + rcv_h;
    let dsr = (dist_m * dist_m + (rcv_e - src_e).powi(2)).sqrt();
    let mut best_cand: Option<(f64, CrossingCandidate, f64)> = None; // (δ, cand, top)
    for cand in obstacles
        .candidates
        .iter()
        .copied()
        .chain(barrier_candidates)
    {
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
        let los = src_e + (rcv_e - src_e) * cand.t;
        // Below-LOS candidates stay in the race with a NEGATIVE δ (fix-pack
        // Fix 2 penumbra): a wall that just fails to break the sight line still
        // attenuates down to δ = −λ/20. Ranking on the signed δ keeps any real
        // blocker ahead of every near miss.
        let sign = if top >= los { 1.0 } else { -1.0 };
        let d_sg = cand.t * dist_m;
        let d_rg = (1.0 - cand.t) * dist_m;
        let delta = sign
            * ((d_sg * d_sg + (top - src_e).powi(2)).sqrt()
                + (d_rg * d_rg + (top - rcv_e).powi(2)).sqrt()
                - dsr);
        if best_cand.is_none_or(|(bd, _, _)| delta > bd) {
            best_cand = Some((delta, cand, top));
        }
    }

    // 5c. Winner: exact-crossing candidate vs cadence sample edge, by δ.
    let candidate_wins = match (&best_cand, &res_opt) {
        (Some((cd, _, _)), Some(res)) => *cd > res.delta,
        (Some(_), None) => true,
        (None, _) => false,
    };

    if candidate_wins {
        let (cd, cand, top) = best_cand.unwrap();
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
        debug_assert!((cres.delta - cd).abs() < 1e-9);
        let combined = diffraction::diffraction_attenuation_mixed(&cres);
        let mut atten_screen = [0.0_f64; NUM_BANDS];
        for i in 0..NUM_BANDS {
            atten_screen[i] = (combined[i] - terrain_atten[i]).max(0.0);
        }
        // A near miss the penumbra branch (or the Rayleigh gate) zeroed is not
        // an obstacle: reporting it would list a building the path flies over.
        if cres.delta <= 0.0 && atten_screen.iter().all(|&a| a <= 0.0) {
            let mut tr = make_empty();
            tr.samples_taken = samples_taken;
            return ([0.0; NUM_BANDS], tr);
        }
        // Trace straight from the crossing — exact kind + geometry, no
        // sample-based classification heuristics.
        let kind: &'static str = match cand.kind {
            ObstacleKind::Building => "building",
            ObstacleKind::Barrier => "barrier",
        };
        let los_edge = src_e + (rcv_e - src_e) * cand.t;
        let trace = ScreeningObstacleTrace {
            kind,
            height_m: cand.height_m as f64,
            t: cand.t,
            screen_h_m: top - los_edge,
            delta_m: cres.delta,
            samples_taken,
            step_m: step_m_med,
            n_edges: 1,
            edges: vec![ObstacleEdge {
                kind,
                t: cand.t,
                height_m: cand.height_m as f64,
                screen_h_m: top - los_edge,
                obstacle_id: Some(cand.id),
            }],
            obstacle_id: Some(cand.id),
        };
        return (atten_screen, trace);
    }

    let Some(res) = res_opt else {
        let mut tr = make_empty();
        tr.samples_taken = samples_taken;
        return ([0.0; NUM_BANDS], tr);
    };

    // 6. Screening = increment of combined over terrain (passed in by the caller,
    //    already computed in terrain_attenuation — no redundant bare-earth pass).
    //    `terrain + screen` = max(A_terrain, A_combined) per band → no double-count.
    let mut atten_screen = [0.0_f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        atten_screen[i] = (atten_combined[i] - terrain_atten[i]).max(0.0);
    }

    // 7. The single δ-edge → trace. A bare-terrain dominant edge is owned by
    //    terrain_attenuation, NOT a screening obstacle (atten_screen is 0 here) —
    //    report "none" so the popup doesn't list a terrain hill as a barrier.
    let idx = res.edge_idx;
    let above = (composite_h_scratch[idx] - elevation_f64[idx]).max(0.0);
    if above <= 0.0 {
        let mut tr = make_empty();
        tr.samples_taken = samples_taken;
        return (atten_screen, tr);
    }
    // The composite carries ONLY raster building heights (§3), so a winning
    // sample edge is a building by construction — walls are reported from
    // their exact crossing above, with their OSM id.
    let kind: &'static str = "building";
    let los_edge = src_elev + (rcv_alt - src_elev) * t[idx];
    let screen_h = composite_h_scratch[idx] - los_edge;
    let height_m = above;

    let trace = ScreeningObstacleTrace {
        kind,
        height_m,
        t: t[idx],
        screen_h_m: screen_h,
        delta_m: res.delta,
        samples_taken,
        step_m: step_m_med,
        n_edges: 1,
        edges: vec![ObstacleEdge {
            kind,
            t: t[idx],
            height_m,
            screen_h_m: screen_h,
            obstacle_id: None,
        }],
        obstacle_id: None,
    };

    (atten_screen, trace)
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
    // carve this scratch with the source-platform clamp (SPEC §3.5.1), and the
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
    use crate::propagation::path_profile::fill_t_values;

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
        p.building_h_m = vec![0; n];
        p.forest_u8 = vec![0; n];
        p.imd_u8 = vec![50; n];
        p.step_m_med = if n > 1 {
            ((p.t[1] - p.t[0]) * dist_m) as f32
        } else {
            0.0
        };
        p
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

    /// THE defect SPEC §3.5.1 exists for: a phantom shoulder hump one sample
    /// (~10 m) from the source on a downhill embankment path must NOT dominate
    /// the terrain term — after the platform clamp, only the genuine plateau
    /// edge (source cell's own elevation) may diffract. Geometry measured on
    /// the D4 at Voznice (owner report 2026-08-20): src cell 375.28, phantom
    /// 375.80 at 10 m, receiver 51 m downhill at 366.34.
    /// The ground mean-plane must read the RAW profile even after the
    /// terrain pass carved the shared scratch (SPEC §3.5.1): ground result is
    /// identical whether or not terrain ran first, and repeats are stable.
    #[test]
    fn ground_path_is_blind_to_the_platform_clamp() {
        let build = || {
            let mut p = PathProfile::new();
            p.dist_m = 50.9;
            p.t = vec![0.0, 0.1963, 0.5, 0.8037, 1.0];
            p.elevation_m = vec![375.28, 375.80, 371.0, 369.0, 366.34];
            p.building_h_m = vec![0; 5];
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

    #[test]
    fn phantom_shoulder_hump_is_carved_to_the_platform() {
        let dist = 50.9;
        let mut p = PathProfile::new();
        p.dist_m = dist;
        p.t = vec![0.0, 0.1963, 0.5, 0.8037, 1.0];
        p.elevation_m = vec![375.28, 375.80, 371.0, 369.0, 366.34];
        p.building_h_m = vec![0; 5];
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
        p.building_h_m = vec![0; 5];
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
    /// distances.
    #[test]
    fn terrain_subset_bound_never_exceeds_the_full_cadence_bands() {
        use super::super::diffraction::diffraction_mixed_lower_bound;
        use crate::constants::{FAV_RAY_CURVATURE_MIN_M, FAV_RAY_CURVATURE_PER_DSR};
        let src_elev = 10.05;
        let rcv_alt = 11.5;
        for &dist in &[600.0, 1_000.0, 3_000.0] {
            for &hill_t in &[0.2, 0.35, 0.5, 0.65, 0.8] {
                for &hill_h in &[14.0, 20.0, 30.0, 45.0, 70.0] {
                    let mut p = build_flat_profile(dist, 10.0);
                    let (idx, _) =
                        p.t.iter()
                            .enumerate()
                            .min_by(|(_, &a), (_, &b)| {
                                ((a - hill_t).abs())
                                    .partial_cmp(&((b - hill_t).abs()))
                                    .unwrap()
                            })
                            .unwrap();
                    p.elevation_m[idx] = hill_h;
                    let full = terrain_attenuation(&mut p, src_elev, rcv_alt);
                    let n = p.t.len();
                    let k = 8usize;
                    let subset: Vec<usize> = (0..k)
                        .map(|j| ((j as f64) * (n - 1) as f64 / (k - 1) as f64).round() as usize)
                        .collect();
                    let t_sub: Vec<f64> = subset.iter().map(|&i| p.t[i]).collect();
                    let e_sub: Vec<f32> = subset.iter().map(|&i| p.elevation_m[i]).collect();
                    let Some((delta_sub, dsr)) =
                        terrain_subset_delta_lower_bound(&t_sub, &e_sub, dist, src_elev, rcv_alt)
                    else {
                        continue; // no hill in the subset ⇒ no bound ⇒ sound
                    };
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
            }
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
        p.building_h_m[idx] = 20;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            &[],
            ObstacleInput::CANDIDATES_OFF,
            0.01,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(trace.kind, "building");
        assert!(trace.height_m == 20.0);
        assert_eq!(
            trace.edges.len(),
            trace.n_edges as usize,
            "edges vec must match n_edges"
        );
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
            y_m / crate::constants::M_PER_DEG_LAT,
            x_m / crate::constants::M_PER_DEG_LON_EQ,
        )
    }

    /// A wall microsegment from (x0, y0) to (x1, y1) scene metres, with
    /// `dist_m` given as the (lower-bound) receiver→midpoint distance.
    fn wall(x0: f64, y0: f64, x1: f64, y1: f64, height_m: f32, dist_m: f64) -> Barrier {
        let (start_lat, start_lon) = barrier_ll(x0, y0);
        let (end_lat, end_lon) = barrier_ll(x1, y1);
        Barrier {
            osm_id: 1_390_017_809,
            segment_idx: 0,
            height_m,
            start_lat,
            start_lon,
            end_lat,
            end_lon,
            dist_m,
        }
    }

    /// Mid-path 3 m barrier on a flat profile must screen, and the band-only
    /// wrapper must agree with `_with_meta` (the heatmap kernels call the
    /// wrapper; popup calls `_with_meta` — parity by construction).
    #[test]
    fn screening_finds_midpath_barrier() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 60 m of wall straddling the path at t = 0.5.
        let barrier = wall(100.0, -30.0, 100.0, 30.0, 3.0, dist_m / 2.0);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(trace.kind, "barrier");
        assert!(
            atten.iter().any(|&a| a > 0.0),
            "3 m wall above the 0.05→1.5 m LOS must screen"
        );
        let mut p2 = build_flat_profile(dist_m, 0.0);
        let bands = screening_attenuation(
            &mut p2,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(bands, atten, "band-only wrapper == _with_meta bands");
    }

    #[test]
    fn h0_vector_presence_is_crossing_existence_not_screening_magnitude() {
        let dist_m = 200.0;
        let crossing = wall(100.0, -30.0, 100.0, 30.0, 0.01, dist_m / 2.0);
        let miss = wall(100.0, 20.0, 100.0, 30.0, 20.0, dist_m / 2.0);
        let profile = build_flat_profile(dist_m, 0.0);
        assert!(line_vector_path_present(
            &profile,
            std::slice::from_ref(&crossing),
            &[]
        ));
        assert!(!line_vector_path_present(
            &profile,
            std::slice::from_ref(&miss),
            &[]
        ));
        let candidate = CrossingCandidate {
            t: 0.5,
            height_m: 0.01,
            kind: ObstacleKind::Building,
            id: 7,
        };
        assert!(line_vector_path_present(
            &profile,
            &[],
            std::slice::from_ref(&candidate)
        ));
    }

    /// Early-out refinement: with no buildings and every (sorted, lower-bound
    /// dist) barrier past the `path_len + BARRIER_PATH_HORIZON_M` horizon, the
    /// wrapper must return exactly the empty-slice result (the crossing scan
    /// stops on its first item) — this is what keeps the rural fast path alive
    /// on heatmap tiles that carry a wall somewhere else in the tile.
    #[test]
    fn far_barriers_hit_the_early_out_unchanged() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let far = wall(
            0.0,
            0.0,
            60.0,
            0.0,
            3.0,
            dist_m + BARRIER_PATH_HORIZON_M + 1.0,
        );
        let mut p = build_flat_profile(dist_m, 0.0);
        let bands = screening_attenuation(
            &mut p,
            std::slice::from_ref(&far),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        let mut p2 = build_flat_profile(dist_m, 0.0);
        let empty = screening_attenuation(
            &mut p2,
            &[],
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(bands, empty);
        assert!(bands.iter().all(|&a| a == 0.0));
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
        let barrier = wall(100.0, -20.0, 140.0, 220.0, 3.0, 100.0);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(trace.kind, "barrier");
        assert_eq!(trace.obstacle_id, Some(1_390_017_809));
        assert!(
            (trace.t - 103.3333 / dist_m).abs() < 1e-6,
            "t = {}",
            trace.t
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
        let barrier = wall(80.0, 10.0, 120.0, 10.0, 3.0, 10.0);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(
            trace.kind, "none",
            "a wall the path passes BY is not a screen"
        );
        assert!(atten.iter().all(|&a| a == 0.0), "{atten:?}");
    }

    /// The horizon constant is load-bearing, not slack: a wall crossing the
    /// path near the SOURCE while running away from the receiver puts its
    /// midpoint up to a half-segment (125 m) past the path's own length. The
    /// pre-Fix-3 `+100 m` horizon would break the scan before reaching it.
    #[test]
    fn crossing_wall_beyond_the_old_horizon_still_screens() {
        let dist_m = 200.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        // 247 m wall (5, 0.5) → (−235, −60): crosses the path at x ≈ 3.0,
        // midpoint (−115, −29.75) ⇒ 316 m from the receiver at (200, 0).
        let barrier = wall(5.0, 0.5, -235.0, -60.0, 3.0, 316.4);
        assert!(
            barrier.dist_m > dist_m + 100.0,
            "past the pre-Fix-3 horizon"
        );
        assert!(barrier.dist_m < dist_m + BARRIER_PATH_HORIZON_M);
        let mut p = build_flat_profile(dist_m, 0.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(trace.kind, "barrier");
        assert!(atten.iter().any(|&a| a > 0.0), "{atten:?}");
    }

    /// A barrier and a building on one path race on δ, not on kind: the
    /// exact-crossing wall nearer the receiver wins over a taller raster
    /// building sample mid-path, and the trace names it.
    #[test]
    fn barrier_and_building_race_on_delta() {
        let dist_m = 400.0;
        let terrain_atten = [0.0_f64; NUM_BANDS];
        let mut p = build_flat_profile(dist_m, 0.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p.building_h_m[idx] = 4; // 4 m building at mid-path
                                 // 4 m wall at t = 0.95 — nearer the receiver ⇒ larger δ.
        let barrier = wall(380.0, -20.0, 380.0, 20.0, 4.0, 20.0);
        let (atten, trace) = screening_attenuation_with_meta(
            &mut p,
            std::slice::from_ref(&barrier),
            ObstacleInput::CANDIDATES_OFF,
            0.05,
            1.5,
            0.0,
            &terrain_atten,
        );
        assert_eq!(trace.kind, "barrier");
        assert!((trace.t - 0.95).abs() < 1e-9, "t = {}", trace.t);
        assert!(atten.iter().any(|&a| a > 0.0));
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
        // building_h_m all zero — guaranteed by build_flat_profile.
        let (terrain_trace, _) = terrain_attenuation_with_meta(&mut p, 10.05, 11.5);
        let (atten, screening_trace) = screening_attenuation_with_meta(
            &mut p,
            &[],
            ObstacleInput::CANDIDATES_OFF,
            10.05,
            11.5,
            0.0,
            &terrain_trace.attenuation_bands,
        );
        assert_eq!(
            screening_trace.kind, "none",
            "bare hill is not a screening obstacle"
        );
        assert_eq!(screening_trace.n_edges, 0);
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
        p.building_h_m[idx] = 6; // + 6 m building on top
        let src_elev = 100.05;
        let rcv_alt = 104.0;

        let terrain = terrain_attenuation(&mut p, src_elev, rcv_alt);
        let (screen, _) = screening_attenuation_with_meta(
            &mut p,
            &[],
            ObstacleInput::CANDIDATES_OFF,
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
        composite[idx] += p.building_h_m[idx] as f64;
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

    /// PARITY: a candidate at exactly a sample's t with the sample's height
    /// must reproduce the raster path bit-for-bit — top, δ, δ* and bands all
    /// coincide (terrain lerp at a sample t is the sample; the δ* split puts
    /// the same points on each side).
    #[test]
    fn candidate_at_sample_matches_raster_screening_exactly() {
        let mut p_raster = build_flat_profile(500.0, 100.0);
        let (idx, _) = p_raster
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p_raster.building_h_m[idx] = 12;
        let t_edge = p_raster.t[idx];
        let terrain = [0.0_f64; NUM_BANDS];
        let (a_raster, tr_raster) = screening_attenuation_with_meta(
            &mut p_raster,
            &[],
            ObstacleInput::CANDIDATES_OFF,
            100.05,
            104.0,
            0.0,
            &terrain,
        );

        let mut p_vec = build_flat_profile(500.0, 100.0);
        let cands = [CrossingCandidate {
            t: t_edge,
            height_m: 12.0,
            kind: ObstacleKind::Building,
            id: 1,
        }];
        let (a_vec, tr_vec) = screening_attenuation_with_meta(
            &mut p_vec,
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            100.05,
            104.0,
            0.0,
            &terrain,
        );
        assert!(tr_raster.delta_m > 0.0);
        assert!(
            (tr_raster.delta_m - tr_vec.delta_m).abs() < 1e-12,
            "δ parity: raster {} vs candidate {}",
            tr_raster.delta_m,
            tr_vec.delta_m
        );
        for i in 0..NUM_BANDS {
            assert!(
                (a_raster[i] - a_vec[i]).abs() < 1e-12,
                "band {i}: raster {} vs candidate {}",
                a_raster[i],
                a_vec[i]
            );
        }
        assert_eq!(tr_vec.kind, "building");
    }

    /// A candidate BETWEEN cadence samples with larger δ beats the sample
    /// edge — the exact-position benefit the vector engine exists for.
    #[test]
    fn candidate_between_samples_wins_on_delta() {
        let mut p = build_flat_profile(500.0, 100.0);
        let (idx, _) = p
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p.building_h_m[idx] = 6; // modest mid-path sample obstacle
                                 // Tall candidate near the receiver, deliberately between samples.
        let t_c = (p.t[p.t.len() - 2] + p.t[p.t.len() - 1]) / 2.0;
        let cands = [CrossingCandidate {
            t: t_c,
            height_m: 14.0,
            kind: ObstacleKind::Building,
            id: 7,
        }];
        let terrain = [0.0_f64; NUM_BANDS];
        let (_, trace) = screening_attenuation_with_meta(
            &mut p,
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: false,
            },
            100.05,
            104.0,
            0.0,
            &terrain,
        );
        assert_eq!(trace.kind, "building");
        assert!(
            (trace.t - t_c).abs() < 1e-12,
            "candidate edge must win: trace.t {} vs t_c {}",
            trace.t,
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
                &[],
                ObstacleInput {
                    candidates: &cands,
                    replace_sample_buildings: true,
                },
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

    /// `replace_sample_buildings` drops the raster building channel: with no
    /// candidates, a path full of raster buildings screens ZERO (the A/B
    /// isolation semantics of QM_VECTOR_BUILDINGS).
    #[test]
    fn replace_mode_ignores_raster_buildings() {
        let mut p = build_flat_profile(500.0, 100.0);
        for b in p.building_h_m.iter_mut() {
            *b = 15;
        }
        let terrain = [0.0_f64; NUM_BANDS];
        let bands = screening_attenuation(
            &mut p,
            &[],
            ObstacleInput {
                candidates: &[],
                replace_sample_buildings: true,
            },
            100.05,
            104.0,
            0.0,
            &terrain,
        );
        assert!(bands.iter().all(|&a| a == 0.0));
    }

    /// A candidate must still screen when the sample composite is clear —
    /// the winner logic can't early-return on `res_opt == None`.
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
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            50.05,
            51.5,
            0.0,
            &terrain,
        );
        assert!(atten.iter().any(|&a| a > 0.0), "candidate must screen");
        assert_eq!(trace.kind, "building");
        assert!((trace.t - 0.7).abs() < 1e-12);
    }

    /// Sloped-terrain exact-sample parity: with D in BOTH δ* fits (SPEC
    /// §3.5b), a candidate at a sample's t reproduces the raster result on a
    /// non-flat profile too — the flat-terrain blind spot the gg review
    /// flagged is closed.
    #[test]
    fn candidate_at_sample_matches_raster_on_slope() {
        let make = || {
            let mut p = build_flat_profile(500.0, 100.0);
            let n = p.t.len();
            for i in 0..n {
                p.elevation_m[i] = 100.0 + (8.0 * p.t[i]) as f32; // steady rise
            }
            p
        };
        let mut p_raster = make();
        let (idx, _) = p_raster
            .t
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| ((a - 0.5).abs()).partial_cmp(&((b - 0.5).abs())).unwrap())
            .unwrap();
        p_raster.building_h_m[idx] = 12;
        let t_edge = p_raster.t[idx];
        let terrain = [0.0_f64; NUM_BANDS];
        let (a_raster, tr_raster) = screening_attenuation_with_meta(
            &mut p_raster,
            &[],
            ObstacleInput::CANDIDATES_OFF,
            100.05,
            112.0,
            0.0,
            &terrain,
        );
        let mut p_vec = make();
        let cands = [CrossingCandidate {
            t: t_edge,
            height_m: 12.0,
            kind: ObstacleKind::Building,
            id: 1,
        }];
        let (a_vec, tr_vec) = screening_attenuation_with_meta(
            &mut p_vec,
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            100.05,
            112.0,
            0.0,
            &terrain,
        );
        assert!(tr_raster.delta_m > 0.0);
        assert!((tr_raster.delta_m - tr_vec.delta_m).abs() < 1e-12);
        for i in 0..NUM_BANDS {
            assert!(
                (a_raster[i] - a_vec[i]).abs() < 1e-12,
                "band {i} on slope: raster {} vs candidate {}",
                a_raster[i],
                a_vec[i]
            );
        }
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
                &[],
                ObstacleInput {
                    candidates: &cands,
                    replace_sample_buildings: true,
                },
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
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
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
        assert_eq!(trace.kind, "building");
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
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            0.05,
            4.0,
            0.0,
            &terrain,
        );
        assert!(atten.iter().all(|&a| a == 0.0), "{atten:?}");
        assert_eq!(trace.kind, "none", "nothing to report to the popup");
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
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            0.05,
            4.0,
            0.0,
            &terrain,
        );
        assert_eq!(trace.obstacle_id, Some(2));
        assert!(trace.delta_m > 0.0);
    }

    /// Multiple candidates: the max-δ one wins; a below-LOS candidate is
    /// ignored entirely.
    #[test]
    fn max_delta_candidate_wins_and_below_los_skipped() {
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
            &[],
            ObstacleInput {
                candidates: &cands,
                replace_sample_buildings: true,
            },
            102.0,
            102.0,
            0.0,
            &terrain,
        );
        assert!(
            (trace.t - 0.9).abs() < 1e-12,
            "near-receiver max-δ candidate must win"
        );
    }
}
