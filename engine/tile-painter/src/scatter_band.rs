//! Generic surface-scatter kernel for the road/rail line sources
//! ([`crate::scatter_line`], via its `LineGeometry`) and the industrial/building
//! point sources ([`crate::scatter_point`], via its `PointGeometry`). Both walk
//! the SAME receiver-block structure, energy-budget skip, terrain ray-march,
//! `max(A_gr, A_bar)` path assembly, and 3-period accumulation; they differ ONLY
//! in the per-pixel geometry that turns a (source, receiver) pair into the
//! propagation terms. That divergence is the [`PixelGeometry`] trait; everything
//! else is this one kernel, so a propagation bug is fixed once and a future
//! optimisation lands on both.
//!
//! What stays per-geometry (the [`PixelGeometry::pixel`] return [`PixelTerms`]):
//!  * divergence law — line is ISO 9613-2 CYLINDRICAL `10·log10(2π·d_slant)`,
//!    point is SPHERICAL `20·log10(d)+11`;
//!  * the line's finite-line correction (folded into `base_db`) vs the point's
//!    free-field audibility pre-gate (a real per-pixel cull — `pixel` returns
//!    `None`) and its exclusion radius (effective distance + screening exclusion);
//!  * the ground model — line path-averages `ground_g_from_profile` (hard `G=0`
//!    on a bridge), point samples the RECEIVER's `ground_g` (oracle parity);
//!  * the profile sample point — line uses the segment foot, point the source.
//!
//! ground-ops ([`crate::ground_ops`]) shares the machinery (the [`BandScratch`]
//! and the helpers below) but NOT this kernel: it has per-row event weights, a
//! mixed-geometry skip bound, and a different Lden collapse, so its band body
//! stays its own.
//!
//! ## Energy-budget skip (receiver-band ownership)
//!
//! Most far/quiet sources are inaudible at a pixel a louder near source already
//! dominates — computing their exact terrain/diffraction path is wasted work an
//! aggregate Lden can't resolve. So per pixel we track the kept Lden energy and a
//! `skipped` accumulator: a pair whose BEST-CASE contribution (a cheap upper
//! bound — no terrain/screening/veg + max ground gain ⇒ provably ≥ exact) keeps
//! total skipped within `BUDGET_ETA` of kept is dropped without the profile
//! build. Total under-read is bounded by `10·log10(1+η) = 1.5 dB`. Unlike a
//! reach-radius cut this is PER-PIXEL energy-aware: an isolated rural dwelling
//! whose only source is one far road has no louder source to mask it, so `kept`
//! stays ~0 and the source is computed exactly.
//!
//! The skip needs each pixel's running `kept` to see ALL its sources, so the
//! scatter is parallelised over receiver BLOCKS (not over sources): one block
//! owns a square pixel rectangle ([`recv_block_regions`]) and loops every source
//! clipped to it. (Source-major `par_iter` splits a pixel's sources across
//! threads → partial budgets, measured ~10 % skip vs ~30-46 % for block
//! ownership.)

use std::f64::consts::LN_10;
use std::sync::OnceLock;

use noise_compute::constants::{ALPHA_ATM, A_WEIGHTING, GROUND_GAIN_UB_DB};
use noise_compute::propagation::arc_screening::{
    arc_screened_attenuation, segment_can_span, ArcBounds, ArcScreening, ArcScreeningScratch,
    ArcSkyline,
};
use noise_compute::propagation::iso9613::{fast_exp_f64, ground_atten_db, ground_or_barrier_db};
use noise_compute::propagation::obstacle_index::{CellPrune, CrossingCandidate, ObstacleSet};
use noise_compute::propagation::path_effects;
use noise_compute::propagation::path_profile::CoarseMid;
use noise_compute::propagation::seg_sampling::{
    sampled_gob_bands, seg_arc_bounds, SegSampleScratch,
};
use noise_compute::propagation::PathProfile;
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBbox, TILE_PX};
use rayon::prelude::*;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};

pub(crate) const NUM_BANDS: usize = 8;

/// Receiver block size (px): the scatter parallelises over square `B×B` pixel
/// blocks, one rayon work-item each (shared by the line / point / ground-ops
/// kernels via [`recv_block_regions`]). Square blocks beat full-width row-bands by
/// ~16% on dense tiles and up to ~40% on sparse — per-core L2 locality of the
/// terrain ray-march (a compact block's hot terrain fits L1/L2, a full-width band
/// spills to shared L3) PLUS finer load-balance (a wide band over a sparse tile
/// leaves most cores idle; 1024 blocks + rayon work-stealing spread it). Measured;
/// the standard GPU "tiled rendering" / cache-blocking pattern. `SURFACE_BLOCK_PX`
/// overrides it (must be >0; e.g. 8 is ~3% faster on dense, more scheduling churn).
pub(crate) const RECV_BLOCK_PX: usize = 16;

fn recv_block_px() -> usize {
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("SURFACE_BLOCK_PX")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&b| b > 0)
            .unwrap_or(RECV_BLOCK_PX)
    })
}

/// The tile's TILE_PX×TILE_PX receiver pixels partitioned into square blocks of
/// [`recv_block_px`], each `(py_lo, py_hi, px_lo, px_hi)` a rayon work-item. Shared
/// by all three surface scatter kernels.
pub(crate) fn recv_block_regions() -> Vec<(usize, usize, usize, usize)> {
    let b = recv_block_px();
    let bps = TILE_PX.div_ceil(b);
    (0..bps * bps)
        .map(|blk| {
            let (by, bx) = (blk / bps, blk % bps);
            (
                by * b,
                ((by + 1) * b).min(TILE_PX),
                bx * b,
                ((bx + 1) * b).min(TILE_PX),
            )
        })
        .collect()
}

/// Energy-budget skip tolerance: total skipped Lden energy stays within η of
/// the kept energy, so the displayed under-read is `≤ 10·log10(1+η)`. η=0.40 ⇒
/// ≤ 1.5 dB (HM3's 1 dB quantisation can show a 2.0 dB byte step).
/// The error concentrates at LOUD pixels (large budget); faint near-floor
/// pixels keep a tiny budget so they barely skip and stay near-exact.
/// `SURFACE_BUDGET_ETA` lowers it (clamped to `[0, this]`; 0 = exact reference).
const BUDGET_ETA: f64 = 0.40;

