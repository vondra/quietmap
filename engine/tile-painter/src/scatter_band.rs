//! Generic surface-scatter kernel for the road/rail line sources
//! ([`crate::scatter_line`], via its `LineGeometry`) and the industrial/building
//! point sources ([`crate::scatter_point`], via its `PointGeometry`). Both use
//! the SAME receiver-block structure, terrain ray-march, `max(A_gr, A_bar)`
//! path assembly, and 3-period accumulation; they differ ONLY in the per-pixel
//! geometry that turns a (source, receiver) pair into the propagation terms.
//! The ordinary line and byte-stopped point paths are receiver-major inside a
//! block and use the byte-space stop. The full-tile exact point path
//! (`SURFACE_BUDGET_ETA=0`) is a source-major bypass that computes every
//! admitted pair, with no stop or bound sort. That traversal divergence is kept
//! behind the
//! [`PixelGeometry`] trait and one shared evaluator, so the physics stays one
//! kernel while the point-only ordering optimisation remains explicit.
//!
//! What stays per-geometry (the [`PixelGeometry::pixel`] return [`PixelTerms`]):
//!  * divergence law — line is ISO 9613-2 CYLINDRICAL `10·log10(2π·d_slant)`,
//!    point is SPHERICAL `20·log10(d)+11`;
//!  * the line's finite-line correction (folded into `base_db`) vs the point's
//!    free-field audibility pre-gate (a real per-pixel cull — `pixel` returns
//!    `None`) and its exclusion radius (effective distance + screening exclusion);
//!  * the ground model — both geometries derive CNOSSOS ground from the shared
//!    path profile; line sources additionally force hard `G=0` on a bridge;
//!  * the profile sample point — line uses the segment foot, point the source.
//!
//! ground-ops ([`crate::ground_ops`]) shares the machinery (the [`BandScratch`]
//! and the helpers below) but NOT this kernel: it has per-row event weights, a
//! mixed-geometry skip bound, and a different Lden collapse, so its band body
//! stays its own.
//!
//! ## Stopping in byte space (receiver-block ownership)
//!
//! Most far/quiet sources cannot move a pixel a louder near source already
//! dominates — computing their exact terrain/diffraction path is wasted work the
//! OUTPUT cannot resolve. And the output is one `u8 × 0.5 dB` cell, so a pixel is
//! finished once its BYTE is pinned, not once its value is. Per pixel the kernel
//! therefore keeps an interval — `P⁻` exactly computed, `P⁺` adding the cheap
//! free-field bound ([`budget_ub_lden`], provably ≥ exact) over everything not yet
//! computed — and stops the moment both ends quantise to the same byte
//! ([`crate::byte_stop`]). Every completion of the tail then writes the same
//! cell, so the stop is EXACT: bit-identical output, with the exact per-pair path
//! as the fallback that guarantees termination.
//!
//! This REPLACED an energy-budget skip (`skipped ≤ η·kept`, η = 0.40, 2026-08),
//! and the replacement is a bug fix before it is an optimisation. That test ran
//! against a `kept` which starts at zero and only grows, so it (a) gave a
//! different answer for a different source-load order — measured on dense Praha,
//! two orders keep different source sets, ~75 k pairs apart — (b) admitted up to
//! `10·log10(1+η) = 1.46 dB` of energy never counted at all (NoiseModelling's
//! comparable default is 0.1 dB, 15× tighter), and (c) was one of the standing
//! CPU↔GPU discrepancies. An interval in byte space has none of the three: it
//! never drops energy that could change the answer, so the answer no longer
//! depends on order (`tests::source_order_never_changes_the_answer`).
//!
//! It also strictly dominates the old rule where the old rule was weakest. An
//! isolated rural dwelling reached by one far road has no louder source to mask
//! it, so `kept` stayed ~0 and η never skipped anything; the interval closes at
//! pair ZERO whenever the pixel's whole remaining bound is still under the 0 dB
//! NO_DATA floor.
//!
//! The scatter is parallelised over receiver BLOCKS (not over sources): one
//! block owns a square pixel rectangle ([`recv_block_regions`]). In the ordinary
//! path, each pixel then walks every source clipped to the block (receiver-major)
//! so its interval owns the complete tail. With byte stopping disabled for a
//! full-tile point scatter, the point arm instead walks each prepared source
//! across the block's receivers (source-major); point paths have no
//! [`ArcSkyline`] fan whose result depends on pair order, and each receiver is
//! still accumulated in prepared source-load order.
//!
//! In the ordinary path, PIXEL-MAJOR INSIDE THE BLOCK and cheapest-bound-first
//! order are load-bearing for COST but never for the ANSWER. Pairs are
//! accumulated into the tile in source-load order regardless
//! (`BandScratch::pair_pow`), because the f32 accumulator and the
//! [`ArcSkyline`]'s per-sector growth are mildly order-sensitive and a pixel
//! that never closes must stay bit-identical to a kernel with no stopping at
//! all. The exact point bypass has no tail to stop and writes each pair directly
//! in that same source-load order.

use std::f64::consts::LN_10;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use noise_compute::constants::{ALPHA_ATM, A_WEIGHTING};
use noise_compute::propagation::arc_screening::{
    arc_screened_attenuation_with_ground, segment_can_span, ArcBounds, ArcScreening,
    ArcScreeningScratch, ArcSkyline,
};
use noise_compute::propagation::census;
use noise_compute::propagation::iso9613::{fast_exp_f64, ground_atten_bands, ground_or_barrier_db};
use noise_compute::propagation::obstacle_index::{
    CellPrune, CrossingCandidate, CrossingScratch, ObstacleSet,
};
use noise_compute::propagation::path_effects;
use noise_compute::propagation::path_profile::CoarseMid;
use noise_compute::propagation::seg_sampling::{
    sampled_gob_bands_with_ground, seg_arc_bounds, SegSampleScratch,
};
use noise_compute::propagation::PathProfile;
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBbox, TILE_PX};
use rayon::prelude::*;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::byte_stop;

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

/// Legacy η control slot shared by the surface and ground-ops byte-space stop.
/// A nonzero value enables the exact byte-space interval; zero disables it.
/// For a full-tile point scatter, zero additionally selects the source-major
/// exact bypass, while line, selected-receiver, and direct-surrogate paths stay
/// receiver-major. The numeric value remains part of the existing GPU metadata
/// contract, but these CPU kernels use only whether it is zero or nonzero.
///
/// The stop's claim is bit-identical output, and the only honest way to check
/// that is to paint both arms with one build and diff the bytes. It rides the η
/// env rather than getting one of its own because one variable has to put both
/// lanes on the exact path: the CUDA twin reads ON/OFF out of `meta[9]`, the old
/// η slot (`scatter.cu`'s `line`). With two envs the CPU↔GPU parity gate could
/// compare a stopped kernel against an unstopped one and blame the difference
/// on the GPU. There is nothing to tune either way — an interval in byte space
/// has no tolerance, so an on/off is the whole knob.
pub(crate) fn byte_stop_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("SURFACE_BUDGET_ETA")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|e| e.is_finite() && *e >= 0.0)
            .map(|e| e != 0.0)
            .unwrap_or(true)
    })
}

