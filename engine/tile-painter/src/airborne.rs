//! Airborne scatter onto a Web Mercator tile.
//!
//! Mirrors `noise_compute::compute::aircraft_v6::airborne::scatter`
//! without popup-only state (FlightAccum, peak_lmax, TopFlightCandidate,
//! SegmentTrace). Heatmap energy is commutative — per-segment accumulation
//! alone is correct (Decision #12).
//!
//! Per-row bbox prefilter drops out-of-reach rows before the sub-segment
//! loop. Extract-time endpoint terrain samples feed the stale-ground and
//! Filter D checks; the airborne path does not run a five-point terrain
//! validity gate. Admission (`admit_row`) is row-parallel and its result keeps
//! input order; the scatter passes are then parallel over RECEIVER rows, the
//! same rule the cruise painter follows — see [`AdmittedSubSegment`].

use std::sync::atomic::{AtomicU64, Ordering};

use noise_compute::compute::aircraft_v6::{AirborneRowView, BBox};
use noise_compute::emission::aircraft::{
    self, NpdLuts, SegmentTerrain, AIRCRAFT_MAX_HORIZONTAL_REACH_M, GROUND_CONTEXT_NONE,
    GROUND_OPS_KIND_NONE, M_PER_DEG_LAT,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::AircraftSegment;
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13};
use rayon::prelude::*;

use crate::accumulator::{CoarseLevels, TileAccumulator, COARSE_LEVELS_N, NUM_PERIODS};
use crate::airborne_screening::ReceiverScreeningGrid;
use crate::grid::TILE_PX;
use crate::source_loader_structure::InteriorEstimate;

/// Slant cutoff between the exact per-pixel path and the coarse lattice.
/// A segment whose nearest tile pixel is closer than this is evaluated at
/// every pixel; farther (smooth-field) segments go to the coarse lattice.
///
/// Tuned by sweep against the exact baseline on the two LKPR reference
/// tiles (the dense low-overflight corridor tile 4417,2771 and the lighter
/// 4415,2787): at 500 m every cell matched to within one HM3 quantization
/// step (0.5 dB max, 0 cells over, mean < 0.004 dB) on both, i.e. the
/// residual is quantization noise, not method error. 250 m was also clean
/// but barely faster (the far-coarse work, not the near path, then
/// dominates), so 500 m keeps a 2× margin to the ~150 m point where the
/// single-segment bilinear error (≈ 8.69·(cell/2)²/slant²) reaches 0.5 dB.
///
/// Caveat — validated only on gentle Prague terrain. The coarse lattice
/// samples receiver elevation at its 65² nodes and interpolates *energy*,
/// so terrain curvature inside a ~48 m cell is unmodelled. On rugged tiles
/// (Alpine / fjord approaches) ~15-20 m of un-sampled sub-cell relief under
/// a near-floor far segment could exceed 0.5 dB. Before trusting this on
/// mountainous regions, re-validate with `compare_hm3` exact-vs-coarse on
/// such a tile (e.g. LOWI) and, if it bites, raise the cutoff with tile
/// terrain roughness (`max(500, k·(max-min rx_alt))`).
const NEAR_SLANT_M: f64 = 500.0;

/// Diagnostic escape hatch: with `QM_AIRBORNE_FORCE_EXACT=1` every admitted
/// sub-segment takes the exact per-pixel path (the coarse lattice is
/// bypassed), producing the ground-truth oracle that `compare_hm3` diffs the
/// adaptive build against. Read once per tile (not per segment); unset in all
/// production builds. Kept as the regression oracle for coarse-lattice drift.
fn force_exact() -> bool {
    std::env::var("QM_AIRBORNE_FORCE_EXACT").is_ok_and(|v| v == "1")
}