/// Clamp the env override to `[0, BUDGET_ETA]`: it may only make the skip MORE
/// conservative (or disable it), never exceed the validated ≤1.5 dB bound — an
/// accidental `SURFACE_BUDGET_ETA=1.0` would otherwise mean a 3 dB under-read.
pub(crate) fn budget_eta() -> f64 {
    static ETA: OnceLock<f64> = OnceLock::new();
    *ETA.get_or_init(|| {
        std::env::var("SURFACE_BUDGET_ETA")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|e| e.is_finite() && *e >= 0.0)
            .map(|e| e.min(BUDGET_ETA))
            .unwrap_or(BUDGET_ETA)
    })
}

/// `fast_exp_f64` is ~1.45e-6 non-monotone at its range-reduction joints (Codex
/// /gg), so a numerically "louder" upper bound can read fractionally below the
/// exact value. Inflate the UB by 1e-4 (≫ that wobble) so `ub ≥ exact` stays
/// literally true — the skip's soundness rests on it.
pub(crate) const UB_SAFETY: f64 = 1.0001;

/// Default coarse-middle stride for the surface-heatmap ray-march: beyond the
/// full-res near-end zone the deep-middle ray is stepped at 3× the 245 m coarse
/// step (≈737 m). `SURFACE_SHADOW_STRIDE` overrides; `1` disables (exact
/// reference). The near-end zones (below) are never subsampled. Tuned on the
/// LKPR/Dobříš/rural trio against the method's own raster-phase noise floor:
/// with the 600 m zones protecting the diffraction-critical near field, stride 3
/// (vs 2) buys +11 pt deep-middle reduction at essentially the same error
/// (exceed ≤4.5 % of cells, DEV p99 ≤0.8 dB ≪ the floor's 2.6-5.2 dB p99);
/// stride 4 adds little for the same accuracy. See /tmp/s6-coarse-shadow-report.md.
const SHADOW_MID_STRIDE: usize = 3;

/// Default full-res half-window metres from each end (≈600 m). The dense
/// 10/30/60/120 m bilateral ramp is kept only within this distance of an
/// endpoint — where berms / near-receiver walls make the shadow SHARP — and the
/// far field is coarse-stepped. Tuned to the sweet spot where the coarse error
/// sits WITHIN the method's own noise floor: 200 m exceeded it (20-38 % of
/// cells), 600 m brings exceed to ≤4.5 % with ~33-44 % fewer ray samples on long
/// rays. `SURFACE_SHADOW_SRC_ZONE_M` / `SURFACE_SHADOW_RX_ZONE_M` raise it as
/// future 5 m terrain + OSM-exact buildings sharpen the field. The RECEIVER side
/// is the bigger edge-tail driver, so it can warrant a wider window (measured
/// symmetric here — the CZ fixtures show no rx/src asymmetry at 600 m).
const SHADOW_SRC_ZONE_M: f64 = 600.0;
const SHADOW_RX_ZONE_M: f64 = 600.0;

/// The surface-heatmap coarse-middle cadence config, read once from env. `None`
/// ⇒ exact cadence (the `STRIDE=1` reference path). Shared by the line, point,
/// and ground-ops kernels so the cadence is identical across the three surface
/// terrain-ray-march sources.
pub(crate) fn coarse_mid_cfg() -> Option<CoarseMid> {
    static CFG: OnceLock<Option<CoarseMid>> = OnceLock::new();
    *CFG.get_or_init(|| {
        let env_usize = |k: &str, d: usize| {
            std::env::var(k)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(d)
        };
        let env_f64 = |k: &str, d: f64| {
            std::env::var(k)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(d)
        };
        let mid_stride = env_usize("SURFACE_SHADOW_STRIDE", SHADOW_MID_STRIDE).clamp(1, 8);
        if mid_stride <= 1 {
            return None; // exact reference
        }
        Some(CoarseMid {
            src_zone_m: env_f64("SURFACE_SHADOW_SRC_ZONE_M", SHADOW_SRC_ZONE_M),
            rx_zone_m: env_f64("SURFACE_SHADOW_RX_ZONE_M", SHADOW_RX_ZONE_M),
            mid_stride,
        })
    })
}

/// Source distance (m) below which the heatmap uses the POPUP's exact cadence,
/// never the coarse middle (CNOSSOS fix-pack Fix 5 — heatmap↔popup cadence
/// alignment). The coarse-middle approximation trades mid-path sample density
/// for speed, and its error is a diffraction error: it shows up where an
/// obstacle sits between source and receiver. Those obstacles are NEAR-FIELD
/// (a wall 30 m off a road, the building across the street), and the near
/// field is also where the popup and the tile are compared point-for-point.
/// So the near field runs the exact cadence unconditionally and the
/// approximation is confined to the long rays it was measured on.
///
/// 400 m covers the whole screening-critical band (a 10 m obstacle stops
/// casting a meaningful shadow well before it) at a negligible cost: the
/// coarse middle only ever removes samples from the smooth part of a LONG ray,
/// and rays under 400 m have no such part.
pub(crate) const EXACT_CADENCE_MAX_DIST_M: f64 = 400.0;

/// The cadence for ONE ray: the configured coarse middle beyond
/// [`EXACT_CADENCE_MAX_DIST_M`], the exact popup cadence (`None`) at or below
/// it. Separated from [`build_surface_profile`] so the near-field guarantee is
/// unit-testable without a raster tile.
///
/// Below 400 m the shipped 600 m zones almost never bite (the dense ramp is
/// clamped by the ray midpoint first), but the two builders still diverge
/// there: the coarse builder bridges its middle from the last COMMITTED ramp
/// sample while the exact one bridges from the midpoint, so a ~320 m ray picks
/// up a mid sample the popup does not have. The gate removes that residual and
/// makes the guarantee STRUCTURAL rather than a side effect of two tunables —
/// `SURFACE_SHADOW_SRC_ZONE_M` / `SURFACE_SHADOW_RX_ZONE_M` can be lowered (an
/// earlier default was 200 m) without silently coarsening the near field the
/// popup is compared against.
#[inline]
pub(crate) fn cadence_for_ray(cfg: Option<CoarseMid>, dist_m: f64) -> Option<CoarseMid> {
    cfg.filter(|_| dist_m > EXACT_CADENCE_MAX_DIST_M)
}

