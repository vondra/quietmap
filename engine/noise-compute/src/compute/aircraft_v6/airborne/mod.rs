//! Direct row-view scatter for airborne popup arrows. Each
//! `AirborneRowView` carries one row's sub-segment columns; this kernel
//! iterates the sub-segments, runs the standard Doc 29 SEL chain
//! (`SegmentTerrain` cache + Filter D inline + `segment_sel_with_terrain`,
//! then an Lmax LUT lookup) and updates per-real-flight `FlightAccum`s.
//! No `AircraftSegment` `Vec` is allocated — segments are built on the
//! stack per iteration.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

use grid::geo::wrapped_longitude_delta;

use crate::compute::aircraft_v6::state::{BandStats, FlightAccum, TopFlightCandidate};
use crate::compute::aircraft_v6::views::AirborneRowView;
use crate::emission::aircraft;
use crate::propagation::iso9613::fast_exp_f64;
use crate::types::{
    AircraftAirborneDetail, AircraftEventBandStats, AircraftSegment, AircraftTopFlight,
    ImpactDeltas, NoisePeriods, Receiver, SegmentTrace, TraceCollector,
};

/// Maximum number of `top_flights` rows the popup returns. Frontend
/// renders a sortable table; 20 is the empirical break-even between
/// "user can see all the loud flights" and "table fits without paging".
const TOP_FLIGHTS_N: usize = 20;

/// Lmax (dB) below which sub-segment traces are not built. Saves
/// ~95 % of `SegmentTrace` allocations at LKPR.
///
/// Safety margin (worst case, generous): a single transit yields
/// SEL ≈ Lmax + 15 dB for the loudest profile classes; over a 24 h
/// daily window the contribution is SEL − 10·log10(86400) + 10 dB
/// night penalty = (Lmax+15) − 49.4 + 10 ≈ Lmax − 24.4 dB. With
/// Lmax = 25 dB that puts a single-event Lden contribution near
/// 0.6 dB — already well below the popup segment tab's empirical
/// rank floor of ~40 dB Lden (LKPR top-150). Below 25 dB Lmax a
/// sub-segment cannot survive the user-visible cap regardless of
/// the SEL/Lmax delta assumed. Energy accumulation (period_energy,
/// peak_lmax updates) runs regardless — only the SegmentTrace
/// allocation + push are gated, so Lden parity is held.
const AIRBORNE_TRACE_CUTOFF_DB: f64 = 25.0;

/// Monotone-with-`received_lden.full` rank key used by the bounded
/// top-K heap. Avoids per-sub-seg `compute_lden` calls (3× exp + 1× log10
/// ≈ 120 ns) — `received_lden.full` is
///   `10·log10(W[period] · energy / (n_days · 86400)) + … silent-period floors`
/// and `10·log10(·)` plus the common `/(n_days · 86400)` are monotone.
/// So we rank by `energy * AIRBORNE_RANK_W[period]` (3 ops: index + mul).
/// `W[period]` is the standard Doc 9613 Lden time-weight per period.
const AIRBORNE_RANK_W: [f64; 3] = [
    // day:     12 h × 1.0     ⇒ 12 / 43200 s
    12.0 / 43200.0,
    // evening: 4 h × 10^0.5   ⇒ 4·√10 / 14400 s
    4.0 * 3.162277660168379_f64 / 14400.0,
    // night:   8 h × 10^1.0   ⇒ 80 / 28800 s
    80.0 / 28800.0,
];

/// One entry of the bounded top-K airborne trace heap. Ranks by
/// `rank_key` (linear, monotone with `received_lden.full`) using
/// `f64::total_cmp` so heap pop/peek give a total order even with
/// NaN edge cases.
struct ScoredTrace {
    rank_key: f64,
    trace: SegmentTrace,
}

impl PartialEq for ScoredTrace {
    fn eq(&self, other: &Self) -> bool {
        self.rank_key.total_cmp(&other.rank_key).is_eq()
    }
}
impl Eq for ScoredTrace {}
impl PartialOrd for ScoredTrace {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScoredTrace {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank_key.total_cmp(&other.rank_key)
    }
}