/// Far-field coarse-stride band edges, in metres of best-case slant
/// (`best_slant` = the minimum slant from a segment to any tile pixel). A
/// far segment scatters onto the [`CoarseLevels`] lattice of the band its
/// `best_slant` falls in: `< 2 km → level 0`, `2–8 km → level 1`,
/// `≥ 8 km → level 2` (per-level lattice size and stride: see
/// [`crate::accumulator::COARSE_LEVELS_N`], the single source of truth).
///
/// SIZING HEURISTIC (not a proof): a node cell's bilinear error is
/// `≈ 8.69·(cell/2)²/slant²` dB (the relation that also sets `NEAR_SLANT_M`).
/// Holding error fixed, the cell may grow ∝ slant — a dyadic ladder where
/// slant ×4 lets stride ×4 (nodes ÷16) at ~constant error. At Praha
/// (~6.1 m/px) each band lands at ~0.08 dB at its NEAR (worst, binding)
/// edge, a ~6× margin under the 0.5 dB HM3 step. That formula only describes
/// the smooth spreading term; the full kernel also has the NPD tail, ΔF,
/// lateral/ΔI (< 7.62 km) and the SEL floor, so these edges are EMPIRICAL
/// parameters — the real guard is the adaptive-vs-exact `e2-airborne`
/// diagnostic, not this arithmetic.
///
/// Routing on the MINIMUM slant over the tile is the conservative choice
/// (the nearest pixel sets the fineness; farther pixels only want it
/// coarser). It matches the kernel CPA for on-segment receivers; off-end
/// `best_slant` can overestimate, but that geometry is exactly where ΔF
/// suppresses the contribution 3–10 dB.
///
/// Terrain and roof screening are evaluated at the same lattice nodes as the
/// far segment energy, then bilinearly expanded. Local low-overflight segments
/// inside 500 m take the exact 6 m receiver lattice—the visitor-visible house
/// shadows this wave targets. A far segment's shadow inherits the established
/// coarse interpolation and is therefore intentionally no sharper than its
/// smooth NPD field. Airborne is altitude-capped near FL250,
/// so a >8 km-slant segment is usually a distant LOW segment (slant from
/// horizontal distance); the level is then insensitive to receiver-elevation
/// variation (d(level)/d(rx_alt) is tiny at large slant) and the field stays
/// near-linear across the cell. The near-floor-over-rugged-relief danger
/// lives in the NEAR (exact) path and the <2 km band (unchanged n=65),
/// neither coarsened here.
const COARSE_BAND_M: [f64; 2] = [2_000.0, 8_000.0];

// One coarser CoarseLevels lattice per band, plus one below the first edge.
const _: () = assert!(crate::accumulator::COARSE_LEVELS_N.len() == COARSE_BAND_M.len() + 1);