/// Build a path profile for surface scatter, applying the coarse-middle cadence
/// when enabled AND the ray is long enough ([`cadence_for_ray`]), else the exact
/// cadence. The single call site for all three surface kernels so the cadence
/// policy lives in one place.
#[inline]
pub(crate) fn build_surface_profile(
    tile: &FusedTileZ13,
    cfg: Option<CoarseMid>,
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    dist_m: f64,
    out: &mut PathProfile,
) {
    match cadence_for_ray(cfg, dist_m) {
        Some(cm) => {
            tile.build_path_profile_coarse_mid(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, cm, out)
        }
        None => tile.build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out),
    }
}

/// The tile as a [`RasterSampler`] whose `build_path_profile` runs the SURFACE
/// cadence policy ([`build_surface_profile`]) instead of the popup's exact one.
///
/// `arc_screening` marches its own ray per blocked angular interval through the
/// `RasterSampler` it is handed. Passing the bare `FusedTileZ13` would silently
/// give those rays the exact cadence while every other ray of the same tile
/// keeps the coarse middle — a cadence fork INSIDE one kernel, and one the CUDA
/// twin (a single compile-time `fill_t`) could not mirror. Wrapping keeps the
/// heatmap's one cadence policy, `cadence_for_ray` and all.
struct SurfaceCadenceRasters<'a> {
    tile: &'a FusedTileZ13,
    cfg: Option<CoarseMid>,
}

impl RasterSampler for SurfaceCadenceRasters<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.tile.elevation(lat, lon)
    }
    fn building_height(&self, lat: f64, lon: f64) -> f64 {
        self.tile.building_height(lat, lon)
    }
    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        self.tile.ground_g(lat, lon)
    }
    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        self.tile.building_enclosure(lat, lon)
    }
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut PathProfile,
    ) {
        build_surface_profile(
            self.tile, self.cfg, src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out,
        );
    }
}

/// Lden-energy period weights = `compute_lden`'s 12/4/8-hour × 0/5/10-dB
/// penalties (`10^(penalty/10)`): collapse a pair's per-period power to the one
/// scalar the budget compares. The shared `/24` cancels in the skip RATIO, so
/// it's dropped (√10 = 3.162… for the +5 dB evening penalty).
pub(crate) const LDEN_WEIGHTS: [f64; NUM_PERIODS] = [12.0, 4.0 * 3.1622776601683795, 80.0];

/// dB path level → linear A-weighted band energy. The hot loop's innermost op,
/// shared by the budget bound and the exact path factor — keep the expression
/// identical (the exact path must stay bit-exact vs the pre-skip kernel).
#[inline]
pub(crate) fn db_to_lin_a(path_db: f64, band: usize) -> f64 {
    fast_exp_f64((path_db + A_WEIGHTING[band]) * LN_10 * 0.1)
}

/// Best-case Lden energy of a source→pixel pair for the budget skip: no
/// terrain/screening/veg + the most favourable ground any band can meet
/// ([`GROUND_GAIN_UB_DB`] = 3.0 dB, the CNOSSOS hard-ground floor attained at
/// G = 0 in every band — see the constant for why it is no longer the per-band
/// `(-CF).max(0)`), inflated by [`UB_SAFETY`] — provably ≥ the exact
/// contribution. Shared verbatim by the line + point kernels so the
/// `ub ≥ exact` soundness invariant lives in one place. `base_db` already folds
/// in divergence/FLC/reflection; `emission_lden` is the pair's Lden-weighted
/// band spectrum.
#[inline]
pub(crate) fn budget_ub_lden(base_db: f64, atm_d_km: f64, emission_lden: &[f64; NUM_BANDS]) -> f64 {
    let mut ub = 0.0;
    for i in 0..NUM_BANDS {
        let path_db = base_db - ALPHA_ATM[i] * atm_d_km + GROUND_GAIN_UB_DB;
        ub += emission_lden[i] * db_to_lin_a(path_db, i);
    }
    ub * UB_SAFETY
}

/// Per-worker scatter state, threaded through the blocks one rayon worker folds.
/// `kept`/`skipped` are full-tile but each block touches only its own (disjoint)
/// pixel rectangle, so they need no clearing between a worker's blocks. Shared by
/// the line, point, and ground-ops kernels.
pub(crate) struct BandScratch {
    pub(crate) local: TileAccumulator,
    pub(crate) profile: PathProfile,
    pub(crate) kept: Vec<f64>,
    pub(crate) skipped: Vec<f64>,
    pub(crate) path_calls: u64,
    pub(crate) skipped_calls: u64,
    /// Raster cadence samples taken by `build_path_profile` (the ray-march). Each
    /// reads a 4-cell bilinear quad, so cell reads = 4× this — the numerator of
    /// the read-redundancy metric (cell reads ÷ grid cells = ×-reread).
    pub(crate) raster_samples: u64,
    /// Vector-obstacle crossings of the current ray (geodata-v2, reused).
    pub(crate) cand_scratch: Vec<CrossingCandidate>,
    /// Arc-screening (fix-pack Fix 1) interval-ray buffers, amortised across
    /// every (source, pixel) pair this worker folds.
    pub(crate) arc_scratch: ArcScreeningScratch,
    /// One obstacle SKYLINE per pixel of the receiver block this worker owns,
    /// in block-local `(py - py_lo) * w + (px - px_lo)` order.
    ///
    /// The scatter loops SOURCES outside PIXELS (block ownership is what keeps
    /// the energy-budget skip effective), so a single skyline slot would rebuild
    /// on every pair and buy nothing. One slot per pixel of the block instead:
    /// each receiver's skyline is built on the first source that needs it and
    /// reused by every later source — the same "once per receiver" the popup
    /// kernels get for free, and the same thing the CUDA lane gets from
    /// thread-per-pixel.
    pub(crate) skylines: Vec<ArcSkyline>,
    /// Bounds ([`tile_arc_bounds`]) read once per worker, never per pair.
    pub(crate) arc_bounds: ArcBounds,
    /// Bucket-ray buffers for the angular quadrature ([`seg_samples`]). Kept
    /// off `profile`/`cand_scratch` so the cp ray's ground-`G`, terrain and
    /// vegetation stay on the cp profile, exactly as the single-verdict kernel
    /// resolves them (and exactly as `arc_screening` resolves them for its own
    /// interval rays).
    pub(crate) seg_scratch: SegSampleScratch,
}