/// Scatter the airborne row set onto a per-real-flight accumulator
/// table. Caller passes the resulting flights map to `build_detail` for
/// Doc 29 normalization (`n_days × period_seconds`).
///
/// The receiver terrain horizon is mandatory. The optional building horizon
/// represents an empty local roof skyline, not an alternate propagation path.
///
/// Per-sub-segment `SegmentTrace`s land in `traces.segments` so the
/// Noise Segments popup tab can render one row per Doc 29 SEL call
/// instead of one row per flight (the popup's actual compute unit).
pub fn scatter(
    receiver: &Receiver,
    rows: &[AirborneRowView<'_>],
    n_days_f: f64,
    // GA hybrid per-class weight LUT.
    // Each row's energy AND its count (`flight_weight`) are multiplied by
    // `class_weights.get(class)` so a GA one-off divides by `ga_n_days`, not
    // `n_days`. Uniform (all-1.0) for non-hybrid extracts.
    class_weights: &aircraft::ClassWeights,
    horizon: &aircraft::ReceiverHorizon,
    buildings: Option<&aircraft::BuildingHorizon>,
    trace_cap: usize,
    mut traces: Option<&mut TraceCollector>,
) -> HashMap<u64, FlightAccum> {
    let rx_elev = receiver.altitude_m();
    let npd_luts = aircraft::NpdLuts::shared();
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    // Bounded top-K min-heap (size `trace_cap`). We rank by `rank_key`
    // (monotone with received_lden.full) and use `Reverse` so the heap
    // root is the *weakest* kept trace — pop+push replaces it when a
    // stronger candidate arrives. Avoids ~4.4 M `SegmentTrace`
    // allocations at LKPR (only ~150 + a few hundred replacements
    // actually allocate; the rest skip the trace builder entirely).
    let mut heap: BinaryHeap<Reverse<ScoredTrace>> = if traces.is_some() && trace_cap > 0 {
        BinaryHeap::with_capacity(trace_cap)
    } else {
        BinaryHeap::new()
    };

    // Bbox prefilter at the conservative max-class reach (16 km) —
    // drops rows whose stored bbox is fully outside the receiver
    // envelope. Per v15 the row's sub-segments already carry pre-sampled
    // terrain, so the savings from the prune are now segment-construction
    // + NPD-lookup cost rather than raster I/O. Must stay at the global
    // 16 km cap, not per-row noise-class reach: the kernel
    // (`doc29.rs:359-366`) uses the UNCLAMPED foot of perpendicular for
    // its slant test, so a row whose endpoints lie just outside a
    // smaller class envelope can still have an unclamped foot inside
    // class reach and survive ΔF. Tightening the bbox here would
    // false-reject those rows.
    // The per-class gate fires at the per-direction line-distance check
    // below and inside `segment_sel_with_cuts`. At airport density a
    // popup touches ~21 k rows (3 k/R4 × 7 R4); the prune drops 60-80 %
    // of those rows before per-sub-seg work.
    let radius_lat_deg = aircraft::meters_to_lat_deg(aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M);
    let radius_lon_deg =
        aircraft::meters_to_lon_deg(receiver.lat, aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M);
    let env_min_lat = (receiver.lat - radius_lat_deg) as f32;
    let env_max_lat = (receiver.lat + radius_lat_deg) as f32;
    let env_min_lon_raw = receiver.lon - radius_lon_deg;
    let env_max_lon_raw = receiver.lon + radius_lon_deg;
    // Antimeridian-safe envelope: if the receiver's reach radius would
    // wrap past ±180°, fall back to a no-op longitude prune. Stored
    // bboxes are in [-180, 180]; the simple comparison `bb.max_lon <
    // env_min_lon` would otherwise drop sources on the other side of
    // the dateline. Latitude is bounded ±90 so it never
    // wraps; only longitude gets the guard.
    let lon_prune_active = env_min_lon_raw >= -180.0 && env_max_lon_raw <= 180.0;
    let env_min_lon = env_min_lon_raw as f32;
    let env_max_lon = env_max_lon_raw as f32;

    // Sub-seg line-distance projection constants — receiver-relative
    // meters factors are computed ONCE outside the loops.
    // `m_per_deg_lon` expects radians; degrees would make east/west metres
    // ~1.5× too large at 50 °N and mis-project the line-distance check.
    // Also mirror the kernel's lat factor (constants module's
    // `M_PER_DEG_LAT` differs from `aircraft::M_PER_DEG_LAT = 111132.92`.
    // Match the kernel's value so
    // the prefilter geometry is bit-identical to `segment_sel_with_overrides`).
    let cos_lat = receiver.lat.to_radians().cos().max(0.2);
    let rx_m_per_lon = aircraft::M_PER_DEG_LAT * cos_lat;
    let rx_m_per_lat = aircraft::M_PER_DEG_LAT;

    for row in rows {
        let bb = &row.bbox;
        if bb.max_lat < env_min_lat || bb.min_lat > env_max_lat {
            continue;
        }
        if lon_prune_active && (bb.max_lon < env_min_lon || bb.min_lon > env_max_lon) {
            continue;
        }
        // Per-class reach for the per-direction line-distance gate
        // below. The kernel uses the same `REACH_SQ_TABLE[class_idx][is_dep]`
        // internally (`segment_sel.rs:196`), so rejecting earlier on
        // `cross² > reach² × len²` (unclamped line-distance squared,
        // matches `doc29.rs:359` exactly) cannot false-reject anything
        // the kernel would accept. Light classes (≈ 8 km reach) gain
        // most.
        let class_idx = aircraft::noise_class_of(row.profile_idx) as usize;
        let reach_sq_class = aircraft::REACH_SQ_TABLE[class_idx];
        // GA hybrid weight for this row's class: one f64 per row, applied to
        // every sub-segment's energy and the flight's
        // count. A whole airborne row is one flight = one class, so the
        // weight is row-constant.
        let class_weight = class_weights.get(class_idx as u8);
        let sub = row.sub_segments;
        let n = sub.len();
        for i in 0..n {
            // Per-sub-seg prefilter, two layers:
            // (a) cheap lat/lon endpoint bbox in the 16 km envelope —
            //     rejects ~90 % of sub-segs whose endpoints sit fully
            //     outside (~6 ns).
            // (b) line-distance check on the unclamped extension at
            //     the per-class per-direction reach — handles the
            //     borderline case where both endpoints lie just outside
            //     the bbox but the track points through the receiver.
            //     Mirrors the kernel's CPA geometry (`doc29.rs:359`) so
            //     a sub-seg the kernel would accept never gets pruned
            //     here (~10 ns).
            //
            // The full kernel (`segment_sel_with_cuts`) is ~60 ns/sub-
            // seg, so this two-layer prefilter is a clear net win even
            // when both checks must run.
            let s_lat_f = sub.start_lat[i];
            let e_lat_f = sub.end_lat[i];
            let sub_max_lat = s_lat_f.max(e_lat_f);
            let sub_min_lat = s_lat_f.min(e_lat_f);
            if sub_max_lat < env_min_lat || sub_min_lat > env_max_lat {
                continue;
            }
            let s_lon_f = sub.start_lon[i];
            let e_lon_f = sub.end_lon[i];
            if lon_prune_active {
                let sub_max_lon = s_lon_f.max(e_lon_f);
                let sub_min_lon = s_lon_f.min(e_lon_f);
                if sub_max_lon < env_min_lon || sub_min_lon > env_max_lon {
                    continue;
                }
            }
            // Layer (b): line-distance to receiver, in receiver-local
            // meters. Cross product squared vs `reach² × seg_len²`
            // avoids a sqrt and a divide. For degenerate sub-segs
            // (seg_len ≈ 0) the bbox check above already covers the
            // endpoint-only case, so we skip the line test there.
            let ax = wrapped_longitude_delta(receiver.lon, s_lon_f as f64) * rx_m_per_lon;
            let ay = (s_lat_f as f64 - receiver.lat) * rx_m_per_lat;
            let by = (e_lat_f as f64 - receiver.lat) * rx_m_per_lat;
            let sdx = wrapped_longitude_delta(s_lon_f as f64, e_lon_f as f64) * rx_m_per_lon;
            let sdy = by - ay;
            let seg_len_sq = sdx * sdx + sdy * sdy;
            let flags = sub.flags[i];
            let is_departure = flags & 0b001 != 0;
            if seg_len_sq > 1.0 {
                let cross = ax * sdy - ay * sdx;
                let cross_sq = cross * cross;
                let reach_sq_dir = reach_sq_class[is_departure as usize];
                if cross_sq > reach_sq_dir * seg_len_sq {
                    continue;
                }
            }
            // Stack-only segment — `segment_sel_with_terrain` and the
            // validity gates only need `&AircraftSegment`, not Vec storage.
            //
            // `ground_context = NONE` for every airborne sub-segment. The
            // popup-side `is_near_airport` carve-out (run every popup query
            // over ~561 airport centroids at LKPR — 944 ns × 22 M sub-segs
            // = 21 s) was a defensive guard against Stage 1 misclassifying
            // 0-15 m AGL near-airport approaches as airborne. Empirically
            // (5-receiver SKIP_STALE comparison, 2026-05-23) the carve-out
            // NEVER fired — Stage 1's `ground_inference.rs` (32-point edge
            // window + surface signature) already correctly routes those
            // points to ground.arrow. Aircraft Lden delta when the stale
            // filter is bypassed entirely: 0.000 dB on all five reference
            // receivers (LKPR / Praha / Brdy / Šumava / 10 km W Praha).
            let seg = AircraftSegment {
                flight_id: row.flight_id,
                profile_idx: row.profile_idx,
                is_departure,
                on_ground: false,
                period: sub.period[i],
                date_id: sub.date_id[i],
                start_lat: sub.start_lat[i] as f64,
                start_lon: sub.start_lon[i] as f64,
                start_alt_m: sub.start_alt_m[i],
                end_lat: sub.end_lat[i] as f64,
                end_lon: sub.end_lon[i] as f64,
                end_alt_m: sub.end_alt_m[i],
                speed_kt: sub.speed_kt[i],
                segment_length_m: sub.length_m[i],
                count_weight: 1.0,
                surface_model: false,
                ground_context: aircraft::GROUND_CONTEXT_NONE,
                ground_ops_kind: aircraft::GROUND_OPS_KIND_NONE,
                source_id: row.source_id as u16,
            };
            // Only start/end terrain elevations are stored. The popup does
            // not run the intermediate chord validity gate; the endpoint
            // ground-stale gate still runs (start/end AGL ≤ 15 m), and
            // Filter D's extrapolation cuts are receiver-dependent — they
            // stay here, fed from the two stored endpoint elevs.
            let start_elev = sub.terrain_start_elev_m[i] as f64;
            let end_elev = sub.terrain_end_elev_m[i] as f64;
            let terrain = aircraft::SegmentTerrain {
                start_elev,
                // q1/mid/q3 are unused by the endpoint-only stale-ground gate;
                // cruise still populates all five samples for its validity gate.
                q1_elev: 0.0,
                mid_elev: 0.0,
                q3_elev: 0.0,
                end_elev,
            };
            if aircraft::is_ground_stale_with_terrain(&seg, &terrain) {
                continue;
            }
            let Some(kernel) = aircraft::segment_kernel_with_cuts(
                &seg,
                receiver.lat,
                receiver.lon,
                rx_elev,
                start_elev - 30.0,
                end_elev - 30.0,
                npd_luts,
                horizon,
                buildings,
            ) else {
                continue;
            };
            // GA hybrid weight folded into the per-sub-seg energy here so
            // EVERY downstream consumer (period totals, band stats, trace
            // rank/energies) sees the `1/ga_n_days`-scaled value with no further
            // per-site multiply. `flight_weight = class_weight` carries the
            // SAME factor through the count machinery (helicopter_flights_
            // per_day, observed_flights_per_day): weight/n_days = 1/ga_n_days
            // per one-off.
            let period = (seg.period.min(2)) as usize;
            let energy_for_sel =
                |sel: f64| fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) * class_weight;
            let energy = energy_for_sel(kernel.sel);
            let acc = flights.entry(row.flight_id).or_insert_with(|| {
                FlightAccum::new(
                    row.profile_idx,
                    class_weight,
                    false,
                    row.aircraft_type,
                    row.callsign.to_string(),
                )
            });
            acc.free_period_energy[period] += energy_for_sel(kernel.free_sel);
            acc.no_terrain_period_energy[period] += energy_for_sel(kernel.sel_no_terrain);
            acc.no_screening_period_energy[period] += energy_for_sel(kernel.sel_no_screening);
            // Keep the received path's historical 20 dB event floor. The
            // retained variants above deliberately run before this check so
            // a strong aircraft hidden by a terrain/building edge still
            // contributes to `periods_free` and its effect deltas.
            if kernel.sel < 20.0 {
                continue;
            }
            acc.period_energy[period] += energy;

            let cpa = aircraft::CpaResult {
                q_m: kernel.q_m,
                d_p_m: kernel.d_p_m,
                lateral_m: kernel.lateral_m,
                relative_alt_m: kernel.rel_alt_m,
                beta_deg: kernel.beta_deg,
                seg_len_m: kernel.seg_len_m,
                t: kernel.t,
            };
            let sel = kernel.sel;

            let class_idx = aircraft::noise_class_of(seg.profile_idx) as usize;
            // Display metrics use the CLAMPED CPA — the closest the aircraft
            // actually reaches while on this segment — so a curving track's
            // extrapolated infinite-line foot can't report a phantom near pass
            // (see `clamped_display_cpa`). `sel`/`energy` above keep the
            // unclamped `cpa`; ΔF needs the infinite-line `q_m`.
            let sdz = seg.end_alt_m as f64 - seg.start_alt_m as f64;
            let (disp_dist, disp_alt) = aircraft::clamped_display_cpa(&cpa, sdz);
            // log2 × LOG10_2 ≡ log10 at f64; matches the kernel's NPD-lookup
            // distance idiom (doc29.rs:376), same identity as d0317794.
            let log_d =
                (disp_dist * aircraft::FT_PER_M).max(100.0).log2() * std::f64::consts::LOG10_2;
            let lmax = npd_luts.lookup_lmax(class_idx, seg.is_departure, log_d);
            if lmax > acc.peak_lmax {
                acc.peak_lmax = lmax;
                acc.peak_sel = sel;
                acc.peak_altitude_m = disp_alt;
                acc.peak_period = seg.period;
                acc.peak_date_id = seg.date_id;
                acc.peak_seg_start = [seg.start_lon, seg.start_lat];
                acc.peak_seg_end = [seg.end_lon, seg.end_lat];
            }
            if disp_dist < acc.min_dist_m {
                acc.min_dist_m = disp_dist;
            }

            if lmax >= AIRBORNE_TRACE_CUTOFF_DB {
                if let Some(t) = traces.as_deref_mut() {
                    // Maintain the "N visible" denominator regardless of
                    // whether this sub-seg's trace survives the heap.
                    t.airborne_above_cutoff = t.airborne_above_cutoff.saturating_add(1);

                    if trace_cap > 0 {
                        let rank_key = energy * AIRBORNE_RANK_W[period];
                        // Skip the trace builder unless this sub-seg can
                        // displace the weakest kept trace.
                        let should_build = heap.len() < trace_cap
                            || heap.peek().map(|w| rank_key > w.0.rank_key).unwrap_or(true);
                        if should_build {
                            let mut period_energies = [0.0f64; 3];
                            period_energies[period] = energy;
                            let mut free_period_energies = [0.0f64; 3];
                            free_period_energies[period] = energy_for_sel(kernel.free_sel);
                            let mut no_terrain_period_energies = [0.0f64; 3];
                            no_terrain_period_energies[period] =
                                energy_for_sel(kernel.sel_no_terrain);
                            let mut no_screening_period_energies = [0.0f64; 3];
                            no_screening_period_energies[period] =
                                energy_for_sel(kernel.sel_no_screening);
                            let (screening_kind, screening_db) =
                                if kernel.terrain_dz <= 0.0 && kernel.building_dz <= 0.0 {
                                    ("none", 0.0)
                                } else if kernel.terrain_dz >= kernel.building_dz {
                                    ("terrain", kernel.terrain_dz)
                                } else {
                                    ("building", kernel.building_dz)
                                };
                            let installation = match kernel.installation {
                                aircraft::Installation::Wing => "wing",
                                aircraft::Installation::Fuselage => "fuselage",
                                aircraft::Installation::Propeller => "propeller",
                            };
                            let doc29 = crate::types::Doc29Breakdown {
                                sel_npd_db: kernel.sel_npd_db,
                                delta_v_db: kernel.delta_v_db,
                                delta_i_db: kernel.delta_i_db,
                                lambda_db: kernel.lambda_db,
                                delta_f_db: kernel.delta_f_db,
                                d_p_m: cpa.d_p_m,
                                lateral_m: cpa.lateral_m,
                                beta_deg: cpa.beta_deg,
                                seg_len_m: seg.segment_length_m as f64,
                                d_bar_m: kernel.d_bar_m,
                                installation,
                                cffk_fast_path: kernel.cffk_fast_path,
                                screening_kind,
                                screening_db,
                            };
                            let trace = crate::traces::build_aircraft_airborne_subsegment_trace(
                                crate::traces::BuildAircraftAirborneSubSegmentTrace {
                                    callsign: row.callsign,
                                    aircraft_type: &row.aircraft_type,
                                    class_name: aircraft::CLASS_NAMES[class_idx],
                                    flight_id: row.flight_id,
                                    start_lat: seg.start_lat,
                                    start_lon: seg.start_lon,
                                    end_lat: seg.end_lat,
                                    end_lon: seg.end_lon,
                                    cpa_distance_m: disp_dist,
                                    altitude_m_at_cpa: disp_alt,
                                    d_slant_m: disp_dist.max(1.0),
                                    is_departure: seg.is_departure,
                                    period_energies,
                                    free_period_energies,
                                    no_terrain_period_energies,
                                    no_screening_period_energies,
                                    n_days: n_days_f,
                                    doc29,
                                },
                            );
                            let scored = ScoredTrace { rank_key, trace };
                            if heap.len() < trace_cap {
                                heap.push(Reverse(scored));
                            } else {
                                heap.pop();
                                heap.push(Reverse(scored));
                            }
                        }
                    }
                }
            }
        }
    }
    // Drain bounded top-K heap into `t.segments`. No sort needed:
    // `apply_segment_top_k_with_cap` (source-reader) re-sorts the entire
    // Vec by `received_lden.full` desc — it has to, because road / rail /
    // cruise / ground traces are mixed in. An in-scatter sort would be
    // redundant work.
    if let Some(t) = traces {
        t.segments
            .extend(heap.into_iter().map(|Reverse(st)| st.trace));
    }
    flights
}