/// The painter's one production entry into the acoustic kernel: terrain is
/// mandatory, while an empty roof horizon takes the bit-stable terrain-only
/// path. Exact pixels and every coarse lattice call this same wrapper.
#[inline]
fn screened_segment_energy(
    prepared: &aircraft::SegmentPrepared,
    row_state: &aircraft::SegmentRowState,
    receiver_lon: f64,
    receiver_alt_m: f64,
    npd_luts: &NpdLuts,
    terrain: &aircraft::ReceiverHorizon,
    buildings: Option<&aircraft::BuildingHorizon>,
) -> Option<f64> {
    if let Some(buildings) = buildings {
        aircraft::segment_sel_at_pixel_energy_screened(
            prepared,
            row_state,
            receiver_lon,
            receiver_alt_m,
            npd_luts,
            terrain,
            buildings,
        )
    } else {
        aircraft::segment_sel_at_pixel_energy(
            prepared,
            row_state,
            receiver_lon,
            receiver_alt_m,
            npd_luts,
            Some(terrain),
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AirborneStats {
    pub rows_seen: usize,
    pub rows_bbox_pass: usize,
    pub sub_segments_seen: u64,
    pub sub_segments_outside_tile: u64,
    pub sub_segments_slant_pruned: u64,
    pub sub_segments_coarse: u64,
    pub sub_segments_invalid: u64,
    pub pairs_evaluated: u64,
    pub pairs_below_threshold: u64,
    /// Sub-segments on the exact per-pixel (near) path. With `coarse_band`
    /// this is the near/far work split — the Amdahl telemetry that explains
    /// why near-heavy airport tiles gain least from the adaptive far stride.
    pub sub_near: u64,
    /// Coarse sub-segments per stride level, binned by `best_slant` (m):
    /// `[<2 km, 2-8 km, ≥8 km]`. Confirms the routing hits the bands the
    /// field distribution predicts.
    pub coarse_band: [u64; 3],
}

impl AirborneStats {
    /// Sum per-thread / per-tile partials (the rayon reduce + `RegionStats`).
    pub fn merge(&mut self, o: &AirborneStats) {
        self.rows_seen += o.rows_seen;
        self.rows_bbox_pass += o.rows_bbox_pass;
        self.sub_segments_seen += o.sub_segments_seen;
        self.sub_segments_outside_tile += o.sub_segments_outside_tile;
        self.sub_segments_slant_pruned += o.sub_segments_slant_pruned;
        self.sub_segments_coarse += o.sub_segments_coarse;
        self.sub_segments_invalid += o.sub_segments_invalid;
        self.pairs_evaluated += o.pairs_evaluated;
        self.pairs_below_threshold += o.pairs_below_threshold;
        self.sub_near += o.sub_near;
        for i in 0..3 {
            self.coarse_band[i] += o.coarse_band[i];
        }
    }
}

#[derive(Clone, Copy)]
struct TileDistanceContext {
    centre_lat: f64,
    centre_lon: f64,
    m_per_deg_lon: f64,
    half_diag_m: f64,
    max_receiver_alt_m: f64,
    /// Squared reach cap from tile centre — a sub-segment farther than this
    /// reaches no pixel of the tile.
    prune_radius_sq: f64,
    near_min_lat: f64,
    near_max_lat: f64,
    near_min_lon: f64,
    near_max_lon: f64,
    near_lon_prune_active: bool,
}

impl TileDistanceContext {
    fn minimum_horizontal_distance_sq(
        self,
        start_lat: f64,
        start_lon: f64,
        end_lat: f64,
        end_lon: f64,
    ) -> f64 {
        let x1 = (start_lon - self.centre_lon) * self.m_per_deg_lon;
        let y1 = (start_lat - self.centre_lat) * M_PER_DEG_LAT;
        let x2 = (end_lon - self.centre_lon) * self.m_per_deg_lon;
        let y2 = (end_lat - self.centre_lat) * M_PER_DEG_LAT;
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len_sq = dx * dx + dy * dy;
        if len_sq < 1.0 {
            x1 * x1 + y1 * y1
        } else {
            let t_num = -(x1 * dx + y1 * dy);
            if t_num <= 0.0 {
                x1 * x1 + y1 * y1
            } else if t_num >= len_sq {
                x2 * x2 + y2 * y2
            } else {
                let cross = dx * y1 - dy * x1;
                (cross * cross) / len_sq
            }
        }
    }

    fn best_slant_sq(self, horizontal_sq: f64, start_alt_m: f64, end_alt_m: f64) -> f64 {
        let horizontal_m = (horizontal_sq.sqrt() - self.half_diag_m).max(0.0);
        let relative_alt_m = (start_alt_m.min(end_alt_m) - self.max_receiver_alt_m).max(0.0);
        horizontal_m * horizontal_m + relative_alt_m * relative_alt_m
    }

    /// Conservative prepass: a false positive only builds extra horizons;
    /// every segment that can enter the exact path must make this return true.
    fn has_exact_candidate(self, airborne: &[AirborneRowView<'_>]) -> bool {
        airborne.par_iter().any(|row| {
            let bbox = &row.bbox;
            if f64::from(bbox.max_lat) < self.near_min_lat
                || f64::from(bbox.min_lat) > self.near_max_lat
            {
                return false;
            }
            if self.near_lon_prune_active
                && (f64::from(bbox.max_lon) < self.near_min_lon
                    || f64::from(bbox.min_lon) > self.near_max_lon)
            {
                return false;
            }
            (0..row.sub_segments.len()).any(|i| {
                let horizontal_sq = self.minimum_horizontal_distance_sq(
                    row.sub_segments.start_lat[i] as f64,
                    row.sub_segments.start_lon[i] as f64,
                    row.sub_segments.end_lat[i] as f64,
                    row.sub_segments.end_lon[i] as f64,
                );
                self.best_slant_sq(
                    horizontal_sq,
                    row.sub_segments.start_alt_m[i] as f64,
                    row.sub_segments.end_alt_m[i] as f64,
                ) < NEAR_SLANT_M * NEAR_SLANT_M
            })
        })
    }
}

/// One airborne sub-segment admitted to this tile: the hoisted per-segment kernel
/// state, the period it lands in, its GA hybrid class weight, and the receiver lattice
/// it scatters onto (`None` = the exact per-pixel path).
struct AdmittedSubSegment {
    prepared: aircraft::SegmentPrepared,
    period_idx: u8,
    class_weight: f32,
    coarse_level: Option<usize>,
}

/// Row-parallel admission counters. Integer sums are order-free, so — unlike the
/// energy, whose summation order decides a cell's quantised byte — the prune tallies
/// need no ordered reduction. The near/coarse split is counted from the admitted list
/// itself, so only the rejections live here.
#[derive(Default)]
struct AdmissionCounters {
    rows_bbox_pass: AtomicU64,
    sub_segments_seen: AtomicU64,
    sub_segments_outside_tile: AtomicU64,
    sub_segments_invalid: AtomicU64,
    sub_segments_slant_pruned: AtomicU64,
}

impl AdmissionCounters {
    /// One publish per row that passed the tile envelope, not per sub-segment.
    fn record_row(&self, seen: u64, outside_tile: u64, invalid: u64, slant_pruned: u64) {
        self.rows_bbox_pass.fetch_add(1, Ordering::Relaxed);
        self.sub_segments_seen.fetch_add(seen, Ordering::Relaxed);
        self.sub_segments_outside_tile
            .fetch_add(outside_tile, Ordering::Relaxed);
        self.sub_segments_invalid
            .fetch_add(invalid, Ordering::Relaxed);
        self.sub_segments_slant_pruned
            .fetch_add(slant_pruned, Ordering::Relaxed);
    }

    fn into_stats(self) -> AirborneStats {
        AirborneStats {
            rows_bbox_pass: self.rows_bbox_pass.into_inner() as usize,
            sub_segments_seen: self.sub_segments_seen.into_inner(),
            sub_segments_outside_tile: self.sub_segments_outside_tile.into_inner(),
            sub_segments_invalid: self.sub_segments_invalid.into_inner(),
            sub_segments_slant_pruned: self.sub_segments_slant_pruned.into_inner(),
            ..AirborneStats::default()
        }
    }
}

/// The tile's lat/lon rejection envelope: a row whose bbox misses it reaches no pixel.
/// Same shape as the popup's single-receiver envelope (airborne.rs:84-99), sized around
/// tile centre + half-diagonal instead of one point.
#[derive(Clone, Copy)]
struct TileEnvelope {
    min_lat: f32,
    max_lat: f32,
    min_lon: f32,
    max_lon: f32,
    /// Antimeridian-safe longitude prune (matches popup airborne.rs): off when the
    /// envelope wraps past ±180°.
    lon_prune_active: bool,
}

impl TileEnvelope {
    fn admits(self, bbox: &BBox) -> bool {
        if bbox.max_lat < self.min_lat || bbox.min_lat > self.max_lat {
            return false;
        }
        !(self.lon_prune_active && (bbox.max_lon < self.min_lon || bbox.min_lon > self.max_lon))
    }
}

/// Everything a pixel needs besides the sub-segment itself: the receiver lattice, the
/// per-row longitude scale, the NPD LUTs and the pixel-indexed screening horizons.
struct ReceiverField<'a> {
    tile: &'a FusedTileZ13,
    m_per_deg_lon_row: &'a [f64; TILE_PX],
    npd_luts: &'a NpdLuts,
    screening: &'a ReceiverScreeningGrid,
}

impl ReceiverField<'_> {
    #[inline]
    fn screened_energy(
        &self,
        prepared: &aircraft::SegmentPrepared,
        row_state: &aircraft::SegmentRowState,
        pixel: usize,
    ) -> Option<f64> {
        let (horizon, buildings) = self.screening.at(pixel);
        screened_segment_energy(
            prepared,
            row_state,
            self.tile.rx_lon[pixel % TILE_PX],
            self.tile.rx_alt_m[pixel] as f64,
            self.npd_luts,
            horizon,
            buildings,
        )
    }
}

pub fn scatter_tile(
    tile: &FusedTileZ13,
    airborne: &[AirborneRowView<'_>],
    // GA hybrid per-class weight LUT.
    // Each sub-segment's energy is scaled by `class_weights.get(class)` so a
    // GA one-off divides by `ga_n_days`, not `n_days`. Uniform for non-hybrid
    // extracts (byte-identical to the pre-hybrid scatter).
    class_weights: &aircraft::ClassWeights,
    obstacles: &ObstacleSet,
    interior: &InteriorEstimate,
    accum: &mut TileAccumulator,
) -> AirborneStats {
    let npd_luts = NpdLuts::shared();

    // Per-tile rejection envelope in lat/lon degrees. Same shape as the
    // popup's single-receiver envelope (airborne.rs:84-99), sized around
    // tile centre + half-diagonal instead of one point.
    let bbox = &tile.bbox;
    let tile_centre_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
    let tile_centre_lon = (bbox.east_lon + bbox.west_lon) * 0.5;
    let px_m = tile_pixel_size_m(tile.zoom, tile_centre_lat);
    let half_diag_m = (TILE_PX as f64) * px_m * std::f64::consts::SQRT_2 * 0.5;
    let prune_radius_m = AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m;
    let radius_lat_deg = aircraft::meters_to_lat_deg(prune_radius_m);
    let radius_lon_deg = aircraft::meters_to_lon_deg(tile_centre_lat, prune_radius_m);
    let env_min_lon_raw = tile_centre_lon - radius_lon_deg;
    let env_max_lon_raw = tile_centre_lon + radius_lon_deg;
    let envelope = TileEnvelope {
        min_lat: (tile_centre_lat - radius_lat_deg) as f32,
        max_lat: (tile_centre_lat + radius_lat_deg) as f32,
        min_lon: env_min_lon_raw as f32,
        max_lon: env_max_lon_raw as f32,
        lon_prune_active: env_min_lon_raw >= -180.0 && env_max_lon_raw <= 180.0,
    };

    // `m_per_deg_lon` depends only on the row latitude (tile-invariant), so
    // precompute the TILE_PX row values once instead of recomputing `cos()` in
    // `prepare_row` per (sub-seg × row).
    let m_per_deg_lon_row: [f64; TILE_PX] =
        std::array::from_fn(|py| M_PER_DEG_LAT * tile.rx_lat[py].to_radians().cos().max(0.2));

    // Tile-centre-local meter projection for the per-sub-seg perpendicular
    // prune. Uses the Doc 29 `M_PER_DEG_LAT` so projection matches the
    // kernel's own distance math (`segment_energy_kernel` projects rx-relative).
    let cos_tile_lat = tile_centre_lat.to_radians().cos().max(0.2);
    let m_per_deg_lon = M_PER_DEG_LAT * cos_tile_lat;

    // Highest receiver on the tile — used by the per-sub-seg slant reach
    // gate. A sub-seg's smallest possible slant to ANY tile pixel uses the
    // closest horizontal point AND the tallest receiver (smallest rel_alt).
    let tile_max_rx_alt = tile
        .rx_alt_m
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max) as f64;
    let near_centre_radius_m = NEAR_SLANT_M + half_diag_m;
    let near_lat_deg = aircraft::meters_to_lat_deg(near_centre_radius_m);
    let near_lon_deg = aircraft::meters_to_lon_deg(tile_centre_lat, near_centre_radius_m);
    let near_min_lon_raw = tile_centre_lon - near_lon_deg;
    let near_max_lon_raw = tile_centre_lon + near_lon_deg;

    let tile_distance = TileDistanceContext {
        centre_lat: tile_centre_lat,
        centre_lon: tile_centre_lon,
        m_per_deg_lon,
        half_diag_m,
        max_receiver_alt_m: tile_max_rx_alt,
        prune_radius_sq: prune_radius_m * prune_radius_m,
        near_min_lat: tile_centre_lat - near_lat_deg,
        near_max_lat: tile_centre_lat + near_lat_deg,
        near_min_lon: near_min_lon_raw,
        near_max_lon: near_max_lon_raw,
        near_lon_prune_active: near_min_lon_raw >= -180.0 && near_max_lon_raw <= 180.0,
    };

    let near_slant_sq = NEAR_SLANT_M * NEAR_SLANT_M;
    let force_exact = force_exact();
    // Receiver-scoped screening is built in pixel order before the
    // segment-parallel scatter. Initialising lazily inside that scatter makes
    // every worker reach the same cold pixel together and serialize behind a
    // once-lock; this prepass has no locks and reuses one crossing buffer per
    // Rayon worker.
    let receiver_screening = ReceiverScreeningGrid::build(
        tile,
        obstacles,
        interior,
        force_exact || tile_distance.has_exact_candidate(airborne),
    );

    // One admitted sub-segment per surviving (row, sub-segment), materialised in INPUT
    // order; every scatter pass below then parallelises over RECEIVERS, so each
    // accumulator cell is summed by exactly one task over one fixed sub-segment order.
    // That is what makes the painted tile bit-reproducible — a rayon `fold` + `reduce`
    // over sub-segments merges partial f32 sums in work-stealing order instead, and the
    // sibling cruise painter measurably flipped cells across the 0.5 dB quantisation
    // step between two runs of one binary on one host (2026-09-04).
    let counters = AdmissionCounters::default();
    let admitted: Vec<AdmittedSubSegment> = airborne
        .par_iter()
        .flat_map_iter(|row| {
            admit_row(
                row,
                class_weights,
                envelope,
                tile_distance,
                force_exact,
                near_slant_sq,
                &counters,
            )
            .into_iter()
        })
        .collect();

    let mut st = counters.into_stats();
    let mut near: Vec<&AdmittedSubSegment> = Vec::new();
    let mut coarse_by_level: [Vec<&AdmittedSubSegment>; COARSE_LEVELS_N.len()] =
        std::array::from_fn(|_| Vec::new());
    for sub in &admitted {
        match sub.coarse_level {
            None => {
                st.sub_near += 1;
                near.push(sub)
            }
            Some(level) => {
                st.sub_segments_coarse += 1;
                st.coarse_band[level] += 1;
                coarse_by_level[level].push(sub)
            }
        }
    }

    let field = ReceiverField {
        tile,
        m_per_deg_lon_row: &m_per_deg_lon_row,
        npd_luts,
        screening: &receiver_screening,
    };
    let mut coarse = CoarseLevels::new();
    let (mut evaluated, mut below) = scatter_near_pixels(&near, &field, accum);
    for (level, subs) in coarse_by_level.iter().enumerate() {
        let (level_evaluated, level_below) = scatter_coarse_level(subs, level, &field, &mut coarse);
        evaluated += level_evaluated;
        below += level_below;
    }
    coarse.expand_into(accum);

    st.rows_seen = airborne.len();
    st.pairs_evaluated = evaluated;
    st.pairs_below_threshold = below;
    st
}

/// Resolve one airborne row's sub-segments against this tile: the row bbox envelope,
/// then per sub-segment the clamped-CPA prune, the endpoint ground-stale gate, the
/// per-class slant reach gate, and finally the near/coarse routing.
fn admit_row(
    row: &AirborneRowView<'_>,
    class_weights: &aircraft::ClassWeights,
    envelope: TileEnvelope,
    tile_distance: TileDistanceContext,
    force_exact: bool,
    near_slant_sq: f64,
    counters: &AdmissionCounters,
) -> Vec<AdmittedSubSegment> {
    let mut admitted = Vec::new();
    if !envelope.admits(&row.bbox) {
        return admitted;
    }
    let mut outside_tile = 0u64;
    let mut invalid = 0u64;
    let mut slant_pruned = 0u64;

    // GA hybrid weight for this row's class — row-constant (one airborne row =
    // one flight = one class). Folded into every sub-segment's energy before scatter.
    let class_weight = class_weights.get(aircraft::noise_class_of(row.profile_idx)) as f32;

    let n = row.sub_segments.len();
    for i in 0..n {
        let flags = row.sub_segments.flags[i];
        let is_departure = flags & 0b001 != 0;
        let start_lat = row.sub_segments.start_lat[i] as f64;
        let start_lon = row.sub_segments.start_lon[i] as f64;
        let end_lat = row.sub_segments.end_lat[i] as f64;
        let end_lon = row.sub_segments.end_lon[i] as f64;

        // Clamped CPA from tile centre to sub-seg's physical extent.
        // Infinite-line variant (M11a) admitted sub-segs whose LINE
        // grazed the tile but whose endpoints sat far away — the
        // per-pixel kernel then rejected them 262 144× via its own
        // CPA gate. Strictly more conservative; one decision per
        // sub-seg. Degenerate (<1 m) → start-endpoint distance.
        let min_d_sq =
            tile_distance.minimum_horizontal_distance_sq(start_lat, start_lon, end_lat, end_lon);
        if min_d_sq > tile_distance.prune_radius_sq {
            outside_tile += 1;
            continue;
        }
        // Hardcoded NONE: the `is_near_airport` carve-out (which
        // could promote sub-segs to AIRPORT_LINE) was empirically
        // never firing — see commit a5a6bf3 on the popup-side
        // mirror for the 5-receiver Step-0 measurement.
        let seg = AircraftSegment {
            flight_id: row.flight_id,
            profile_idx: row.profile_idx,
            is_departure,
            on_ground: false,
            period: row.sub_segments.period[i],
            date_id: row.sub_segments.date_id[i],
            start_lat,
            start_lon,
            start_alt_m: row.sub_segments.start_alt_m[i],
            end_lat,
            end_lon,
            end_alt_m: row.sub_segments.end_alt_m[i],
            speed_kt: row.sub_segments.speed_kt[i],
            segment_length_m: row.sub_segments.length_m[i],
            count_weight: 1.0,
            surface_model: false,
            ground_context: GROUND_CONTEXT_NONE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            source_id: row.source_id as u16,
        };
        // Only start/end terrain elevations are stored. Keep the
        // endpoint ground-stale gate (start/end AGL ≤ 15 m).
        // Filter D extrapolation cuts pre-computed once per
        // sub-seg from the two endpoint elevs, then passed into
        // the per-pixel `segment_sel_with_cuts` call so the
        // pixel loop pays zero raster I/O.
        let start_elev = row.sub_segments.terrain_start_elev_m[i] as f64;
        let end_elev = row.sub_segments.terrain_end_elev_m[i] as f64;
        let terrain = SegmentTerrain {
            start_elev,
            q1_elev: 0.0,
            mid_elev: 0.0,
            q3_elev: 0.0,
            end_elev,
        };
        if aircraft::is_ground_stale_with_terrain(&seg, &terrain) {
            invalid += 1;
            continue;
        }

        // Three-level hoist: sub-seg constants out of the 262 144-pixel loop; row
        // constants (cos_lat-derived) out of the 512-pixel loop. See
        // `segment_sel::prepare_segment` for the split.
        let prepared = aircraft::prepare_segment(&seg, start_elev - 30.0, end_elev - 30.0);

        // Slant reach gate (Phase 2 port from popup airborne.rs). The
        // clamped-CPA prune above is horizontal-only at the 16 km
        // max-class radius, so a high-altitude overflight with small
        // horizontal CPA survives it, then rejects at all 262 144
        // pixels via the kernel's own `slant_sq > reach_sq`
        // (doc29.rs:366). Hoist that to one decision per sub-seg: the
        // smallest slant any tile pixel can see pairs the closest
        // horizontal approach with the segment's lowest altitude over
        // the tallest receiver. `reach_sq` is per-class, so this
        // rejects only what the kernel already rejects everywhere — a
        // provable lower bound, exact rather than a heuristic.
        let best_slant_sq = tile_distance.best_slant_sq(
            min_d_sq,
            prepared.start_alt_m,
            prepared.start_alt_m + prepared.sdz,
        );
        if best_slant_sq > prepared.reach_sq {
            slant_pruned += 1;
            continue;
        }

        // Near/far split (the artifact-safe coarse strategy): a segment whose
        // nearest pixel is within NEAR_SLANT_M has a sharp field — evaluate every
        // pixel exactly, since coarsening it would alias into visible blocky
        // squares. Farther segments vary slowly across the tile, so sample the
        // coarsest lattice whose bilinear error stays under the HM3 step at this
        // segment's nearest slant (see COARSE_BAND_M) and expand it once.
        let coarse_level = if force_exact || best_slant_sq < near_slant_sq {
            None
        } else {
            let best_slant = best_slant_sq.sqrt();
            Some(if best_slant < COARSE_BAND_M[0] {
                0
            } else if best_slant < COARSE_BAND_M[1] {
                1
            } else {
                2
            })
        };
        admitted.push(AdmittedSubSegment {
            prepared,
            period_idx: seg.period.min(2),
            class_weight,
            coarse_level,
        });
    }
    counters.record_row(n as u64, outside_tile, invalid, slant_pruned);
    admitted
}

/// Sum the near sub-segments into `accum`, parallel over receiver pixel ROWS: one task
/// owns a pixel row and walks the sub-segments in order, so the pixel's energy has one
/// fixed summation order. Returns `(kernel evals, evals below the SEL floor)`.
fn scatter_near_pixels(
    subs: &[&AdmittedSubSegment],
    field: &ReceiverField<'_>,
    accum: &mut TileAccumulator,
) -> (u64, u64) {
    let below = AtomicU64::new(0);
    accum
        .energy
        .par_chunks_mut(TILE_PX * NUM_PERIODS)
        .enumerate()
        .for_each(|(py, pixel_row)| {
            let rx_lat = field.tile.rx_lat[py];
            let row_base = py * TILE_PX;
            let mut row_below = 0u64;
            for sub in subs {
                let row_state =
                    aircraft::prepare_row(&sub.prepared, rx_lat, field.m_per_deg_lon_row[py]);
                for px in 0..TILE_PX {
                    let Some(sel) = field.screened_energy(&sub.prepared, &row_state, row_base + px)
                    else {
                        row_below += 1;
                        continue;
                    };
                    pixel_row[px * NUM_PERIODS + sub.period_idx as usize] +=
                        sub_segment_energy(sel, sub.class_weight);
                }
            }
            below.fetch_add(row_below, Ordering::Relaxed);
        });
    (
        (subs.len() * TILE_PX * TILE_PX) as u64,
        below.load(Ordering::Relaxed),
    )
}

/// Sum one coarse band's sub-segments onto its lattice, parallel over lattice node ROWS
/// — the same one-task-per-receiver-row rule as the near pass. Terrain and roof
/// screening are evaluated at these same nodes and the whole field is expanded once, so
/// a far segment's shadow is no sharper than its smooth NPD field.
/// Returns `(kernel evals, evals below the SEL floor)`.
fn scatter_coarse_level(
    subs: &[&AdmittedSubSegment],
    level: usize,
    field: &ReceiverField<'_>,
    coarse: &mut CoarseLevels,
) -> (u64, u64) {
    coarse
        .level_mut(level)
        .scatter_in_fixed_parts(subs, |chunk, part| {
            let n = part.n();
            let mut below = 0u64;
            for sub in chunk {
                for ci in 0..n {
                    let py = part.coarse_pixel(ci);
                    let rx_lat = field.tile.rx_lat[py];
                    let row_state =
                        aircraft::prepare_row(&sub.prepared, rx_lat, field.m_per_deg_lon_row[py]);
                    let row_base = py * TILE_PX;
                    for cj in 0..n {
                        let px = part.coarse_pixel(cj);
                        let Some(sel) =
                            field.screened_energy(&sub.prepared, &row_state, row_base + px)
                        else {
                            below += 1;
                            continue;
                        };
                        part.add_energy_at(
                            ci,
                            cj,
                            sub.period_idx,
                            sub_segment_energy(sel, sub.class_weight),
                        );
                    }
                }
            }
            ((chunk.len() * n * n) as u64, below)
        })
}

/// SEL (dB) → linear energy for one sub-segment, weighted by its GA hybrid class weight.
/// `fast_exp_f64` is the Padé exponential the popup shares
/// (`aircraft_v6/airborne.rs:224`); the f32 narrowing happens before the weight, as it
/// did when this multiply sat inline in the pixel loop.
#[inline]
fn sub_segment_energy(sel: f64, class_weight: f32) -> f32 {
    noise_compute::propagation::iso9613::fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) as f32
        * class_weight
}