impl BandScratch {
    pub(crate) fn new() -> Self {
        let n = TILE_PX * TILE_PX;
        Self {
            local: TileAccumulator::new(),
            profile: PathProfile::new(),
            kept: vec![0.0; n],
            skipped: vec![0.0; n],
            path_calls: 0,
            skipped_calls: 0,
            raster_samples: 0,
            cand_scratch: Vec::new(),
            arc_scratch: ArcScreeningScratch::new(),
            skylines: Vec::new(),
            arc_bounds: tile_arc_bounds(),
            seg_scratch: SegSampleScratch::new(),
        }
    }
}

/// How many uniform-in-angle buckets a line microsegment's fan is split into at
/// each receiver — the tile path's quadrature of the segment's angular span.
///
/// `1` is the single characteristic-point verdict the kernel shipped with: one
/// ray for the whole 250 m microsegment, whatever it flies past. `N > 1` tiles
/// the segment's ANGULAR span at the receiver into `N` equal buckets, evaluates
/// the ground/barrier term `max(A_ground, A_terrain + A_screen)` on the EXACT
/// source point at each bucket's centre azimuth, and energy-averages
/// (`noise_compute::propagation::seg_sampling`, which is also where the rule
/// itself lives so the physics fixture judges THIS code and not a lookalike).
///
/// ## Why 5, and why this instead of arc screening
///
/// Both compute the SAME integral — uniform in angle, which for a line source is
/// uniform in length weighted by the `dl/r²` its elements actually contribute —
/// and differ only in quadrature rule: uniform-`N` here, skyline-driven adaptive
/// in `arc_screening`. Measured 2026-08-05, ONE binary, both arms of each tile
/// back to back with the cheap arm on both sides of the expensive one so the
/// box's drifting load cancels (CPU-seconds, never wall; sandwich drift ≤3 %),
/// scored against an `N = 9` reference painted by the same binary:
///
/// ```text
///             N=1 + arc (was)     N=5, no arc (is)    cheaper   closer
///   praha     28542 s  0.77 dB     7443 s  0.42 dB     3.83×    1.84×
///   suburb     7596 s  0.40 dB     1960 s  0.15 dB     3.88×    2.62×
///   d1open     4585 s  0.43 dB     1096 s  0.14 dB     4.18×    2.99×
///   rail2206   5703 s  0.34 dB     1243 s  0.06 dB     4.59×    5.24×
/// ```
///
/// Pareto-dominant on all four: cheaper AND closer, together 3.95× the cost for
/// 1.8-5.2× the accuracy. The two methods also OVERLAP rather than add
/// (r = 0.67-0.80 between their corrections, same sign on 82-97 % of the pixels
/// where either moves >0.5 dB), so running both is worse than `N = 5` alone at
/// 2.5-3× the price — at the WHOLE-SEGMENT granularity that measurement composed
/// them at. Arc screening therefore shipped OFF from 2026-08-05 to 2026-08-08,
/// and it had to stay reachable because it still owned the near-field TAIL this
/// rule does not. It is now back ON at BUCKET granularity and only where the
/// bucket is wide (`seg_sampling::SEG_ARC_MIN_SPAN_RAD`), which is the +6.6-8.6 %
/// the overlap argument above does not price: composing at the segment level pays
/// for the far pairs that carry a tile's cost, composing per bucket does not.
///
/// ## Why 5 and not another number, and why not per-tile
///
/// From the same four tiles, scored against the `N = 17` arm so the reference's
/// own error does not flatter the answer (RMS dB):
///
/// ```text
///              N=1     N=5     N=9   fitted p   N=3 (from p)
///   praha     1.516   0.362   0.206    0.89        0.57
///   suburb    0.576   0.132   0.081    0.91        0.21
///   d1open    0.588   0.122   0.081    0.98        0.20
///   rail2206  0.355   0.059   0.042    1.12        0.10
/// ```
///
/// `p ≈ 1`: the error is LINEAR in bucket width, on every tile. That fixes both
/// ends. Not fewer — `N = 3` gives Praha back 0.57 dB, over half of what `N = 1`
/// had, for 40 % of the saving. Not more — `N = 9` costs 1.7× `N = 5` to buy
/// 0.16 dB on the worst tile and 0.04-0.06 dB on the rest, all of it under both
/// the 1.0 dB output LSB and the ≤1.5 dB the energy-budget skip above already
/// admits, i.e. tuning below the pipeline's own noise floor.
///
/// ONE FIXED `N`, NOT A DENSITY RULE, and the numbers are the reason. `N = 5`
/// removes 76 / 77 / 79 / 83 % of each tile's own single-ray error — a 1.4×
/// spread — while the single-ray error itself spreads 4.3× across the same four
/// tiles. The METHOD behaves the same everywhere; what varies is the PROBLEM. A
/// per-tile `N` would need a paint-time predictor of `e(1)`, and four tiles
/// cannot fit one (`loaded_rows` orders them wrong: rail2206 has 22× fewer rows
/// than Praha and 1.5× fewer than d1open, yet sits between them on error).
///
/// WHERE THE RESIDUAL LIVED, and how it was closed (2026-08-08): wide-span pairs.
/// `screening_fixture` scenes A-J stand 45-445 m from the source line, where a
/// 250 m microsegment spans up to 2.4 rad and five buckets are 28° wide — there
/// `N = 5` reached 2.4-7.6 dB max error while arc screening held 0.6-0.9 dB. On a
/// whole tile those pairs are a small pixel share (2 % of Praha over 1 dB), which
/// is why the tile RMS favoured this rule anyway.
///
/// The note here used to propose capping the BUCKET WIDTH (`N = clamp(ceil(span /
/// Δ), 1, N_max)`, linear by `p ≈ 1`). That was built and swept, and **it is the
/// wrong lever** — `p ≈ 1` governs the MEAN, and the tail is not a mean. `N`
/// itself is non-monotone in the worst receiver (scene F: 2.17 dB at `N = 5`,
/// 8.21 at `N = 6`), because uniform nodes alias against a building row; capping
/// the width only picks a different `N`. `seg_sampling`'s module docs carry the
/// full sweep for both that lever and a disagreement-driven refinement, and the
/// reason neither reaches the tail. What closed it is
/// [`tile_arc_bounds`] — geometry-placed nodes, per bucket, gated on width.
///
/// ## Lane divergence — read before comparing against CUDA
///
/// The CUDA surface kernel (`engine/noise-gpu/kernels/scatter.cu`) has NO
/// equivalent: it arc-screens one cp ray per microsegment. With this default the
/// two lanes therefore compute different physics, by the full 1.5 dB Praha gap.
/// Any CPU-vs-GPU tile comparison must run the CPU with `QM_SEG_SAMPLES=1
/// QM_ARC_MIN_SPAN_DEG=0`, which restores the pre-2026-08-05 kernel exactly.
pub(crate) fn seg_samples() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("QM_SEG_SAMPLES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(SEG_SAMPLES_DEFAULT)
    })
}