/// Build airborne-side `AircraftAirborneDetail` and the airborne-only
/// periods (Doc 29 normalized). Walks airborne flights for per-band
/// stats; folds cruise period_energy into the periods total via the
/// separate `cruise_flights` table, whose synth-fid namespace is disjoint.
#[allow(clippy::too_many_arguments)]
pub fn build_detail(
    flights: &HashMap<u64, FlightAccum>,
    cruise_flights: &HashMap<u64, FlightAccum>,
    cruise_transit_count: usize,
    top_flight_candidates: &HashMap<u64, TopFlightCandidate>,
    cruise_band_stats: &[BandStats; 3],
    n_days_f: f64,
    // GA-class window for the popup's per-class "Data" row;
    // equals `n_days_f` for non-hybrid extracts.
    ga_n_days_f: f64,
) -> (
    NoisePeriods,
    NoisePeriods,
    ImpactDeltas,
    AircraftAirborneDetail,
) {
    use crate::periods;

    let mut airborne_energy = [0.0f64; 3];
    let mut free_airborne_energy = [0.0f64; 3];
    let mut no_terrain_airborne_energy = [0.0f64; 3];
    let mut no_screening_airborne_energy = [0.0f64; 3];
    let mut band_faint = BandStats::new();
    let mut band_audible = BandStats::new();
    let mut band_disruptive = BandStats::new();
    let mut helicopter_count = 0.0f64;
    let mut global_peak_lmax = f64::NEG_INFINITY;

    // Cruise period_energy folds into the airborne total — the popup
    // exposes a single Aircraft Lden, not separate cruise / airborne
    // numbers. Cruise band counters come via `cruise_band_stats`
    // (real-fid dedup) so we don't iterate cruise here for those.
    // Ascending flight_id, not HashMap order: f64 addition is not
    // associative, so the iteration order is part of the number. See
    // `crate::compute::key_sorted` for why sorting beats a fixed hasher here.
    for (_, acc) in crate::compute::key_sorted(cruise_flights) {
        // Period accumulation (day/evening/night) — `p` indexes both sides; the
        // f64 sum order across flights is part of popup byte parity.
        #[allow(clippy::needless_range_loop)]
        for p in 0..3 {
            airborne_energy[p] += acc.period_energy[p];
            free_airborne_energy[p] += acc.free_period_energy[p];
            no_terrain_airborne_energy[p] += acc.no_terrain_period_energy[p];
            no_screening_airborne_energy[p] += acc.no_screening_period_energy[p];
        }
        if acc.peak_lmax > global_peak_lmax {
            global_peak_lmax = acc.peak_lmax;
        }
    }

    // Sampling-fragility accumulators (see AircraftAirborneDetail docs):
    // real airborne flights only — cruise buckets are aggregate-stable by
    // construction and synthetic fids carry no date.
    //
    // BTreeMap, not HashMap: the `max_by` below picks the loudest day and
    // `max_by` returns the LAST maximum, so a HashMap's iteration order
    // would decide ties. At most one entry per sampled day (≤ 365).
    let mut energy_by_day: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    let mut max_flight_energy = 0.0f64;

    // Sorted once and reused by `build_top_flights` below — the top-20
    // selection needs the same deterministic order and would otherwise
    // sort the same map a second time.
    let flights_by_id = crate::compute::key_sorted(flights);

    for &(&flight_id, acc) in flights_by_id.iter() {
        // Period accumulation — same byte-parity sum order as the cruise loop above.
        #[allow(clippy::needless_range_loop)]
        for p in 0..3 {
            airborne_energy[p] += acc.period_energy[p];
            free_airborne_energy[p] += acc.free_period_energy[p];
            no_terrain_airborne_energy[p] += acc.no_terrain_period_energy[p];
            no_screening_airborne_energy[p] += acc.no_screening_period_energy[p];
        }
        let flight_energy: f64 = acc.period_energy.iter().sum();
        if flight_energy <= 0.0 {
            continue;
        }
        if acc.peak_lmax > global_peak_lmax {
            global_peak_lmax = acc.peak_lmax;
        }
        if acc.is_cruise {
            continue;
        }
        if let crate::flight_id::FlightIdKind::Real { start_unix, .. } =
            crate::flight_id::unpack(flight_id)
        {
            *energy_by_day.entry(start_unix / 86_400).or_default() += flight_energy;
            max_flight_energy = max_flight_energy.max(flight_energy);
        }
        if aircraft::is_helicopter_profile(acc.profile_idx) {
            helicopter_count += acc.flight_weight / n_days_f;
        }
        let cls = aircraft::noise_class_of(acc.profile_idx) as usize;
        let weight = acc.flight_weight.round().max(1.0) as u32;
        // Band stats want average altitude per event, not CPA distance.
        // Feeding `min_dist_m` into `alt_sum` would report CPA values
        // labelled as altitude. Use the peak-encounter altitude
        // weighted by flight_weight to match cruise.rs band stats.
        let alt_w_sum = acc.peak_altitude_m * acc.flight_weight;
        if acc.peak_lmax > 30.0 {
            band_faint.add_event(acc.flight_weight, alt_w_sum, cls, weight);
            if acc.peak_lmax > 45.0 {
                band_audible.add_event(acc.flight_weight, alt_w_sum, cls, weight);
                if acc.peak_lmax > 60.0 {
                    band_disruptive.add_event(acc.flight_weight, alt_w_sum, cls, weight);
                }
            }
        }
    }

    // Cruise band counters routed via the dedicated cruise dedup table —
    // `cruise.rs::scatter` populated `cruise_band_stats` per band so a
    // single transit crossing many grid cells counts once per band.
    for (band, cruise) in [&mut band_faint, &mut band_audible, &mut band_disruptive]
        .into_iter()
        .zip(cruise_band_stats.iter())
    {
        band.count += cruise.count;
        band.alt_sum += cruise.alt_sum;
        for k in 0..aircraft::NUM_CLASSES {
            band.class_counts[k] += cruise.class_counts[k];
        }
    }

    let periods_from_energy = |energy: [f64; 3]| {
        if energy.iter().sum::<f64>() > 0.0 {
            let ld = aircraft::period_leq(energy[0], n_days_f, aircraft::PERIOD_SECONDS[0]);
            let le = aircraft::period_leq(energy[1], n_days_f, aircraft::PERIOD_SECONDS[1]);
            let ln = aircraft::period_leq(energy[2], n_days_f, aircraft::PERIOD_SECONDS[2]);
            periods::periods(ld, le, ln)
        } else {
            NoisePeriods::silence()
        }
    };
    let airborne_periods = periods_from_energy(airborne_energy);
    let airborne_periods_free = periods_from_energy(free_airborne_energy);
    let periods_no_terrain = periods_from_energy(no_terrain_airborne_energy);
    let periods_no_screening = periods_from_energy(no_screening_airborne_energy);
    let impact = |variant: &NoisePeriods| {
        if airborne_periods.lden_db.is_finite() && variant.lden_db.is_finite() {
            (airborne_periods.lden_db - variant.lden_db).min(0.0)
        } else {
            0.0
        }
    };
    let impacts = ImpactDeltas {
        terrain: impact(&periods_no_terrain),
        screening: impact(&periods_no_screening),
        ..Default::default()
    };

    let observed_flights_per_day: f64 = flights_by_id
        .iter()
        .map(|&(_, f)| f)
        .filter(|f| !f.is_cruise && f.period_energy.iter().sum::<f64>() > 0.0)
        .map(|f| f.flight_weight / n_days_f)
        .sum();
    // Cruise transits seen at this receiver, real-fid deduped (count
    // passed in by `compute_aircraft_v6` from `cruise_flight_stats`).
    // Distinct from `observed_flights_per_day` (airborne only). Acts as
    // a context counter for the Lmax band rows: cruise transits whose
    // `peak_lmax` crosses 30/45/60 dB inflate band counts above the
    // airborne flight count, and naming the cruise total separately
    // makes that delta legible. (Below-threshold cruise transits are
    // included here but don't enter the bands — the row is therefore
    // an upper bound on the cruise contribution, not an exact remainder.)
    let cruise_transits_per_day = cruise_transit_count as f64 / n_days_f;

    let total_airborne_energy: f64 = airborne_energy.iter().sum();
    let top_flights =
        build_top_flights(&flights_by_id, top_flight_candidates, total_airborne_energy);

    let (top_day_energy_share, top_day_date) = energy_by_day
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|_| total_airborne_energy > 0.0)
        .map(|(&day, &e)| (e / total_airborne_energy, date_from_unix(day * 86_400)))
        .unwrap_or((0.0, String::new()));
    let top_flight_energy_share = if total_airborne_energy > 0.0 {
        max_flight_energy / total_airborne_energy
    } else {
        0.0
    };

    let to_band = |band: &BandStats| AircraftEventBandStats {
        observed_events_per_day: band.count / n_days_f,
        avg_altitude_m: if band.count > 0.0 {
            band.alt_sum / band.count
        } else {
            0.0
        },
        top_aircraft: band.top_type().to_string(),
    };
    let detail = AircraftAirborneDetail {
        periods: airborne_periods.clone(),
        observed_flights_per_day,
        helicopter_flights_per_day: helicopter_count,
        cruise_transits_per_day,
        lmax_peak: if global_peak_lmax > -900.0 {
            Some(global_peak_lmax)
        } else {
            None
        },
        faint: to_band(&band_faint),
        audible: to_band(&band_audible),
        disruptive: to_band(&band_disruptive),
        top_day_energy_share: round3(top_day_energy_share),
        top_day_date,
        top_flight_energy_share: round3(top_flight_energy_share),
        sample_days: n_days_f as u32,
        ga_sample_days: ga_n_days_f as u32,
        top_flights,
    };

    (airborne_periods, airborne_periods_free, impacts, detail)
}