#[cfg(test)]
static EXACT_POINT_BYPASS_PAIRS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static CACHED_PIXEL_TERM_READS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static POINT_EARLY_STOPS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static POINT_BARRIER_PATHS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static POINT_OBSTACLE_CROSSINGS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn note_exact_point_bypass_pair() {
    #[cfg(test)]
    EXACT_POINT_BYPASS_PAIRS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn note_cached_pixel_term_read() {
    #[cfg(test)]
    CACHED_PIXEL_TERM_READS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn note_early_stop() {
    #[cfg(test)]
    POINT_EARLY_STOPS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn note_barrier_path() {
    #[cfg(test)]
    POINT_BARRIER_PATHS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn note_obstacle_crossing() {
    #[cfg(test)]
    POINT_OBSTACLE_CROSSINGS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
fn reset_point_optimization_counts() {
    EXACT_POINT_BYPASS_PAIRS.store(0, Ordering::Relaxed);
    CACHED_PIXEL_TERM_READS.store(0, Ordering::Relaxed);
    POINT_EARLY_STOPS.store(0, Ordering::Relaxed);
    POINT_BARRIER_PATHS.store(0, Ordering::Relaxed);
    POINT_OBSTACLE_CROSSINGS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
fn point_optimization_counts() -> (u64, u64) {
    (
        EXACT_POINT_BYPASS_PAIRS.load(Ordering::Relaxed),
        CACHED_PIXEL_TERM_READS.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
fn point_execution_counts() -> (u64, u64, u64) {
    (
        POINT_EARLY_STOPS.load(Ordering::Relaxed),
        POINT_BARRIER_PATHS.load(Ordering::Relaxed),
        POINT_OBSTACLE_CROSSINGS.load(Ordering::Relaxed),
    )
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
/// stride 4 adds little for the same accuracy.
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

/// Best-case Lden energy of a source→pixel pair — the byte-space stop's `P⁺`
/// per pair: no terrain/screening/veg + the most favourable ground any band can
/// meet, tightened per pair by [`crate::bound_m3`] (M3: the ground-gain floor
/// from a max-pooled imperviousness pyramid, plus a cp-ray terrain lower bound
/// where the exact path is a single characteristic-point ray — see the doc
/// block below and `bound_m3.rs` for the soundness arguments), inflated by
/// [`UB_SAFETY`] — provably ≥ the exact contribution. Shared verbatim by the
/// line + point kernels so the `ub ≥ exact` soundness invariant lives in one
/// place. `base_db` already folds in divergence/FLC/reflection; `emission_lden`
/// is the pair's Lden-weighted band spectrum.
///
/// ## How much a TIGHTER bound would buy, and what it costs to stay sound
///
/// This bound is loose by ~22 dB (per-pair median), which is what caps the stop:
/// replayed over 47.1 M recorded pairs from six tiles, walking loudest-bound
/// first closes 51.6 % of the way through the pair list on average (0 byte
/// mismatches). Per-pixel median slack splits GROUND / TERRAIN / OBSTACLE as
/// 1.6-1.7 / 2.2-3.1 / 2.9-4.8 dB in Praha, 4.0-4.1 / 1.4-2.2 / 0.5-1.6 in
/// suburb, 4.6 / 5.8-9.8 / 0.3-0.6 in open country. Perfect ground knowledge
/// alone would take the walk to 45.5 %, perfect ground+terrain to 22.9 %.
/// **Terrain in open country is the lever; buildings outside dense cities are
/// not**, which is why an obstacle-side bound is not attempted here (and a
/// cp-ray one is measured WRONG for a different reason: of pairs with a ≥10 m
/// building on the ray at d > 60 m, 30.05 % are screened by under 0.5 dB,
/// because the arc energy-averages over the whole angular span).
///
/// The terrain lever has a REAL obstacle, found while building it. A K-sample
/// subset of a ray's own cadence gives `max-δ(subset) ≤ max-δ(full)`, and
/// Maekawa is monotone in δ, so a coarse march is a sound LOWER bound on that
/// RAY's terrain attenuation (measured: K = 8 recovers 79-84 % of the terrain
/// dB, 0 violations in 240 k pairs, at ~4-5 % of a pair's cost). But the shipped
/// exact value is not one ray's terrain: with the angular quadrature on
/// ([`seg_samples`] = 5, the default whenever there is a vector obstacle store)
/// `seg_sampling::sampled_gob_bands` composites EACH BUCKET's own terrain —
/// pinned by its `composite_uses_each_buckets_own_terrain` — and a cp-ray march
/// bounds none of the others. Sound options are (a) a coarse march per bucket,
/// `n_seg × K` samples, which the energy mean then averages legitimately, or
/// (b) apply it only where the exact path really is a single cp ray (point
/// sources, raster-fallback regions, `QM_SEG_SAMPLES=1`). Not (c) a cp-ray march
/// used under the quadrature: that reads as tighter and is simply unsound.
///
/// The GROUND lever has no such problem — `ground_g` is one scalar shared by
/// every bucket, so replacing `GROUND_GAIN_UB_DB` with `A_gr(i, G_lo)` for any
/// lower bound `G_lo` is sound for the fan too. It needs a max-pooled
/// imperviousness pyramid to bound the path average without marching (measured:
/// K = 8 chunks recover 56-65 % of the ground dB in open country but only 7-11 %
/// in a city centre, at ~1-2 % of a pair), and **uniform 1/K chunk weights are
/// NOT sound** — the coarse middle throws one 737 m interval across several
/// chunks (7 violations in 40 k). The weights have to be the cadence's own
/// trapezoid mass, which is a closed function of `dist_m` and needs no march.
///
/// Both levers are IMPLEMENTED in [`crate::bound_m3`] (M3): the ground bound
/// is the monotone `−3·(1−g_lo)` floor of the two CNOSSOS states (the
/// analytic term itself is NOT monotone in G — image-source interference — so
/// the floor is the sound reading of `A_gr(i, G_lo)`), and the terrain bound
/// walks a K = 8 subset of the ray's own cadence only on the cp-ray population
/// named above. `SURFACE_BOUND_M3=0` restores this loose bound bit-for-bit.
#[inline]
pub(crate) fn budget_ub_lden(
    base_db: f64,
    atm_d_km: f64,
    emission_lden: &[f64; NUM_BANDS],
    bound: &crate::bound_m3::M3PairBound,
) -> f64 {
    let mut ub = 0.0;
    for i in 0..NUM_BANDS {
        let path_db = base_db - ALPHA_ATM[i] * atm_d_km - bound.gob_lb_db(i);
        ub += emission_lden[i] * db_to_lin_a(path_db, i);
    }
    ub * UB_SAFETY
}

/// Per-worker scatter state, threaded through the blocks one rayon worker folds.
pub(crate) struct BandScratch {
    /// Dense per-worker output for the ordinary block path. The selected W1
    /// path deliberately leaves this absent: its worker output is a sparse
    /// receiver list, not a 512² grid.
    pub(crate) local: Option<TileAccumulator>,
    pub(crate) profile: PathProfile,
    pub(crate) path_calls: u64,
    pub(crate) skipped_calls: u64,
    /// Pairs this worker's cheap pass priced — see [`ScatterStats::pairs`].
    pairs_seen: u64,
    /// Raster cadence samples taken by `build_path_profile` (the ray-march). Each
    /// reads a 4-cell bilinear quad, so cell reads = 4× this — the numerator of
    /// the read-redundancy metric (cell reads ÷ grid cells = ×-reread).
    pub(crate) raster_samples: u64,
    /// Vector-obstacle crossings of the current ray (geodata-v2, reused).
    pub(crate) cand_scratch: Vec<CrossingCandidate>,
    /// Generation-tagged obstacle-edge dedup state reused for every point ray
    /// in this worker. It preserves the exact DDA collision semantics while
    /// avoiding a 64-entry table clear per `(source, receiver)` pair.
    pub(crate) crossing_scratch: CrossingScratch,
    /// Arc-screening (fix-pack Fix 1) interval-ray buffers, amortised across
    /// every (source, pixel) pair this worker folds.
    pub(crate) arc_scratch: ArcScreeningScratch,
    /// The CURRENT receiver's obstacle skyline, reset when the walk moves to the
    /// next pixel. One slot suffices because the walk is pixel-major: a receiver's
    /// whole source list is done before the next receiver starts, so the skyline is
    /// built on the first source that needs it and reused by every later one — the
    /// same "once per receiver" the popup kernels get for free, and the same thing
    /// the CUDA lane gets from thread-per-pixel.
    pub(crate) skyline: ArcSkyline,
    /// Bounds ([`tile_arc_bounds`]) read once per worker, never per pair.
    pub(crate) arc_bounds: ArcBounds,
    /// Bucket-ray buffers for the angular quadrature ([`seg_samples`]). Kept
    /// off `profile`/`cand_scratch` so the cp ray's ground-`G`, terrain and
    /// vegetation stay on the cp profile, exactly as the single-verdict kernel
    /// resolves them (and exactly as `arc_screening` resolves them for its own
    /// interval rays).
    pub(crate) seg_scratch: SegSampleScratch,
    /// Prepared-source indices whose reach box meets the block being folded —
    /// resolved ONCE per block, then re-scanned per pixel. Without it the
    /// pixel-major walk would re-clip all of `prep` at every one of the block's
    /// 256 receivers.
    pairs_cand: Vec<u32>,
    /// The current pixel's pairs and their cheap bounds, sorted loudest-bound
    /// first (see [`PairBound`]).
    pairs: Vec<PairBound>,
    /// `suffix[i]` = Σ bound over `pairs[i..]`, i.e. `P⁺ − P⁻` when the walk
    /// stands at `i`. Built once per pixel from the sorted bounds instead of
    /// decremented along the walk: a running subtraction of a f64 sum loses
    /// digits to cancellation, and a residual that reads too SMALL is an
    /// interval that closes too early — the one way this rule could stop being
    /// exact.
    suffix: Vec<f64>,
    /// Per-period power of each pair the ordinary walk computed exactly, indexed
    /// by the pair's SOURCE-LOAD position, plus a hit flag. That walk visits
    /// pairs loudest-bound-first but the tile is accumulated in source-load
    /// order, so a pixel that never closes lands byte-for-byte where an unstopped
    /// kernel lands it (f32 addition does not commute, and [`ArcSkyline`] growth
    /// is per-sector order-sensitive by its own module's measurement). The exact
    /// point source-major bypass writes directly and does not use this staging.
    pair_pow: Vec<[f32; NUM_PERIODS]>,
    pair_hit: Vec<bool>,
    /// Reused cadence buffer for the M3 per-pair bound (`bound_m3::pair_bound`)
    /// — never allocated in the hot loop.
    bound_t: Vec<f64>,
    /// Point geometry terms computed by the cheap pass, indexed by source-load
    /// position. Keeping this parallel to `PairBound` avoids copying a large
    /// `PixelTerms` through every line-source sort record.
    pair_terms: Vec<PixelTerms>,
    /// Per-(source, block) pooled IMD maxima for the M3a ground bound, indexed
    /// like [`BandScratch::pairs_cand`] — `None` entries mean the source's
    /// profile origin is receiver-dependent (line sources fall back to
    /// per-pair chunk boxes).
    bound_blocks: Vec<Option<crate::bound_m3::BlockGroundMaxima>>,
    /// Pairs the walk actually computed (the M3 payoff census numerator:
    /// walked fraction before/after the tightened bound).
    walked_pairs: u64,
    sparse_output: Vec<(usize, [f32; NUM_PERIODS])>,
}

/// One (source, receiver) pair as the ordinary receiver-major cheap pass
/// records it: which prepared source it came from, where it sat in source-load
/// order, and its free-field Lden bound. The exact point source-major bypass
/// does not materialise these records.
///
/// Sorting these by descending `ub` is what makes the ordinary interval close
/// early — the loudest contributors go in first, so `P⁻` climbs as fast as it
/// can while the tail `P⁺` adds shrinks as fast as it can. It changes only the
/// COST: any order commits the same byte, which is what
/// `tests::source_order_never_changes_the_answer` pins.
///
/// The sort is not a tuning preference, it IS the optimisation. Measured
/// 2026-08-09, one binary, both arms back to back on an idle box (CPU-seconds;
/// the A→B→A sandwich put box drift at ≤0.14 %):
///
/// ```text
///                       pairs skipped        CPU-s     vs the old η=0.40
///   d1open  loudest       49.9 %             4 761        1.17× faster
///           load          11.0 %             8 387        1.51× SLOWER
///   rail    loudest       51.7 %             4 777        1.14× faster
///           load           8.6 %             9 909        1.81× SLOWER
/// ```
///
/// In load order an ordinary pixel's dominant source arrives at a random point in
/// the list, so the interval cannot close until nearly everything has been
/// computed and the kernel comes out SLOWER than the approximate rule it
/// replaced. The exact point arm does not sort at all because it has no
/// order-sensitive skyline.
///
/// The price is stated exactly, and it is not the stop's. Against the pre-stop
/// kernel at η=0, walking in load order is bit-identical (0 of 256 496 cells on
/// rail); walking loudest-first differs at 1 cell on d1open and 29 on rail — RMS
/// 0.0088 dB, 2 cells over 0.5 dB, max 3.5 dB at one cell — because [`ArcSkyline`]
/// grows its per-sector radius on demand and is mildly order-sensitive by its own
/// module's measurement. With the stop OFF and ON, loudest-first paints the same
/// FILE byte for byte on both tiles. It does mean the CPU no longer walks pairs in
/// the order `scatter.cu` does, so a CPU↔GPU comparison is a >0.5 dB / max-dB one
/// until that skyline bookkeeping is order-free.
#[derive(Clone, Copy)]
struct PairBound {
    /// Index into the `prep` slice.
    src: u32,
    /// Position in source-load order — the ACCUMULATION order (see
    /// `BandScratch::pair_pow`).
    ord: u32,
    /// [`budget_ub_lden`] for this pair: provably ≥ its exact Lden contribution.
    ub: f64,
}

impl BandScratch {
    pub(crate) fn new() -> Self {
        Self {
            local: Some(TileAccumulator::new()),
            profile: PathProfile::new(),
            path_calls: 0,
            skipped_calls: 0,
            pairs_seen: 0,
            raster_samples: 0,
            cand_scratch: Vec::new(),
            crossing_scratch: CrossingScratch::default(),
            arc_scratch: ArcScreeningScratch::new(),
            skyline: ArcSkyline::default(),
            arc_bounds: tile_arc_bounds(),
            seg_scratch: SegSampleScratch::new(),
            pairs_cand: Vec::new(),
            pairs: Vec::new(),
            suffix: Vec::new(),
            pair_pow: Vec::new(),
            pair_hit: Vec::new(),
            bound_t: Vec::new(),
            pair_terms: Vec::new(),
            bound_blocks: Vec::new(),
            walked_pairs: 0,
            sparse_output: Vec::new(),
        }
    }

    fn new_compact() -> Self {
        Self {
            local: None,
            profile: PathProfile::new(),
            path_calls: 0,
            skipped_calls: 0,
            pairs_seen: 0,
            raster_samples: 0,
            cand_scratch: Vec::new(),
            crossing_scratch: CrossingScratch::default(),
            arc_scratch: ArcScreeningScratch::new(),
            skyline: ArcSkyline::default(),
            arc_bounds: tile_arc_bounds(),
            seg_scratch: SegSampleScratch::new(),
            pairs_cand: Vec::new(),
            pairs: Vec::new(),
            suffix: Vec::new(),
            pair_pow: Vec::new(),
            pair_hit: Vec::new(),
            bound_t: Vec::new(),
            pair_terms: Vec::new(),
            bound_blocks: Vec::new(),
            walked_pairs: 0,
            sparse_output: Vec::new(),
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
/// The CUDA surface kernel (`engine/noise-gpu/kernels/scatter.cu`) paints the
/// same SPEC §4.7 rule: it compiles this default in
/// (SEG_SAMPLES buckets + the 3° bucket gate, injected by build.rs from
/// `SEG_SAMPLES_DEFAULT` and `seg_sampling::SEG_ARC_MIN_SPAN_RAD`).
/// A CPU-vs-GPU tile comparison therefore runs BOTH lanes at defaults;
/// `noise_gpu::ensure_no_cpu_only_arc_levers` refuses every CPU-only override;
/// accepting a copied numeric default would create a second source of truth.
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
/// Note this feeds BOTH the quadrature's per-bucket query and branch 2's
/// whole-span query when a complete vector store exists but quadrature cannot
/// form a fan. The no-vector-store lane keeps its cp verdict unchanged.
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

/// Everything the generic band loop needs from a (source, receiver) pair that the
/// per-geometry [`PixelGeometry::pixel`] computes. The shared kernel folds these
/// into the budget bound, the path build, and the `max(A_gr, A_bar)` assembly
/// identically for line and point — the divergence law, FLC, exclusion, ground
/// model, and profile sample point are all already baked in here.
#[derive(Clone, Copy)]
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
    /// Preserve the explicit bridge hard-surface rule.  Every other source
    /// geometry (line and point) derives both path G and source-end G from this
    /// pair's sampled IMD profile after the byte-stop has admitted it.
    pub(crate) force_hard_ground: bool,
    /// The LINE geometry's arc-screening query (fix-pack Fix 1); `None` for the
    /// point kernels, which have no angular span to clip.
    pub(crate) arc: Option<ArcSegment>,
}

/// The microsegment an arc-screening query runs over — everything
/// `arc_screening` needs beyond what [`PixelTerms`] already carries.
///
/// Line sources only: a point source IS its characteristic point, so its cp-ray
/// verdict already covers every direction it radiates in.
#[derive(Clone, Copy)]
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
    /// The source-side profile origin, when it is RECEIVER-INDEPENDENT (a
    /// point source IS its own characteristic point). `None` for line sources
    /// (the profile foot moves with the receiver), which disables the
    /// per-(source, block) M3 ground-bound cache and falls back to per-pair
    /// chunk queries.
    fn block_constant_source_latlon(&self) -> Option<(f64, f64)>;
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

    /// Whether this geometry's exact path is independent of pair walk order
    /// when byte-stop is disabled. Point sources have no [`ArcSkyline`] fan,
    /// so their pair result is independent; line sources retain the shipped
    /// loudest-bound walk because skyline growth is order-sensitive.
    fn exact_walk_order_is_stable(&self) -> bool {
        false
    }

    /// Whether the cheap-pass [`PixelTerms`] can be reused by the ordinary
    /// bounded walk. Point geometry is pure and receiver/source-local; line
    /// geometry keeps the historical recomputation unless a future line-
    /// specific proof pays for its larger terms.
    fn cache_pixel_terms(&self) -> bool {
        false
    }
}

/// Telemetry returned by the generic scatter (line and point share the shape).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ScatterStats {
    pub(crate) rows: usize,
    pub(crate) path_calls: u64,
    pub(crate) skipped_calls: u64,
    /// (source, receiver) pairs the cheap pass priced — the DENOMINATOR the skip
    /// fraction actually wants. `path_calls` counts profile builds, and with the
    /// angular quadrature one pair builds `1 + n_seg` of them, so
    /// `skipped/(path+skipped)` understated the pair-level skip several-fold.
    pub(crate) pairs: u64,
    /// (source, receiver) pairs the walk actually computed — the walked
    /// fraction `walked_pairs/pairs` is the M3 payoff number, per tile per arm.
    pub(crate) walked_pairs: u64,
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
    obstacles: &ObstacleSet,
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
    obstacles: &ObstacleSet,
    n_rows: usize,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
) -> ScatterStats {
    scatter_tile_with_cfg_and_options(
        geo, tile, barriers, obstacles, n_rows, accum, cfg, None, None, None,
    )
}

/// [`scatter_tile_with_cfg`] with an optional receiver mask and direct
/// point-source surrogate. This is an experiment hook: `None, None` is the
/// stock exact path, while the W1 industrial candidate passes a mask for
/// exact anchors/blocks or `Some(0.0)` for its direct-local surrogate. The
/// mask is row-major over the complete 512² receiver lattice and is checked
/// before any source/path work for a receiver.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scatter_tile_with_cfg_and_options<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    n_rows: usize,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
    receiver_mask: Option<&[bool]>,
    direct_extra_db: Option<f64>,
    selected_receivers: Option<&[usize]>,
) -> ScatterStats {
    if let Some(mask) = receiver_mask {
        assert_eq!(
            mask.len(),
            TILE_PX * TILE_PX,
            "receiver mask must cover the complete tile"
        );
    }
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
    if let Some(receivers) = selected_receivers {
        assert!(receiver_mask.is_none() && direct_extra_db.is_none());
        return scatter_selected_receivers(
            geo, tile, &prep, barriers, obstacles, n_rows, accum, cfg, receivers,
        );
    }

    let (merged, path_calls, skipped_calls, pairs, walked_pairs, raster_samples) =
        recv_block_regions()
            .into_par_iter()
            .fold(BandScratch::new, |mut s, (py_lo, py_hi, px_lo, px_hi)| {
                // Sparse exact phases often have entire 16x16 receiver blocks
                // outside the selected mask. Do this cheap row-slice check
                // before building the block's source shortlist; otherwise a
                // false-only block still pays pairs_cand clipping and the
                // M3 block-bound setup even though the pixel loop rejects
                // every receiver. The direct phase passes None and keeps
                // the ordinary all-receiver behavior.
                let block_has_receiver = receiver_mask.is_none_or(|mask| {
                    (py_lo..py_hi).any(|py| {
                        mask[py * TILE_PX + px_lo..py * TILE_PX + px_hi]
                            .iter()
                            .any(|&selected| selected)
                    })
                });
                if py_lo < py_hi && px_lo < px_hi && block_has_receiver {
                    scatter_band(
                        geo,
                        tile,
                        &prep,
                        barriers,
                        obstacles,
                        py_lo,
                        py_hi,
                        px_lo,
                        px_hi,
                        cfg,
                        receiver_mask,
                        direct_extra_db,
                        None,
                        &mut s,
                    );
                }
                s
            })
            .map(|s| {
                (
                    s.local.expect("dense scatter scratch"),
                    s.path_calls,
                    s.skipped_calls,
                    s.pairs_seen,
                    s.walked_pairs,
                    s.raster_samples,
                )
            })
            .reduce(
                || (TileAccumulator::new(), 0u64, 0u64, 0u64, 0u64, 0u64),
                |mut a, b| {
                    a.0.merge_from(&b.0);
                    (a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3, a.4 + b.4, a.5 + b.5)
                },
            );
    accum.merge_from(&merged);
    ScatterStats {
        rows: n_rows,
        path_calls,
        skipped_calls,
        pairs,
        walked_pairs,
        raster_samples,
    }
}

#[inline]
fn add_source_power_in_load_order(
    destination: &mut [f32; NUM_PERIODS],
    source_power: &[f32; NUM_PERIODS],
) {
    for (dst, &value) in destination.iter_mut().zip(source_power.iter()) {
        if value > 0.0 {
            *dst += value;
        }
    }
}

#[inline]
fn validate_selected_receiver_indices(receivers: &[usize]) {
    assert!(
        receivers.iter().all(|&index| index < TILE_PX * TILE_PX),
        "selected receiver index is outside the tile"
    );
    assert!(
        receivers.windows(2).all(|pair| pair[0] < pair[1]),
        "selected receiver indices must be strictly sorted and unique"
    );
}

fn merge_compact_output(
    accum: &mut TileAccumulator,
    mut sparse_output: Vec<(usize, [f32; NUM_PERIODS])>,
) {
    sparse_output.sort_unstable_by_key(|(pixel_index, _)| *pixel_index);
    assert!(
        sparse_output.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "selected receiver output contains a duplicate pixel index"
    );
    for (pixel_index, energy) in sparse_output {
        let destination = pixel_index * NUM_PERIODS;
        for (period, &value) in energy.iter().enumerate() {
            if value > 0.0 {
                accum.energy[destination + period] += value;
            }
        }
    }
}

/// Scatter a compact receiver list in parallel spatial blocks. Each block builds
/// its source shortlist and M3 bounds once, then the pixel loop filters to the
/// selected receivers; unselected pixels never enter the pair evaluator.
#[allow(clippy::too_many_arguments)]
fn scatter_selected_receivers<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    prep: &[G::Prep],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    n_rows: usize,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
    receivers: &[usize],
) -> ScatterStats {
    validate_selected_receiver_indices(receivers);
    if receivers.is_empty() {
        return ScatterStats {
            rows: n_rows,
            ..Default::default()
        };
    }

    // The selector returns row-major indices, so each group is already sorted
    // for the binary-search membership check in scatter_band. Grouping by the
    // same spatial blocks as the ordinary path is the key optimization: one
    // shortlist and one M3 setup serve all selected pixels in that block.
    let regions = recv_block_regions();
    let block_size = recv_block_px();
    let blocks_per_side = TILE_PX.div_ceil(block_size);
    let mut selected_by_block: Vec<Vec<usize>> = (0..regions.len()).map(|_| Vec::new()).collect();
    for &index in receivers {
        let py = index / TILE_PX;
        let px = index % TILE_PX;
        let block_index = (py / block_size) * blocks_per_side + px / block_size;
        selected_by_block[block_index].push(index);
    }

    let (sparse_output, path_calls, skipped_calls, pairs, walked_pairs, raster_samples) =
        selected_by_block
            .into_par_iter()
            .enumerate()
            .filter(|(_, selected)| !selected.is_empty())
            .fold(
                BandScratch::new_compact,
                |mut s, (block_index, selected)| {
                    let (py_lo, py_hi, px_lo, px_hi) = regions[block_index];
                    scatter_band(
                        geo,
                        tile,
                        prep,
                        barriers,
                        obstacles,
                        py_lo,
                        py_hi,
                        px_lo,
                        px_hi,
                        cfg,
                        None,
                        None,
                        Some(selected.as_slice()),
                        &mut s,
                    );
                    s
                },
            )
            .map(|s| {
                (
                    s.sparse_output,
                    s.path_calls,
                    s.skipped_calls,
                    s.pairs_seen,
                    s.walked_pairs,
                    s.raster_samples,
                )
            })
            .reduce(
                || (Vec::new(), 0u64, 0u64, 0u64, 0u64, 0u64),
                |mut a, b| {
                    if a.0.len() < b.0.len() {
                        let mut larger = b.0;
                        larger.extend(a.0);
                        a.0 = larger;
                    } else {
                        a.0.extend(b.0);
                    }
                    (a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3, a.4 + b.4, a.5 + b.5)
                },
            );
    merge_compact_output(accum, sparse_output);
    ScatterStats {
        rows: n_rows,
        path_calls,
        skipped_calls,
        pairs,
        walked_pairs,
        raster_samples,
    }
}

struct ExactPairEvaluation {
    power: [f32; NUM_PERIODS],
    lden_energy: f64,
    used_quadrature: bool,
    used_escalation: bool,
}

/// Evaluate one admitted pair with the complete shared surface physics.
///
/// Both the ordinary bounded walk and the exact point source-major bypass call
/// this function. Keeping the ground/terrain/obstacle/arc/vegetation assembly in
/// one body prevents the two traversal topologies from becoming two physical
/// models; callers retain ownership of pair bounds, walk order, and f32
/// accumulation order.
#[allow(clippy::too_many_arguments)]
fn evaluate_exact_pair(
    tile: &FusedTileZ13,
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    cfg: Option<CoarseMid>,
    t: &PixelTerms,
    rx_lat: f64,
    rx_lon: f64,
    rx_alt: f64,
    emission_lin: &[[f32; NUM_BANDS]; NUM_PERIODS],
    n_seg: usize,
    s: &mut BandScratch,
) -> ExactPairEvaluation {
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
    let ground_path = path_effects::cnossos_ground_path_from_profile(
        &mut s.profile,
        t.src_alt,
        rx_alt,
        t.force_hard_ground,
    );
    let ground_g = ground_path.ground_path_g;
    // Screening is evaluated per angular bucket; the ground term remains tied
    // to the line segment's characteristic point.
    let ground_bands = ground_atten_bands(ground_path);
    // Heatmap discards the popup obstacle traces, so call the metadata-free
    // band-only variants: terrain skips the per-pixel EdgePoint Vec, screening
    // skips the ObstacleEdge materialisation.
    let (terrain_bands, terrain_delta_m) =
        path_effects::terrain_attenuation(&mut s.profile, t.src_alt, rx_alt);
    // ── the ground/barrier term, ISO 9613-2 §7.3.1 ──────────────────────────
    // `max(A_ground, A_terrain + A_screen)`: a barrier REPLACES ground, never
    // adds. Uniform angular quadrature is tried before the characteristic-point
    // ray (and its optional arc-screening fallback) because each bucket already
    // evaluates its own ray; computing the cp screening first would be dead work
    // and turn an N-ray pair into N+1 rays. The ordering and energy average are
    // owned by `seg_sampling::sampled_gob_bands_with_ground`. The arc rule
    // clips against the receiver's obstacle skyline, so it needs a skyline to
    // clip: with no indexed cells the cp-ray verdict stands, exactly as it did
    // when an absent store was representable. (An EMPTY set reaching the arc
    // quadrature is not free — it is a different approximation of the segment,
    // and it broke the kernel-vs-popup parity test by 43 %.)
    let sampled = match &t.arc {
        Some(arc) if n_seg > 1 && !obstacles.indexes.is_empty() => {
            let query = arc_query(
                arc,
                t,
                rx_lat,
                rx_lon,
                rx_alt,
                &ZERO_BANDS,
                &terrain_bands,
                ground_g,
                barriers,
                obstacles,
                s.arc_bounds,
            );
            sampled_gob_bands_with_ground(
                &query,
                &SurfaceCadenceRasters { tile, cfg },
                n_seg,
                &ground_bands,
                &mut s.skyline,
                &mut s.seg_scratch,
            )
        }
        _ => None,
    };
    let (gob_bands, used_quadrature, used_escalation) = match sampled {
        Some((gob, cost)) => {
            s.path_calls += cost.rays;
            s.raster_samples += cost.raster_samples;
            (gob, true, cost.escalated > 0)
        }
        None => {
            obstacles.crossings_pruned_with_scratch(
                t.cp_lat,
                t.cp_lon,
                rx_lat,
                rx_lon,
                &CellPrune::for_profile(&s.profile, t.src_alt, rx_alt),
                &mut s.crossing_scratch,
                &mut s.cand_scratch,
            );
            if !s.cand_scratch.is_empty() {
                note_obstacle_crossing();
            }
            let obstacle_input = path_effects::ObstacleInput {
                candidates: &s.cand_scratch,
            };
            let cp_screening = path_effects::screening_attenuation(
                &mut s.profile,
                barriers,
                obstacle_input,
                t.src_alt,
                rx_alt,
                t.excl_m,
                &terrain_bands,
                terrain_delta_m,
            );
            let screening = match &t.arc {
                Some(arc)
                    if !obstacles.indexes.is_empty()
                        && segment_can_span(arc.length_m, arc.dist_m, s.arc_bounds) =>
                {
                    let query = arc_query(
                        arc,
                        t,
                        rx_lat,
                        rx_lon,
                        rx_alt,
                        &cp_screening,
                        &terrain_bands,
                        ground_g,
                        barriers,
                        obstacles,
                        s.arc_bounds,
                    );
                    arc_screened_attenuation_with_ground(
                        &query,
                        &SurfaceCadenceRasters { tile, cfg },
                        &mut s.skyline,
                        &ground_bands,
                        &mut s.arc_scratch,
                    )
                }
                _ => cp_screening,
            };
            if !barriers.is_empty() {
                note_barrier_path();
            }
            let mut gob = [0.0f64; NUM_BANDS];
            for (i, g) in gob.iter_mut().enumerate() {
                *g = ground_or_barrier_db(ground_bands[i], terrain_bands[i], screening[i]);
            }
            (gob, false, false)
        }
    };
    let veg = path_effects::vegetation_attenuation_path(&s.profile);

    // Period-independent per-band path factor (A-weighted linear).
    let mut pf = [0.0f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        let path_db = t.base_db - ALPHA_ATM[i] * t.atm_d_km - gob_bands[i] - veg[i];
        pf[i] = db_to_lin_a(path_db, i);
    }

    let mut lden_energy = 0.0;
    let mut power = [0.0f32; NUM_PERIODS];
    for p in 0..NUM_PERIODS {
        let mut period_power = 0.0f64;
        for i in 0..NUM_BANDS {
            period_power += emission_lin[p][i] as f64 * pf[i];
        }
        if period_power.is_finite() && period_power > 0.0 {
            power[p] = period_power as f32;
            lden_energy += period_power * LDEN_WEIGHTS[p];
        }
    }
    ExactPairEvaluation {
        power,
        lden_energy,
        used_quadrature,
        used_escalation,
    }
}

/// Exact point-source walk in source-major order.
///
/// With byte stopping disabled, point paths have no receiver skyline and their
/// result is independent of source order. Walking one prepared point across the
/// block's receivers removes the ordinary per-receiver sort and second geometry
/// evaluation while keeping obstacle/raster state local to the source. Each
/// receiver still accumulates in prepared-source order, and every admitted pair
/// follows the same exact propagation body before its power is committed.
#[allow(clippy::too_many_arguments)]
fn scatter_exact_point_source_major<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    prep: &[G::Prep],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    py_lo: usize,
    py_hi: usize,
    px_lo: usize,
    px_hi: usize,
    cfg: Option<CoarseMid>,
    n_seg: usize,
    s: &mut BandScratch,
) {
    for candidate_index in 0..s.pairs_cand.len() {
        let ci = s.pairs_cand[candidate_index];
        let pr = &prep[ci as usize];
        let (rpy0, rpy1, rpx0, rpx1) = pr.reach_box();
        let py_start = py_lo.max(rpy0);
        let py_end = py_hi.min(rpy1.saturating_add(1));
        let px_start = px_lo.max(rpx0);
        let px_end = px_hi.min(rpx1.saturating_add(1));
        if py_start >= py_end || px_start >= px_end {
            continue;
        }
        for py in py_start..py_end {
            let rx_lat = tile.rx_lat[py];
            let row_base = py * TILE_PX;
            for px in px_start..px_end {
                let idx = row_base + px;
                let rx_lon = tile.rx_lon[px];
                let rx_alt = tile.rx_alt_m[idx] as f64;
                let refl = tile.rx_refl_db[idx] as f64;
                let Some(t) = geo.pixel(pr, tile, rx_lat, rx_lon, rx_alt, refl) else {
                    continue;
                };
                debug_assert!(
                    t.arc.is_none(),
                    "source-major exact walk is only valid for point geometry"
                );
                s.pairs_seen += 1;
                s.walked_pairs += 1;
                note_exact_point_bypass_pair();
                census::pair_walked();

                let evaluated = evaluate_exact_pair(
                    tile,
                    barriers,
                    obstacles,
                    cfg,
                    &t,
                    rx_lat,
                    rx_lon,
                    rx_alt,
                    pr.emission_lin(),
                    n_seg,
                    s,
                );
                let local = s.local.as_mut().expect("dense scatter scratch");
                for (period, &power) in evaluated.power.iter().enumerate() {
                    if power > 0.0 {
                        local.add_energy_at(py as u32, px as u32, period as u8, power);
                    }
                }
            }
        }
    }
}

/// Scatter every source that reaches the block `[py_lo, py_hi) × [px_lo, px_hi)`
/// into its pixels, stopping each pixel as soon as its output BYTE is decided
/// ([`crate::byte_stop`], and the module docs for why that is exact). The single
/// hot loop both line and point share; the per-pixel geometry is `geo.pixel`.
#[allow(clippy::too_many_arguments)]
fn scatter_band<G: PixelGeometry>(
    geo: &G,
    tile: &FusedTileZ13,
    prep: &[G::Prep],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    py_lo: usize,
    py_hi: usize,
    px_lo: usize,
    px_hi: usize,
    cfg: Option<CoarseMid>,
    receiver_mask: Option<&[bool]>,
    direct_extra_db: Option<f64>,
    selected_pixels: Option<&[usize]>,
    s: &mut BandScratch,
) {
    // Sources reaching this block AT ALL, resolved once. The ordinary walk
    // below is pixel-major, so without the shortlist every one of the block's
    // 256 receivers would re-clip the whole `prep` slice. The exact point
    // source-major arm uses the same shortlist to clip each source once.
    s.pairs_cand.clear();
    for (i, pr) in prep.iter().enumerate() {
        let (rpy0, rpy1, rpx0, rpx1) = pr.reach_box();
        if rpy0 < py_hi && rpy1 >= py_lo && rpx0 < px_hi && rpx1 >= px_lo {
            s.pairs_cand.push(i as u32);
        }
    }
    if s.pairs_cand.is_empty() {
        return;
    }
    let n_seg = seg_samples();
    let stop_on = byte_stop_enabled();
    // The exact/reference arm (`SURFACE_BUDGET_ETA=0`) still used to price,
    // sort, and re-walk every point pair. For point sources there is no arc
    // skyline whose construction depends on walk order, so preserve source
    // load order directly and skip the cheap-pass/sort machinery. The bypass
    // has no byte-stop tail and therefore needs no pair bound. Selected sparse
    // receivers keep the old path because they need compact-output handling.
    let load_order_exact = !stop_on
        && receiver_mask.is_none()
        && direct_extra_db.is_none()
        && selected_pixels.is_none()
        && geo.exact_walk_order_is_stable();
    let cache_terms = geo.cache_pixel_terms();
    if load_order_exact {
        scatter_exact_point_source_major(
            geo, tile, prep, barriers, obstacles, py_lo, py_hi, px_lo, px_hi, cfg, n_seg, s,
        );
        return;
    }
    // M3a per-(source, block) pooled chunk maxima, resolved ONCE per block
    // (8 pyramid boxes per source instead of per pair) for the sources whose
    // profile origin is receiver-independent.
    if direct_extra_db.is_none() && stop_on && crate::bound_m3::surface_bound_m3_enabled() {
        let (lat_lo, lat_hi) = if tile.rx_lat[py_lo] <= tile.rx_lat[py_hi - 1] {
            (tile.rx_lat[py_lo], tile.rx_lat[py_hi - 1])
        } else {
            (tile.rx_lat[py_hi - 1], tile.rx_lat[py_lo])
        };
        let (lon_lo, lon_hi) = if tile.rx_lon[px_lo] <= tile.rx_lon[px_hi - 1] {
            (tile.rx_lon[px_lo], tile.rx_lon[px_hi - 1])
        } else {
            (tile.rx_lon[px_hi - 1], tile.rx_lon[px_lo])
        };
        s.bound_blocks.clear();
        for &ci in &s.pairs_cand {
            let m = prep[ci as usize]
                .block_constant_source_latlon()
                .map(|(la, lo)| {
                    crate::bound_m3::block_ground_maxima(
                        tile, la, lo, lat_lo, lat_hi, lon_lo, lon_hi,
                    )
                });
            s.bound_blocks.push(m);
        }
    }
    for py in py_lo..py_hi {
        let rx_lat = tile.rx_lat[py];
        let row_base = py * TILE_PX;
        for px in px_lo..px_hi {
            let idx = row_base + px;
            if receiver_mask.is_some_and(|mask| !mask[idx])
                || selected_pixels.is_some_and(|pixels| pixels.binary_search(&idx).is_err())
            {
                continue;
            }
            let rx_lon = tile.rx_lon[px];
            let rx_alt = tile.rx_alt_m[idx] as f64;
            let refl = tile.rx_refl_db[idx] as f64;

            // Direct-local W1 surrogate: keep the point geometry (reach,
            // free-field audibility, area-source distance, spherical spread,
            // receiver altitude and atmospheric attenuation), but do not
            // build a terrain profile or query obstacles/barriers. The fixed
            // loose M3 floor is deliberately conservative and is only
            // reachable through the explicit industrial candidate API.
            if let Some(extra_db) = direct_extra_db {
                let loose = crate::bound_m3::M3PairBound::loose();
                for &ci in &s.pairs_cand {
                    let pr = &prep[ci as usize];
                    let (rpy0, rpy1, rpx0, rpx1) = pr.reach_box();
                    if py < rpy0 || py > rpy1 || px < rpx0 || px > rpx1 {
                        continue;
                    }
                    let Some(t) = geo.pixel(pr, tile, rx_lat, rx_lon, rx_alt, refl) else {
                        continue;
                    };
                    debug_assert!(t.arc.is_none(), "direct W1 point arm reached a line source");
                    s.pairs_seen += 1;
                    s.walked_pairs += 1;
                    let emission_lin = pr.emission_lin();
                    let mut path_factor = [0.0f64; NUM_BANDS];
                    for band in 0..NUM_BANDS {
                        let path_db = t.base_db
                            - ALPHA_ATM[band] * t.atm_d_km
                            - (loose.gob_lb_db(band) + extra_db);
                        path_factor[band] = db_to_lin_a(path_db, band);
                    }
                    for (period, period_emission) in emission_lin.iter().enumerate() {
                        let mut power = 0.0f64;
                        for band in 0..NUM_BANDS {
                            power += period_emission[band] as f64 * path_factor[band];
                        }
                        if power.is_finite() && power > 0.0 {
                            s.local
                                .as_mut()
                                .expect("dense scatter scratch")
                                .add_energy_at(py as u32, px as u32, period as u8, power as f32);
                        }
                    }
                }
                continue;
            }

            // ── cheap pass: this receiver's pairs and their bounds ──────────
            // `P⁺` needs the bound over the WHOLE tail, so every pair is priced
            // before any is computed. This is the same per-pair bound the
            // superseded budget skip already paid for on every pair; what it
            // buys now is a certain upper bound instead of a running comparison.
            s.pairs.clear();
            s.pair_terms.clear();
            for k in 0..s.pairs_cand.len() {
                let ci = s.pairs_cand[k];
                let pr = &prep[ci as usize];
                let (rpy0, rpy1, rpx0, rpx1) = pr.reach_box();
                if py < rpy0 || py > rpy1 || px < rpx0 || px > rpx1 {
                    continue;
                }
                let Some(t) = geo.pixel(pr, tile, rx_lat, rx_lon, rx_alt, refl) else {
                    continue;
                };
                // M3: tighten this pair's bound (ground pyramid + cp-ray
                // terrain march) BEFORE pricing it into P⁺ — the cheap pass is
                // where every pair is priced, so the tightening must be at
                // least an order under the exact path it saves.
                let bound = crate::bound_m3::pair_bound(
                    tile,
                    cfg,
                    &t,
                    rx_lat,
                    rx_lon,
                    rx_alt,
                    obstacles,
                    n_seg,
                    s.bound_blocks.get(k).and_then(|m| m.as_ref()),
                    &mut s.bound_t,
                );
                let ub = budget_ub_lden(t.base_db, t.atm_d_km, pr.emission_lden(), &bound);
                let ord = s.pairs.len() as u32;
                if cache_terms {
                    s.pair_terms.push(t);
                }
                s.pairs.push(PairBound { src: ci, ord, ub });
            }
            let n_pairs = s.pairs.len();
            if n_pairs == 0 {
                continue;
            }
            s.pairs_seen += n_pairs as u64;
            // Loudest bound first — cost only, never the answer (see
            // [`PairBound`]).
            s.pairs.sort_unstable_by(|a, b| b.ub.total_cmp(&a.ub));
            s.suffix.clear();
            s.suffix.resize(n_pairs + 1, 0.0);
            for k in (0..n_pairs).rev() {
                s.suffix[k] = s.suffix[k + 1] + s.pairs[k].ub;
            }
            s.pair_hit.clear();
            s.pair_hit.resize(n_pairs, false);
            if s.pair_pow.len() < n_pairs {
                s.pair_pow.resize(n_pairs, [0.0; NUM_PERIODS]);
            }
            let margin = byte_stop::accum_margin(n_pairs);
            s.skyline.reset();
            let mut p_lo = 0.0f64;
            let mut walked = n_pairs;
            // Gather-redesign census (QM_TILE_CENSUS=1, else dead bools).
            let mut census_any_quad = false;
            let mut census_any_esc = false;

            for k in 0..n_pairs {
                // ── THE STOP ────────────────────────────────────────────────
                // `[P⁻, P⁺]` brackets every completion of the tail, so once both
                // ends quantise to one byte the tail cannot be seen and the
                // pixel is finished. Tested BEFORE the pair is priced in, so the
                // pair that would have closed it is never computed either.
                if stop_on
                    && byte_stop::decided(
                        p_lo,
                        p_lo + s.suffix[k],
                        byte_stop::SURFACE_LDEN_SCALE,
                        margin,
                    )
                {
                    note_early_stop();
                    walked = k;
                    break;
                }
                let PairBound { src, ord, ub } = s.pairs[k];
                let pr = &prep[src as usize];
                let t = if cache_terms {
                    note_cached_pixel_term_read();
                    s.pair_terms[ord as usize]
                } else {
                    let Some(t) = geo.pixel(pr, tile, rx_lat, rx_lon, rx_alt, refl) else {
                        continue;
                    };
                    t
                };
                census::pair_walked();

                let evaluated = evaluate_exact_pair(
                    tile,
                    barriers,
                    obstacles,
                    cfg,
                    &t,
                    rx_lat,
                    rx_lon,
                    rx_alt,
                    pr.emission_lin(),
                    n_seg,
                    s,
                );
                census_any_quad |= evaluated.used_quadrature;
                census_any_esc |= evaluated.used_escalation;
                // ── THE SOUNDNESS INVARIANT, IN THE KERNEL ──────────────────
                // The whole method rests on `ub ≥ exact` for every single pair:
                // if one bound ever under-reads its pair, `P⁺` stops being an
                // upper bound and a pixel can commit the wrong byte. It is one
                // f64 compare per exactly-computed pair — cheap enough to assert
                // in RELEASE, which is the only build that paints the world.
                // (`UB_SAFETY`'s 1e-4 head-room absorbs `fast_exp_f64`'s ~1.4e-6
                // non-monotonicity and the band/period summation-order split
                // between `emission_lden` and this loop; a failure here is a real
                // bound bug, not float noise.)
                assert!(
                    evaluated.lden_energy <= ub,
                    "byte-stop bound violated: exact {:e} > ub {ub:e} \
                     (py={py} px={px} src={src})",
                    evaluated.lden_energy,
                );
                p_lo += evaluated.lden_energy;
                s.pair_hit[ord as usize] = true;
                s.pair_pow[ord as usize] = evaluated.power;
            }
            s.skipped_calls += (n_pairs - walked) as u64;
            s.walked_pairs += walked as u64;

            // ── accumulate in SOURCE-LOAD order ─────────────────────────────
            // Not the walk's order: f32 addition does not commute, so a pixel
            // that never closed must land where a kernel with no stopping at all
            // lands it, to the bit. See `BandScratch::pair_pow`.
            if selected_pixels.is_some() {
                let mut compact_energy = [0.0f32; NUM_PERIODS];
                for o in 0..n_pairs {
                    if s.pair_hit[o] {
                        add_source_power_in_load_order(&mut compact_energy, &s.pair_pow[o]);
                    }
                }
                s.sparse_output.push((idx, compact_energy));
            } else {
                let local = s.local.as_mut().expect("dense scatter scratch");
                for o in 0..n_pairs {
                    if !s.pair_hit[o] {
                        continue;
                    }
                    let pow = s.pair_pow[o];
                    for (p, &e) in pow.iter().enumerate() {
                        if e > 0.0 {
                            local.add_energy_at(py as u32, px as u32, p as u8, e);
                        }
                    }
                }
            }
            census::receiver_done(census_any_quad, census_any_esc);
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
    use crate::scatter_point::{PointGeometry, PreparedPoint};
    use crate::source_line::LineRow;
    use crate::source_point::PointRow;
    use crate::wire_hm3::collapse_lden_surface_u8;
    use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
    use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
    use noise_compute::propagation::path_profile::{fill_t_values, fill_t_values_coarse_mid};
    use raster_reader::RealRasters;
    use std::{fs, path::Path, process::Command, sync::Arc};
    use tempfile::TempDir;

    /// A scene of many line sources of wildly different loudness spread over the
    /// tile, so a receiver's pair list spans tens of dB and the interval has
    /// something to close on. `emission_lin` steps by decades and the segments
    /// step across the tile, which puts each pixel's dominant source at a
    /// different position in the load order — the property the old η rule was
    /// sensitive to.
    fn many_sources(tile: &FusedTileZ13) -> Vec<LineRow> {
        let c_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let c_lon = (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5;
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(c_lat.to_radians());
        (0..24)
            .map(|k| {
                let off = (k as f64 - 11.5) * 130.0;
                let e = 1.0e5 * 10f32.powi(k % 5);
                LineRow {
                    start_lat: c_lat + d_lat(off),
                    start_lon: c_lon + d_lon(-300.0 + 40.0 * (k % 7) as f64),
                    end_lat: c_lat + d_lat(off + 40.0),
                    end_lon: c_lon + d_lon(300.0),
                    length_m: 620.0,
                    max_distance_m: 2_500.0,
                    source_height_m: 0.05,
                    bridge: k % 11 == 0,
                    emission_lin: [[e; NUM_BANDS]; NUM_PERIODS],
                }
            })
            .collect()
    }

    /// THE regression the byte-space stop exists to close.
    ///
    /// The superseded energy-budget skip compared each pair against a `kept`
    /// that starts at zero and only grows, so whether a source was dropped
    /// depended on how many louder ones had already been folded — i.e. on the
    /// order rows came off disk. Two orders kept different source sets and
    /// painted different tiles (measured on dense Praha: ~75 k pairs apart).
    ///
    /// The interval rule cannot do that: it only ever drops a tail it has
    /// PROVEN cannot move the byte, and that proof does not reference the order.
    /// So the painted bytes must be identical for any permutation of the load
    /// order — asserted here on the whole 512² grid, not a sample. A single
    /// differing cell means the tail was not really immaterial.
    #[test]
    fn source_order_never_changes_the_answer() {
        let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-bytestop-fixture"));
        let tile = FusedTileZ13::build(12, 2211, 1386, 2_500.0, &rasters);
        let lines = many_sources(&tile);

        let paint = |rows: &[LineRow]| {
            let mut accum = TileAccumulator::new();
            let stats = crate::scatter_line::scatter_tile(
                &tile,
                rows,
                &[],
                &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
                &mut accum,
            );
            (collapse_lden_surface_u8(&accum), stats)
        };
        let (bytes_a, stats_a) = paint(&lines);

        // Not a vacuous test: the stop must actually be firing on this scene,
        // and the scene must actually be painting something.
        assert!(
            stats_a.skipped_calls > 0,
            "fixture never triggers the stop — the assertion below proves nothing"
        );
        assert!(
            bytes_a.iter().any(|&b| b != crate::wire_hm3::NO_DATA),
            "fixture painted an empty tile"
        );

        // Reversed, and a stride shuffle: two permutations that move every
        // pixel's dominant source to a different place in the walk.
        let mut rev = many_sources(&tile);
        rev.reverse();
        let mut shuffled = many_sources(&tile);
        for i in 0..shuffled.len() {
            let j = (i * 7 + 3) % shuffled.len();
            shuffled.swap(i, j);
        }
        for (name, rows) in [("reversed", &rev), ("shuffled", &shuffled)] {
            let (bytes_b, _) = paint(rows);
            let diff = bytes_a
                .iter()
                .zip(bytes_b.iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(diff, 0, "{name} load order moved {diff} cells");
        }
    }

    fn point_optimization_raster_fixture() -> TempDir {
        let root = tempfile::tempdir().expect("create point optimization raster fixture");
        for subdir in ["dem/copernicus", "rasters/forest", "rasters/imd"] {
            fs::create_dir_all(root.path().join(subdir)).expect("create raster fixture directory");
        }

        // RealRasters accepts the standard 1201² SRTM alternate for DEM and
        // requires 3601² for the three u8 products. The tested z12 tile and its
        // 700 m halo stay wholly inside N50E014.
        let dem_value = 135_i16.to_be_bytes();
        let mut dem = Vec::with_capacity(1201 * 1201 * 2);
        for _ in 0..1201 * 1201 {
            dem.extend_from_slice(&dem_value);
        }
        fs::write(root.path().join("dem/copernicus/N50E014.hgt"), dem).expect("write DEM fixture");

        let cells = 3601 * 3601;
        let mut building = vec![0_u8; cells];
        // A real non-zero building patch around this tile's centre, rather than
        // a merely present all-zero file.
        for row in 3000..3135 {
            building[row * 3601 + 1260..row * 3601 + 1405].fill(7);
        }
        fs::write(
            root.path().join("rasters/forest/N50E014.raw"),
            vec![37_u8; cells],
        )
        .expect("write forest fixture");
        fs::write(
            root.path().join("rasters/imd/N50E014.raw"),
            vec![58_u8; cells],
        )
        .expect("write IMD fixture");
        root
    }

    /// The exact industrial arm may bypass the receiver-major sort, while the
    /// stopped arm may reuse cheap-pass point terms. Compare both candidates
    /// against the historical generic evaluator on raw f32 energy, with real
    /// raster, barrier, and vector-obstacle work in the scene. Child processes
    /// isolate the process-wide `SURFACE_BUDGET_ETA` OnceLock.
    #[test]
    fn point_bypass_and_term_cache_preserve_f32_energy() {
        let Some(mode) = std::env::var_os("QM_POINT_OPTIMIZATION_CHILD") else {
            for (mode, eta) in [("bypass", "0"), ("cache", "0.4")] {
                let status =
                    Command::new(std::env::current_exe().expect("current test executable"))
                        .arg("--exact")
                        .arg("scatter_band::tests::point_bypass_and_term_cache_preserve_f32_energy")
                        .arg("--nocapture")
                        .env("QM_POINT_OPTIMIZATION_CHILD", mode)
                        .env("SURFACE_BUDGET_ETA", eta)
                        .status()
                        .expect("spawn point optimization child test");
                assert!(status.success(), "point {mode} child test failed");
            }
            return;
        };
        let mode = mode.to_str().expect("UTF-8 child mode");
        assert_eq!(byte_stop_enabled(), mode == "cache");

        let raster_fixture = point_optimization_raster_fixture();
        let rasters = RealRasters::new(raster_fixture.path());
        let tile = FusedTileZ13::build(12, 2211, 1386, 700.0, &rasters);
        assert!(tile.inner_elev_m.iter().any(|&v| v > 0.0));
        assert!(tile.inner_forest.iter().any(|&v| v > 0));
        assert!(tile.inner_imd.iter().any(|&v| v > 0 && v < 100));

        let c_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let c_lon = (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5;
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(c_lat.to_radians());

        let point_offsets = [
            (-210.0, -170.0),
            (-190.0, 190.0),
            (0.0, -260.0),
            (0.0, 260.0),
            (210.0, -150.0),
            (190.0, 210.0),
        ];
        let points: Vec<PointRow> = point_offsets
            .into_iter()
            .enumerate()
            .map(|(k, (north_m, east_m))| {
                let base = 2.0e7_f32 * (k + 1) as f32;
                PointRow {
                    lat: c_lat + d_lat(north_m),
                    lon: c_lon + d_lon(east_m),
                    source_height_m: 6.0,
                    max_distance_m: 550.0,
                    exclusion_radius_m: 8.0,
                    max_day_emission_db: 120.0,
                    emission_lin: std::array::from_fn(|period| {
                        std::array::from_fn(|band| {
                            base * (1.0 + period as f32 * 0.17 + band as f32 * 0.03)
                        })
                    }),
                }
            })
            .collect();

        let barriers = [Barrier {
            osm_id: 71,
            segment_idx: 0,
            height_m: 4.5,
            start_lat: c_lat + d_lat(-160.0),
            start_lon: c_lon,
            end_lat: c_lat + d_lat(160.0),
            end_lon: c_lon,
            dist_m: 0.0,
        }];
        let mut obstacle_builder = ObstacleIndex::builder(c_lat, c_lon);
        obstacle_builder.add_ring(
            &[
                (c_lat + d_lat(-35.0), c_lon + d_lon(-45.0)),
                (c_lat + d_lat(-35.0), c_lon + d_lon(45.0)),
                (c_lat + d_lat(35.0), c_lon + d_lon(45.0)),
                (c_lat + d_lat(35.0), c_lon + d_lon(-45.0)),
            ],
            11.0,
            ObstacleKind::Building,
            19,
        );
        let obstacles = ObstacleSet {
            indexes: vec![Arc::new(obstacle_builder.build())],
        };
        assert!(!barriers.is_empty());
        assert!(obstacles.edge_count() > 0);

        struct HistoricalPointGeometry<'a> {
            points: &'a [PointRow],
        }
        impl<'a> PixelGeometry for HistoricalPointGeometry<'a> {
            type Prep = PreparedPoint<'a>;

            fn prepare(&self, tile: &FusedTileZ13, prep: &mut Vec<Self::Prep>) {
                let geometry = PointGeometry {
                    points: self.points,
                };
                geometry.prepare(tile, prep);
            }

            fn pixel(
                &self,
                prep: &Self::Prep,
                tile: &FusedTileZ13,
                rx_lat: f64,
                rx_lon: f64,
                rx_alt: f64,
                refl: f64,
            ) -> Option<PixelTerms> {
                PointGeometry {
                    points: self.points,
                }
                .pixel(prep, tile, rx_lat, rx_lon, rx_alt, refl)
            }
        }

        let candidate = PointGeometry { points: &points };
        let historical = HistoricalPointGeometry { points: &points };
        reset_point_optimization_counts();
        let mut candidate_accum = TileAccumulator::new();
        let candidate_stats = scatter_tile_with_cfg(
            &candidate,
            &tile,
            &barriers,
            &obstacles,
            TILE_PX,
            &mut candidate_accum,
            None,
        );
        let candidate_counts = point_optimization_counts();
        let candidate_execution = point_execution_counts();

        reset_point_optimization_counts();
        let mut historical_accum = TileAccumulator::new();
        let historical_stats = scatter_tile_with_cfg(
            &historical,
            &tile,
            &barriers,
            &obstacles,
            TILE_PX,
            &mut historical_accum,
            None,
        );
        let historical_counts = point_optimization_counts();
        let historical_execution = point_execution_counts();

        eprintln!(
            "point-{mode}: candidate pairs={} paths={} bypass={} cache_reads={} \
             early_stops={} barrier_paths={} obstacle_crossings={}; historical \
             pairs={} paths={} bypass={} cache_reads={} early_stops={} \
             barrier_paths={} obstacle_crossings={}",
            candidate_stats.pairs,
            candidate_stats.path_calls,
            candidate_counts.0,
            candidate_counts.1,
            candidate_execution.0,
            candidate_execution.1,
            candidate_execution.2,
            historical_stats.pairs,
            historical_stats.path_calls,
            historical_counts.0,
            historical_counts.1,
            historical_execution.0,
            historical_execution.1,
            historical_execution.2,
        );

        assert!(candidate_stats.path_calls > 0 && historical_stats.path_calls > 0);
        assert!(candidate_accum.energy.iter().any(|&energy| energy > 0.0));
        for (name, execution) in [
            ("candidate", candidate_execution),
            ("historical", historical_execution),
        ] {
            assert!(
                execution.1 > 0,
                "{name} never executed the explicit barrier screening path"
            );
            assert!(
                execution.2 > 0,
                "{name} never executed a vector-obstacle crossing path"
            );
        }
        match mode {
            "bypass" => {
                assert!(
                    candidate_counts.0 > 0,
                    "candidate never entered exact bypass"
                );
                assert_eq!(candidate_counts.1, 0, "exact bypass read term cache");
                assert_eq!(historical_counts, (0, 0));
                assert_eq!(
                    candidate_execution.0, 0,
                    "exact bypass unexpectedly stopped"
                );
                assert_eq!(
                    historical_execution.0, 0,
                    "unstopped historical arm stopped"
                );
            }
            "cache" => {
                assert_eq!(candidate_counts.0, 0, "stopped candidate entered bypass");
                assert!(candidate_counts.1 > 0, "candidate never read cached terms");
                assert_eq!(historical_counts, (0, 0));
                assert!(
                    candidate_execution.0 > 0 && historical_execution.0 > 0,
                    "cache arm did not execute a real early stop: candidate={} historical={}",
                    candidate_execution.0,
                    historical_execution.0,
                );
                assert!(
                    candidate_stats.skipped_calls > 0 && historical_stats.skipped_calls > 0,
                    "cache arm reported no skipped pairs: candidate={} historical={}",
                    candidate_stats.skipped_calls,
                    historical_stats.skipped_calls,
                );
            }
            other => panic!("unknown point optimization child mode {other}"),
        }

        let first_energy_difference = candidate_accum
            .energy
            .iter()
            .zip(&historical_accum.energy)
            .enumerate()
            .find(|(_, (candidate, historical))| candidate.to_bits() != historical.to_bits());
        assert!(
            first_energy_difference.is_none(),
            "point {mode} changed raw f32 energy at {first_energy_difference:?}"
        );
        assert_eq!(
            collapse_lden_surface_u8(&candidate_accum),
            collapse_lden_surface_u8(&historical_accum),
        );
    }

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
    #[test]
    fn compact_accumulation_matches_dense_in_source_load_order() {
        let mut dense = TileAccumulator::new();
        let mut compact = TileAccumulator::new();
        for (pixel_index, sources) in [
            (
                7usize,
                [
                    [1.0, 0.0, 3.5],
                    [0.0001, 2.0, 0.0],
                    [16_777_216.0, 0.25, 4.0],
                    [0.0, 0.00000005, 1.0],
                ],
            ),
            (
                TILE_PX + 11,
                [
                    [0.5, 7.0, 0.0],
                    [0.5, 0.0, 9.0],
                    [0.0000001, 2.0, 3.0],
                    [8.0, 0.0, 0.0000002],
                ],
            ),
        ] {
            let mut compact_energy = [0.0f32; NUM_PERIODS];
            let py = pixel_index / TILE_PX;
            let px = pixel_index % TILE_PX;
            for source_power in sources {
                add_source_power_in_load_order(&mut compact_energy, &source_power);
                for (period, &value) in source_power.iter().enumerate() {
                    if value > 0.0 {
                        dense.add_energy_at(py as u32, px as u32, period as u8, value);
                    }
                }
            }
            merge_compact_output(&mut compact, vec![(pixel_index, compact_energy)]);
        }
        assert_eq!(dense.energy, compact.energy);
    }

    #[test]
    fn compact_scratch_omits_dense_buffers() {
        let scratch = BandScratch::new_compact();
        assert!(scratch.local.is_none());
    }

    #[test]
    #[should_panic(expected = "strictly sorted and unique")]
    fn selected_receiver_duplicates_are_rejected() {
        validate_selected_receiver_indices(&[1, 1]);
    }

    #[test]
    #[should_panic(expected = "outside the tile")]
    fn selected_receiver_out_of_range_is_rejected() {
        validate_selected_receiver_indices(&[TILE_PX * TILE_PX]);
    }

    #[test]
    #[should_panic(expected = "duplicate pixel index")]
    fn compact_output_duplicates_are_rejected() {
        let mut accum = TileAccumulator::new();
        merge_compact_output(
            &mut accum,
            vec![(3, [1.0; NUM_PERIODS]), (3, [2.0; NUM_PERIODS])],
        );
    }
}