/// Buckets per microsegment fan when `QM_SEG_SAMPLES` is unset — see
/// [`seg_samples`] for the four-tile measurement this comes from.
const SEG_SAMPLES_DEFAULT: usize = 5;

/// Arc-screening bounds for the TILE path — [`seg_arc_bounds`], cached.
///
/// Arc screening is not superseded here, it is TARGETED: from 2026-08-05 to
/// 2026-08-08 this returned `min_span_rad = ∞` (never arc-screen), because the
/// uniform quadrature measured cheaper and closer on all four tiles. It also left
/// a 2.4-7.6 dB tail on the near-field fixture scenes that no amount of uniform
/// nodes removes — `seg_sampling`'s module docs carry that measurement and the
/// two rejected fixes. What ships now is one gate: a bucket wider than
/// `seg_sampling::SEG_ARC_MIN_SPAN_RAD` arc-screens its own sub-span, a narrower one does not,
/// which is +6.6-8.6 % CPU on these tiles and puts every fixture receiver inside
/// the owner's 1.0 dB.
///
/// `QM_ARC_MIN_SPAN_DEG` still overrides every reader of those bounds, so
/// `QM_ARC_MIN_SPAN_DEG=0` is arc screening at its own shipped threshold and
/// `=180` is the 2026-08-05 rule. The popup kernels (`compute::roads`,
/// `compute::railways`) never come through here and keep arc screening on their
/// single cp ray unconditionally: the popup is the accuracy etalon and one
/// receiver can afford the adaptive rule.
///
/// Note this feeds BOTH the quadrature's per-bucket query and the cp-ray fallback
/// below it (branch 2, the no-vector-store lane), which is deliberate — the
/// fallback is a whole-span cp ray, so the same width test is the right one.
pub(crate) fn tile_arc_bounds() -> ArcBounds {
    static V: OnceLock<ArcBounds> = OnceLock::new();
    *V.get_or_init(seg_arc_bounds)
}

/// The all-zero band array the quadrature is handed for `cp_screening`, which
/// it does not read — every bucket carries its own ray, which is exactly why the
/// caller may skip the cp screening evaluation.
const ZERO_BANDS: [f64; NUM_BANDS] = [0.0; NUM_BANDS];

/// The one `ArcScreening` query for a (microsegment, receiver) pair, built once
/// and handed to whichever quadrature runs — the uniform one
/// ([`sampled_gob_bands`]) or the adaptive one (`arc_screened_attenuation`).
/// They take the SAME query because they compute the same integral; keeping one
/// builder is what stops the two arms drifting apart in a field nobody looked at.
#[allow(clippy::too_many_arguments)]
fn arc_query<'a>(
    arc: &ArcSegment,
    t: &PixelTerms,
    rx_lat: f64,
    rx_lon: f64,
    rx_alt: f64,
    cp_screening: &'a [f64; NUM_BANDS],
    cp_terrain: &'a [f64; NUM_BANDS],
    ground_g: f64,
    barriers: &'a [Barrier],
    obstacles: &'a ObstacleSet,
    bounds: ArcBounds,
) -> ArcScreening<'a> {
    ArcScreening {
        receiver_lat: rx_lat,
        receiver_lon: rx_lon,
        receiver_alt_m: rx_alt,
        start_lat: arc.start_lat,
        start_lon: arc.start_lon,
        end_lat: arc.end_lat,
        end_lon: arc.end_lon,
        source_height_m: arc.source_height_m,
        cp_lat: t.cp_lat,
        cp_lon: t.cp_lon,
        src_alt_m: t.src_alt,
        cp_screening,
        cp_terrain,
        ground_g,
        barriers,
        obstacles,
        length_m: arc.length_m,
        dist_m: arc.dist_m,
        // Line sources never self-screen (the CPU kernels pass 0 here too).
        exclusion_radius_m: t.excl_m,
        bounds,
    }
}

/// How a pixel's ground attenuation coefficient `G` is resolved AFTER the path
/// profile is built — the one branch where the line and point ground models
/// diverge. Line path-averages the profile (hard `G=0` on a bridge); point
/// samples the receiver's `ground_g` (the popup oracle samples it once at the
/// receiver, not along the path).
pub(crate) enum GroundSrc {
    /// `path_effects::ground_g_from_profile(&profile)` — the line's path average.
    FromProfile,
    /// A pixel-resolved constant — the line's bridge `0.0`.
    Fixed(f64),
    /// `tile.ground_g(rx_lat, rx_lon)` — the point's receiver-sampled `G` (oracle
    /// parity). Resolved by the kernel AFTER the budget skip (like the original
    /// point loop), so a skipped pixel never pays for the raster lookup; the value
    /// is a pure lat/lon function so deferring it is byte-identical.
    ReceiverSampled,
}

/// Everything the generic band loop needs from a (source, receiver) pair that the
/// per-geometry [`PixelGeometry::pixel`] computes. The shared kernel folds these
/// into the budget bound, the path build, and the `max(A_gr, A_bar)` assembly
/// identically for line and point — the divergence law, FLC, exclusion, ground
/// model, and profile sample point are all already baked in here.
pub(crate) struct PixelTerms {
    /// Distance/divergence/FLC/reflection folded to a pre-attenuation dB level
    /// (`refl + flc − geo_div` for line; `refl − geo_div` for point).
    pub(crate) base_db: f64,
    /// Slant distance in km for atmospheric absorption (`d_slant / 1000`).
    pub(crate) atm_d_km: f64,
    /// Horizontal distance passed to `build_surface_profile` (the ray length):
    /// line = `d_endpoint_m`; point = the PRE-exclusion flat `dist_m` (NOT the
    /// exclusion-shrunk `prop_dist`, which only feeds the divergence `d_slant`).
    pub(crate) profile_dist_m: f64,
    /// Source altitude (ground elevation + source height) for terrain/screening.
    pub(crate) src_alt: f64,
    /// `exclusion_radius_m` passed to `screening_attenuation` so footprint
    /// buildings aren't a barrier: line `0.0`, point `exclusion_radius_m`.
    pub(crate) excl_m: f64,
    /// Source-side lat/lon the profile is sampled from: line = the segment foot
    /// `cp_lat/cp_lon`; point = the source `lat/lon`.
    pub(crate) cp_lat: f64,
    pub(crate) cp_lon: f64,
    /// How `G` is resolved after the profile is built (line vs point ground model).
    pub(crate) ground_src: GroundSrc,
    /// The LINE geometry's arc-screening query (fix-pack Fix 1); `None` for the
    /// point kernels, which have no angular span to clip.
    pub(crate) arc: Option<ArcSegment>,
}