/// Top-N flights by `peak_lmax` interleaving airborne (`flights`,
/// real fid) and cruise (`cruise_candidates`, real fid). Bounded
/// insertion sort so a busy airport doesn't pay the full `O(n log n)`.
/// Cruise rows show `energy_pct = 0` because the bucket aggregates many
/// real fids and per-fid energy split would be artificial.
fn build_top_flights(
    flights_by_id: &[(&u64, &FlightAccum)],
    cruise_candidates: &HashMap<u64, TopFlightCandidate>,
    total_airborne_energy: f64,
) -> Vec<AircraftTopFlight> {
    use std::cmp::Ordering;

    // Rank on plain scalars first, build the rows afterwards. The previous
    // bounded insertion sort built a full `AircraftTopFlight` — five String
    // allocations (callsign, typecode, profile name, date, ICAO hex) — for
    // EVERY flight and then dropped all but 20. São Paulo carries ~95 k
    // airborne flights, so ~475 k allocations were made to be discarded.
    //
    // `is_cruise` (0 = airborne, 1 = cruise) is a SELECTION tiebreak, not a
    // display field: the insertion sort fed airborne candidates first and
    // kept the first arrival at an equal `peak_lmax`, so airborne won a tie
    // for the last slot. (lmax desc, is_cruise asc, fid asc) reproduces
    // that, and since a fid appears in at most one of the two groups it is
    // a TOTAL order — which is also why the cruise map below can be walked
    // in hash order without costing reproducibility.
    let mut cands: Vec<(f64, u8, u64)> =
        Vec::with_capacity(flights_by_id.len() + cruise_candidates.len());
    for &(&fid, acc) in flights_by_id.iter() {
        if acc.is_cruise {
            continue;
        }
        let flight_energy: f64 = acc.period_energy.iter().sum();
        if flight_energy <= 0.0 || acc.peak_lmax <= -900.0 {
            continue;
        }
        cands.push((acc.peak_lmax, 0, fid));
    }
    for (&fid, cand) in cruise_candidates.iter() {
        if cand.peak_lmax <= -900.0 {
            continue;
        }
        // Same real fid in both maps means the flight had both an
        // airborne sub-segment encounter and a cruise bucket encounter
        // in receiver radius — keep the airborne entry (sub-segment-level
        // granularity, real `energy_pct`) and skip the cruise dup.
        if flights_by_id
            .binary_search_by_key(&&fid, |&(k, _)| k)
            .is_ok()
        {
            continue;
        }
        cands.push((cand.peak_lmax, 1, fid));
    }

    if cands.len() > TOP_FLIGHTS_N {
        cands.select_nth_unstable_by(TOP_FLIGHTS_N, |a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        cands.truncate(TOP_FLIGHTS_N);
    }
    // Stable total order for DISPLAY: descending peak_lmax, ascending fid
    // as final tiebreak so equal-Lmax + equal-callsign rows don't fall back
    // to map iteration order. Provenance deliberately does not enter here —
    // it only decides which rows survive the cut above.
    cands.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });

    cands
        .into_iter()
        .filter_map(|(_, is_cruise, fid)| {
            if is_cruise == 1 {
                return cruise_candidates
                    .get(&fid)
                    .map(|cand| cruise_top_flight_entry(fid, cand));
            }
            let idx = flights_by_id
                .binary_search_by_key(&&fid, |&(k, _)| k)
                .ok()?;
            let acc = flights_by_id[idx].1;
            let flight_energy: f64 = acc.period_energy.iter().sum();
            let energy_pct = if total_airborne_energy > 0.0 {
                flight_energy / total_airborne_energy * 100.0
            } else {
                0.0
            };
            Some(airborne_top_flight_entry(fid, acc, energy_pct))
        })
        .collect()
}

