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
//! validity gate. A per-thread accumulator
//! with rayon fold/reduce mirrors cruise and ground_ops; per-thread stats
//! (`AirborneStats`) fold the same way — no atomics, no hot-loop contention.

use noise_compute::compute::aircraft_v6::AirborneRowView;
use noise_compute::emission::aircraft::{
    self, NpdLuts, SegmentTerrain, AIRCRAFT_MAX_HORIZONTAL_REACH_M, GROUND_CONTEXT_NONE,
    GROUND_OPS_KIND_NONE, M_PER_DEG_LAT,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::AircraftSegment;
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13};
use rayon::prelude::*;

use crate::accumulator::{CoarseLevels, TileAccumulator};
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
    let radius_lat_deg = aircraft::meters_to_lat_deg(AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m);
    let radius_lon_deg = aircraft::meters_to_lon_deg(
        tile_centre_lat,
        AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m,
    );
    let env_min_lat = (tile_centre_lat - radius_lat_deg) as f32;
    let env_max_lat = (tile_centre_lat + radius_lat_deg) as f32;
    let env_min_lon_raw = tile_centre_lon - radius_lon_deg;
    let env_max_lon_raw = tile_centre_lon + radius_lon_deg;
    // Antimeridian-safe longitude prune (matches popup airborne.rs).
    let lon_prune_active = env_min_lon_raw >= -180.0 && env_max_lon_raw <= 180.0;
    let env_min_lon = env_min_lon_raw as f32;
    let env_max_lon = env_max_lon_raw as f32;

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
    let prune_radius_m = AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m;
    let prune_radius_sq = prune_radius_m * prune_radius_m;

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

    let (mut local, coarse, mut st) = airborne
        .par_iter()
        .fold(
            || {
                (
                    TileAccumulator::new(),
                    CoarseLevels::new(),
                    AirborneStats::default(),
                )
            },
            |(mut local, mut coarse, mut st), row| {
                let bb = &row.bbox;
                if bb.max_lat < env_min_lat || bb.min_lat > env_max_lat {
                    return (local, coarse, st);
                }
                if lon_prune_active && (bb.max_lon < env_min_lon || bb.min_lon > env_max_lon) {
                    return (local, coarse, st);
                }
                st.rows_bbox_pass += 1;

                // GA hybrid weight for this row's class — row-constant (one
                // airborne row = one flight = one class). Folded into every
                // sub-segment's energy before scatter.
                let class_weight =
                    class_weights.get(aircraft::noise_class_of(row.profile_idx)) as f32;

                let n = row.sub_segments.len();
                st.sub_segments_seen += n as u64;
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
                    let min_d_sq = tile_distance
                        .minimum_horizontal_distance_sq(start_lat, start_lon, end_lat, end_lon);
                    if min_d_sq > prune_radius_sq {
                        st.sub_segments_outside_tile += 1;
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
                        st.sub_segments_invalid += 1;
                        continue;
                    }

                    let start_cut = start_elev - 30.0;
                    let end_cut = end_elev - 30.0;
                    let period_idx = seg.period.min(2);
                    let mut local_eval = 0usize;
                    let mut local_below = 0usize;
                    // Three-level hoist: sub-seg constants out of 262 144-pixel
                    // loop; row constants (cos_lat-derived) out of 512-pixel
                    // loop. See `segment_sel::prepare_segment` for the split.
                    let prepared = aircraft::prepare_segment(&seg, start_cut, end_cut);

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
                        st.sub_segments_slant_pruned += 1;
                        continue;
                    }

                    // Near/far split (the artifact-safe coarse strategy): a
                    // segment whose nearest pixel is within NEAR_SLANT_M has a
                    // sharp field — evaluate every pixel exactly, since coarsening
                    // it would alias into visible blocky squares. Farther segments
                    // vary slowly across the 3 km tile, so sample a coarse
                    // lattice and bilinearly expand once after the reduce.
                    // `prepare_row` is hoisted out of the inner loop in both paths
                    // (it depends only on the receiver row latitude). sel → linear
                    // energy via fast_exp_f64 (Padé 5th-order, < 0.001 dB error),
                    // matching the popup at aircraft_v6/airborne.rs:224.
                    if force_exact || best_slant_sq < near_slant_sq {
                        st.sub_near += 1;
                        for py in 0..TILE_PX as u32 {
                            let rx_lat = tile.rx_lat[py as usize];
                            let row_state = aircraft::prepare_row(
                                &prepared,
                                rx_lat,
                                m_per_deg_lon_row[py as usize],
                            );
                            let row_base = (py as usize) * TILE_PX;
                            for px in 0..TILE_PX as u32 {
                                let rx_lon = tile.rx_lon[px as usize];
                                let pixel = row_base + px as usize;
                                let rx_alt = tile.rx_alt_m[pixel] as f64;
                                local_eval += 1;
                                let (horizon, buildings) = receiver_screening.at(pixel);
                                let screened = screened_segment_energy(
                                    &prepared, &row_state, rx_lon, rx_alt, npd_luts, horizon,
                                    buildings,
                                );
                                let Some(sel) = screened else {
                                    local_below += 1;
                                    continue;
                                };
                                local.add_energy_at(
                                    py,
                                    px,
                                    period_idx,
                                    noise_compute::propagation::iso9613::fast_exp_f64(
                                        sel * std::f64::consts::LN_10 * 0.1,
                                    ) as f32
                                        * class_weight,
                                );
                            }
                        }
                    } else {
                        st.sub_segments_coarse += 1;
                        // Adaptive far-field stride: route to the coarsest lattice
                        // whose bilinear error stays under the HM3 step at this
                        // segment's nearest slant (see COARSE_BAND_M).
                        let best_slant = best_slant_sq.sqrt();
                        let level = if best_slant < COARSE_BAND_M[0] {
                            0
                        } else if best_slant < COARSE_BAND_M[1] {
                            1
                        } else {
                            2
                        };
                        st.coarse_band[level] += 1;
                        let cn = coarse.n(level);
                        for ci in 0..cn {
                            let py = coarse.coarse_pixel(level, ci);
                            let rx_lat = tile.rx_lat[py];
                            let row_state =
                                aircraft::prepare_row(&prepared, rx_lat, m_per_deg_lon_row[py]);
                            let row_base = py * TILE_PX;
                            for cj in 0..cn {
                                let px = coarse.coarse_pixel(level, cj);
                                let rx_lon = tile.rx_lon[px];
                                let pixel = row_base + px;
                                let rx_alt = tile.rx_alt_m[pixel] as f64;
                                local_eval += 1;
                                let (horizon, buildings) = receiver_screening.at(pixel);
                                let screened = screened_segment_energy(
                                    &prepared, &row_state, rx_lon, rx_alt, npd_luts, horizon,
                                    buildings,
                                );
                                let Some(sel) = screened else {
                                    local_below += 1;
                                    continue;
                                };
                                coarse.add_energy_at(
                                    level,
                                    ci,
                                    cj,
                                    period_idx,
                                    noise_compute::propagation::iso9613::fast_exp_f64(
                                        sel * std::f64::consts::LN_10 * 0.1,
                                    ) as f32
                                        * class_weight,
                                );
                            }
                        }
                    }
                    st.pairs_evaluated += local_eval as u64;
                    st.pairs_below_threshold += local_below as u64;
                }
                (local, coarse, st)
            },
        )
        .reduce(
            || {
                (
                    TileAccumulator::new(),
                    CoarseLevels::new(),
                    AirborneStats::default(),
                )
            },
            |(mut la, mut ca, mut sa), (lb, cb, sb)| {
                la.merge_from(&lb);
                ca.merge_from(&cb);
                sa.merge(&sb);
                (la, ca, sa)
            },
        );
    coarse.expand_into(&mut local);
    accum.merge_from(&local);

    st.rows_seen = airborne.len();
    st
}