/// The microsegment an arc-screening query runs over — everything
/// `arc_screening` needs beyond what [`PixelTerms`] already carries.
///
/// Line sources only: a point source IS its characteristic point, so its cp-ray
/// verdict already covers every direction it radiates in.
pub(crate) struct ArcSegment {
    pub(crate) start_lat: f64,
    pub(crate) start_lon: f64,
    pub(crate) end_lat: f64,
    pub(crate) end_lon: f64,
    /// Source height above ground, applied to the DEM at every interpolated
    /// source point along the segment.
    pub(crate) source_height_m: f64,
    /// Segment length and the receiver's distance to its nearest point — the
    /// two numbers `segment_can_span` needs to skip the whole arc query, atan2
    /// included, for a segment too narrow at this receiver to stripe.
    pub(crate) length_m: f64,
    pub(crate) dist_m: f64,
}

/// A prepared source's tile-pixel reach box + per-period emission, exposed
/// generically so the band loop clips and accumulates without knowing whether the
/// source is a line or a point. The bbox/emission `prepare` phase stays
/// geometry-specific (line uses segment extents + a widest-segment latitude,
/// point a centre + precomputed source altitude); only the post-clip band body is
/// unified.
pub(crate) trait PreparedSource {
    /// Reach-box top/bottom/left/right tile-pixel bounds (inclusive).
    fn reach_box(&self) -> (usize, usize, usize, usize);
    /// `[period][band]` linear A-unweighted band energy (the accumulation factor).
    fn emission_lin(&self) -> &[[f32; NUM_BANDS]; NUM_PERIODS];
    /// Lden-weighted band spectrum for the budget upper bound.
    fn emission_lden(&self) -> &[f64; NUM_BANDS];
}

/// The per-pixel geometry divergence between the line and point kernels. One
/// `Prep` row is borrowed by [`Self::pixel`] for every receiver in its reach box;
/// returning `None` culls the pixel (out of reach, or below the point's
/// free-field audibility floor) BEFORE any budget/path work.
pub(crate) trait PixelGeometry: Sync {
    /// Per-source prepared row (reach box, emission, and any pixel-independent
    /// hoisted state like the point's source altitude). `Sync` because the
    /// rayon block-fold borrows `&[Prep]` across worker threads.
    type Prep: PreparedSource + Sync;

    /// Prepare every in-reach, emitting source's reach box + emission, pushing
    /// each onto `prep` (silent or wholly-out-of-tile sources drop).
    fn prepare(&self, tile: &FusedTileZ13, prep: &mut Vec<Self::Prep>);

    /// Turn one (source, receiver) pair into the shared propagation terms, or
    /// `None` to cull this pixel. `rx_lat/rx_lon` are the receiver pixel centre;
    /// `rx_alt` its pre-baked altitude; `refl` its reflection dB.
    fn pixel(
        &self,
        prep: &Self::Prep,
        tile: &FusedTileZ13,
        rx_lat: f64,
        rx_lon: f64,
        rx_alt: f64,
        refl: f64,
    ) -> Option<PixelTerms>;
}

/// Telemetry returned by the generic scatter (line and point share the shape).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ScatterStats {
    pub(crate) rows: usize,
    pub(crate) path_calls: u64,
    pub(crate) skipped_calls: u64,
    /// Ray-march cadence samples (×4 = raster cell reads). See [`BandScratch`].
    pub(crate) raster_samples: u64,
}

/// Scatter every prepared source onto `tile` via the geometry `geo`, with the
/// coarse-middle cadence read from the process-wide env (`coarse_mid_cfg`).
#[allow(dead_code)]
pub(crate) fn scatter_tile<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    n_rows: usize,
    accum: &mut TileAccumulator,
) -> ScatterStats {
    scatter_tile_with_cfg(
        geo,
        tile,
        barriers,
        obstacles,
        n_rows,
        accum,
        coarse_mid_cfg(),
    )
}

/// [`scatter_tile`] with the coarse-middle cadence passed EXPLICITLY (bypassing
/// the process-wide `coarse_mid_cfg` env read). The noise-floor harness uses this
/// to render the exact (`None`) and coarse fields in ONE process; production uses
/// [`scatter_tile`].
pub(crate) fn scatter_tile_with_cfg<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    n_rows: usize,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
) -> ScatterStats {
    if n_rows == 0 {
        return ScatterStats::default();
    }
    let mut prep: Vec<G::Prep> = Vec::new();
    geo.prepare(tile, &mut prep);
    if prep.is_empty() {
        return ScatterStats {
            rows: n_rows,
            ..Default::default()
        };
    }

    let eta = budget_eta();
    let (merged, path_calls, skipped_calls, raster_samples) = recv_block_regions()
        .into_par_iter()
        .fold(BandScratch::new, |mut s, (py_lo, py_hi, px_lo, px_hi)| {
            if py_lo < py_hi && px_lo < px_hi {
                scatter_band(
                    geo, tile, &prep, barriers, obstacles, py_lo, py_hi, px_lo, px_hi, eta, cfg,
                    &mut s,
                );
            }
            s
        })
        .map(|s| (s.local, s.path_calls, s.skipped_calls, s.raster_samples))
        .reduce(
            || (TileAccumulator::new(), 0u64, 0u64, 0u64),
            |mut a, b| {
                a.0.merge_from(&b.0);
                (a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3)
            },
        );
    accum.merge_from(&merged);
    ScatterStats {
        rows: n_rows,
        path_calls,
        skipped_calls,
        raster_samples,
    }
}