fn airborne_top_flight_entry(fid: u64, acc: &FlightAccum, energy_pct: f64) -> AircraftTopFlight {
    let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(fid);
    let synthetic = start_unix.is_none();
    AircraftTopFlight {
        lmax_db: round1(acc.peak_lmax),
        cpa_distance_m: round1(acc.min_dist_m),
        altitude_m: round1(acc.peak_altitude_m),
        period: acc.peak_period,
        date: date_from_id(acc.peak_date_id),
        profile: aircraft::PROFILES[aircraft::clamp_profile_idx(acc.profile_idx)]
            .name
            .to_string(),
        aircraft_type: aircraft::typecode_to_string(&acc.aircraft_type),
        callsign: acc.callsign.clone(),
        energy_pct: round1(energy_pct),
        geometry: [
            [acc.peak_seg_start[0], acc.peak_seg_start[1]],
            [acc.peak_seg_end[0], acc.peak_seg_end[1]],
        ],
        icao_hex,
        start_unix,
        synthetic,
    }
}

fn cruise_top_flight_entry(fid: u64, cand: &TopFlightCandidate) -> AircraftTopFlight {
    let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(fid);
    // `date` is the flight's start_unix-derived date (when ADS-B first
    // saw the flight, ~= takeoff); not the overflight encounter time.
    // Cruise scatter has no per-encounter timestamp — Stage 2B
    // aggregates by grid cell, dropping individual sample timing.
    let date = start_unix.map(date_from_unix).unwrap_or_default();
    AircraftTopFlight {
        lmax_db: round1(cand.peak_lmax),
        cpa_distance_m: round1(cand.min_dist_m),
        altitude_m: round1(cand.peak_altitude_m),
        period: cand.peak_period,
        date,
        profile: aircraft::PROFILES[aircraft::clamp_profile_idx(cand.profile_idx)]
            .name
            .to_string(),
        aircraft_type: aircraft::typecode_to_string(&cand.aircraft_type),
        callsign: cand.callsign.clone(),
        // Cruise bucket aggregates many real fids; per-fid energy share
        // would be artificial, so we report 0 instead of fabricating one.
        energy_pct: 0.0,
        geometry: [
            [cand.peak_seg_start[0], cand.peak_seg_start[1]],
            [cand.peak_seg_end[0], cand.peak_seg_end[1]],
        ],
        icao_hex,
        start_unix,
        synthetic: start_unix.is_none(),
    }
}

#[inline]
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[inline]
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

use super::dates::{date_from_id, date_from_unix};

#[cfg(test)]
mod tests;