/// Scatter every source that reaches the block `[py_lo, py_hi) × [px_lo, px_hi)`
/// into its pixels, applying the per-pixel energy-budget skip. The single hot
/// loop both line and point share; the per-pixel geometry is `geo.pixel`.
#[allow(clippy::too_many_arguments)]
fn scatter_band<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    prep: &[G::Prep],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    py_lo: usize,
    py_hi: usize,
    px_lo: usize,
    px_hi: usize,
    eta: f64,
    cfg: Option<CoarseMid>,
    s: &mut BandScratch,
) {
    // One skyline slot per pixel of this block, all stale — the previous block's
    // receivers are gone.
    let block_w = px_hi - px_lo;
    s.skylines
        .resize_with(block_w * (py_hi - py_lo), ArcSkyline::default);
    for sk in s.skylines.iter_mut() {
        sk.reset();
    }
    let n_seg = seg_samples();
    for pr in prep {
        let (rpy0, rpy1, rpx0, rpx1) = pr.reach_box();
        let py0 = rpy0.max(py_lo);
        let py1 = rpy1.min(py_hi - 1);
        if py0 > py1 {
            continue;
        }
        let px0 = rpx0.max(px_lo);
        let px1 = rpx1.min(px_hi - 1);
        if px0 > px1 {
            continue;
        }
        let emission_lin = pr.emission_lin();
        let emission_lden = pr.emission_lden();

        for py in py0..=py1 {
            let rx_lat = tile.rx_lat[py];
            let row_base = py * TILE_PX;
            for px in px0..=px1 {
                let rx_lon = tile.rx_lon[px];
                let idx = row_base + px;
                let rx_alt = tile.rx_alt_m[idx] as f64;
                let refl = tile.rx_refl_db[idx] as f64;
                let Some(t) = geo.pixel(pr, tile, rx_lat, rx_lon, rx_alt, refl) else {
                    continue;
                };

                let ub_lden = budget_ub_lden(t.base_db, t.atm_d_km, emission_lden);
                if s.skipped[idx] + ub_lden <= eta * s.kept[idx] {
                    s.skipped[idx] += ub_lden;
                    s.skipped_calls += 1;
                    continue;
                }

                build_surface_profile(
                    tile,
                    cfg,
                    t.cp_lat,
                    t.cp_lon,
                    rx_lat,
                    rx_lon,
                    t.profile_dist_m,
                    &mut s.profile,
                );
                s.path_calls += 1;
                s.raster_samples += s.profile.len() as u64;
                let ground_g = match t.ground_src {
                    GroundSrc::FromProfile => path_effects::ground_g_from_profile(&s.profile),
                    GroundSrc::Fixed(g) => g,
                    GroundSrc::ReceiverSampled => tile.ground_g(rx_lat, rx_lon),
                };
                // Heatmap discards the popup obstacle traces, so call the
                // metadata-free band-only variants: terrain skips the per-pixel
                // EdgePoint Vec, screening skips the ObstacleEdge materialisation.
                let terrain_bands =
                    path_effects::terrain_attenuation(&mut s.profile, t.src_alt, rx_alt);
                // ── the ground/barrier term, ISO 9613-2 §7.3.1 ──────────────
                // `max(A_ground, A_terrain + A_screen)`: a barrier REPLACES
                // ground, never adds. Two ways to get it for a LINE segment,
                // which subtends an angle at this receiver and screens
                // differently across it, and one for everything else.
                let slot = (py - py_lo) * block_w + (px - px_lo);
                // (1) UNIFORM ANGULAR QUADRATURE ([`seg_samples`], the default):
                // N bucket rays across the fan, energy-averaged. Tried FIRST
                // because when it applies the cp ray's own screening is dead
                // work — so the pair costs N rays, not N+1. The cp PROFILE still
                // runs above: ground-G, terrain and vegetation come off it
                // either way. `None` = no fan (a degenerate segment) or no
                // vector store (a raster-fallback region), and then (2) below
                // is what runs, exactly as it always did.
                let sampled = match (&t.arc, obstacles) {
                    (Some(arc), Some(set)) if n_seg > 1 => {
                        let query = arc_query(
                            arc,
                            &t,
                            rx_lat,
                            rx_lon,
                            rx_alt,
                            &ZERO_BANDS,
                            &terrain_bands,
                            ground_g,
                            barriers,
                            set,
                            s.arc_bounds,
                        );
                        let BandScratch {
                            skylines,
                            seg_scratch,
                            ..
                        } = &mut *s;
                        sampled_gob_bands(
                            &query,
                            &SurfaceCadenceRasters { tile, cfg },
                            n_seg,
                            &mut skylines[slot],
                            seg_scratch,
                        )
                    }
                    _ => None,
                };
                let gob_bands = match sampled {
                    Some((gob, cost)) => {
                        s.path_calls += cost.rays;
                        s.raster_samples += cost.raster_samples;
                        gob
                    }
                    // (2) ONE CHARACTERISTIC-POINT RAY, optionally arc-clipped
                    // (fix-pack Fix 1, the tile twin of the popup's
                    // `arc_screened_line_segment`): the cp verdict covers only
                    // the directions that ray flies through, so the rest of the
                    // span gets its own evaluation, energy-averaged over the
                    // blocked fractions. Gated on the segment's own width since
                    // 2026-08-08 (see [`tile_arc_bounds`]); a segment too narrow
                    // to stripe keeps the plain cp verdict.
                    None => {
                        let obstacle_input = match obstacles {
                            Some(set) => {
                                set.crossings_pruned(
                                    t.cp_lat,
                                    t.cp_lon,
                                    rx_lat,
                                    rx_lon,
                                    &CellPrune::for_profile(&s.profile, t.src_alt, rx_alt),
                                    &mut s.cand_scratch,
                                );
                                path_effects::ObstacleInput {
                                    candidates: &s.cand_scratch,
                                    replace_sample_buildings: true,
                                }
                            }
                            None => path_effects::ObstacleInput::CANDIDATES_OFF,
                        };
                        let cp_screening = path_effects::screening_attenuation(
                            &mut s.profile,
                            barriers,
                            obstacle_input,
                            t.src_alt,
                            rx_alt,
                            t.excl_m,
                            &terrain_bands,
                        );
                        let screening = match (&t.arc, obstacles) {
                            (Some(arc), Some(set))
                                if segment_can_span(arc.length_m, arc.dist_m, s.arc_bounds) =>
                            {
                                let query = arc_query(
                                    arc,
                                    &t,
                                    rx_lat,
                                    rx_lon,
                                    rx_alt,
                                    &cp_screening,
                                    &terrain_bands,
                                    ground_g,
                                    barriers,
                                    set,
                                    s.arc_bounds,
                                );
                                arc_screened_attenuation(
                                    &query,
                                    &SurfaceCadenceRasters { tile, cfg },
                                    &mut s.skylines[slot],
                                    &mut s.arc_scratch,
                                )
                            }
                            _ => cp_screening,
                        };
                        let mut gob = [0.0f64; NUM_BANDS];
                        for (i, g) in gob.iter_mut().enumerate() {
                            *g = ground_or_barrier_db(
                                ground_atten_db(i, ground_g),
                                terrain_bands[i],
                                screening[i],
                            );
                        }
                        gob
                    }
                };
                let veg = path_effects::vegetation_attenuation_path(&s.profile);

                // Period-independent per-band path factor (A-weighted linear).
                let mut pf = [0.0f64; NUM_BANDS];
                for i in 0..NUM_BANDS {
                    let path_db = t.base_db - ALPHA_ATM[i] * t.atm_d_km - gob_bands[i] - veg[i];
                    pf[i] = db_to_lin_a(path_db, i);
                }

                let mut kept_add = 0.0;
                for p in 0..NUM_PERIODS {
                    let mut power = 0.0f64;
                    for i in 0..NUM_BANDS {
                        power += emission_lin[p][i] as f64 * pf[i];
                    }
                    if power.is_finite() && power > 0.0 {
                        s.local
                            .add_energy_at(py as u32, px as u32, p as u8, power as f32);
                        kept_add += power * LDEN_WEIGHTS[p];
                    }
                }
                s.kept[idx] += kept_add;
            }
        }
    }
}

/// Tile pixel row for a latitude (linear in the Mercator bbox, matching
/// `FusedTileZ13::latlon_to_inner_idx`); clamped to `[0, TILE_PX)`. Shared by all
/// three surface scatter kernels for the reach-bbox clip.
#[inline]
pub(crate) fn lat_to_py(bbox: &TileBbox, lat: f64) -> usize {
    let frac = (bbox.north_lat - lat) / (bbox.north_lat - bbox.south_lat);
    (frac * TILE_PX as f64)
        .floor()
        .clamp(0.0, (TILE_PX - 1) as f64) as usize
}

#[inline]
pub(crate) fn lon_to_px(bbox: &TileBbox, lon: f64) -> usize {
    let frac = (lon - bbox.west_lon) / (bbox.east_lon - bbox.west_lon);
    (frac * TILE_PX as f64)
        .floor()
        .clamp(0.0, (TILE_PX - 1) as f64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::propagation::path_profile::{fill_t_values, fill_t_values_coarse_mid};

    /// The cadence the production build ships (env-free).
    fn shipped() -> CoarseMid {
        CoarseMid {
            src_zone_m: SHADOW_SRC_ZONE_M,
            rx_zone_m: SHADOW_RX_ZONE_M,
            mid_stride: SHADOW_MID_STRIDE,
        }
    }

    /// The t-values the surface heatmap ends up sampling for a ray of `dist_m`,
    /// i.e. `build_surface_profile`'s cadence decision made visible.
    fn heatmap_t(cfg: Option<CoarseMid>, dist_m: f64) -> Vec<f64> {
        let mut t = Vec::new();
        match cadence_for_ray(cfg, dist_m) {
            Some(cm) => fill_t_values_coarse_mid(dist_m, &mut t, cm),
            None => fill_t_values(dist_m, &mut t),
        }
        t
    }

    /// The popup's cadence for the same ray.
    fn popup_t(dist_m: f64) -> Vec<f64> {
        let mut t = Vec::new();
        fill_t_values(dist_m, &mut t);
        t
    }

    /// Fix 5, the whole contract: at or below [`EXACT_CADENCE_MAX_DIST_M`] the
    /// heatmap samples EXACTLY where the popup samples; beyond it the coarse
    /// middle is back on. Holds for the shipped zones AND for a tightened zone
    /// (the earlier 200 m default, still reachable via the env overrides).
    #[test]
    fn near_field_cadence_matches_the_popup_beyond_it_stays_coarse() {
        let tight = CoarseMid {
            src_zone_m: 200.0,
            rx_zone_m: 200.0,
            mid_stride: SHADOW_MID_STRIDE,
        };
        for cfg in [shipped(), tight] {
            for d in [
                1.0,
                25.0,
                120.0,
                300.0,
                320.0,
                350.0,
                EXACT_CADENCE_MAX_DIST_M,
            ] {
                assert!(
                    cadence_for_ray(Some(cfg), d).is_none(),
                    "d={d} must run the exact cadence"
                );
                assert_eq!(heatmap_t(Some(cfg), d), popup_t(d), "d={d}: popup parity");
            }
            for d in [400.1, 700.0, 3000.0, 10_000.0] {
                assert!(
                    cadence_for_ray(Some(cfg), d).is_some(),
                    "d={d} must keep the coarse middle"
                );
            }
        }
        // The exact-reference config (SURFACE_SHADOW_STRIDE=1 ⇒ `None`) is
        // untouched by the gate: exact everywhere, as before.
        assert!(cadence_for_ray(None, 10_000.0).is_none());
    }

    /// The gate is load-bearing, not decorative: UNGATED, the coarse builder
    /// really does diverge from the popup inside the 400 m band — a ~320 m ray
    /// gains a middle sample the popup has not got (the coarse builder bridges
    /// its middle from the last committed ramp sample, the exact one from the
    /// midpoint), and a tightened zone truncates the ramp itself at 500 m.
    #[test]
    fn ungated_coarse_cadence_diverges_inside_the_near_field() {
        let tight = CoarseMid {
            src_zone_m: 200.0,
            rx_zone_m: 200.0,
            mid_stride: SHADOW_MID_STRIDE,
        };
        let mut ungated = Vec::new();
        fill_t_values_coarse_mid(320.0, &mut ungated, shipped());
        assert_ne!(
            ungated,
            popup_t(320.0),
            "shipped zones still diverge at 320 m without the gate"
        );
        let mut ungated = Vec::new();
        fill_t_values_coarse_mid(500.0, &mut ungated, tight);
        assert_ne!(
            ungated,
            popup_t(500.0),
            "a tightened zone truncates the near-field ramp at 500 m"
        );
    }

    /// Beyond the gate the coarse middle must still be the speed win it was
    /// tuned to be — fewer samples than the exact cadence on a long ray.
    #[test]
    fn far_field_still_subsamples() {
        let d = 10_000.0;
        assert!(
            heatmap_t(Some(shipped()), d).len() < popup_t(d).len(),
            "the coarse middle must still cut samples on a 10 km ray"
        );
    }
}
