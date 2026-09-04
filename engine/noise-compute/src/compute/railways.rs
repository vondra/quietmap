//! Railway compute kernel — groups named rail segments by (ref, name, type) and unnamed
//! ways by nearby tracks, then emits per-period power and propagates to the receiver
//! (CNOSSOS rail). Shared by popup + heatmap.
use crate::*;

/// Link radius for unnamed ways of one rail type: the prepared-data checks measured
/// a 136.638665 m widest closest-point span across the six nearest Vinohrady tram
/// ways, while the nearest same-type unnamed railway pair kept apart at Prague
/// centre is 150.013705 m (ways 97825895 and 911542434). Both measurements use
/// `data/prepared/2026/z9/276/173/railways.arrow` (Vinohrady
/// `50.0755,14.4378` and Prague centre `50.0850,14.4234` both sit in z9
/// square 276/173; measured 2026-09-03).
/// The receiver-local bucket grid below keeps the exact distance predicate while
/// avoiding the old O(W²) scan; in the measured Prague-centre run it reduced
/// clustering from 58.208 ms to 8.125 ms.
const RAIL_TRACK_LINK_M: f64 = 150.0;

/// Memo key for `REACH_CACHE`: `(rail_type, admin ISO, city_id, continent, speed bits, pax bits, frt bits)`.
type ReachKey = (u8, [u8; 2], u16, u8, u64, u64, u64);

thread_local! {
    /// Exact-key memo for `rail_reach_m` — see the comment at the call site.
    /// Keyed on raw f64 bits (no quantization semantics to reason about) plus the
    /// admin code (C1's per-region split changes the solved reach). Per-thread keeps
    /// the popup single-threaded-per-request contract.
    static REACH_CACHE: std::cell::RefCell<std::collections::HashMap<ReachKey, f64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn is_unnamed_track(segment: &RailSegment) -> bool {
    segment.rail_ref.is_empty() && segment.name.is_empty()
}

#[derive(Clone, Copy)]
struct UnnamedWayClosestPoint {
    rail_type: u8,
    osm_id: i64,
    dist_m: f64,
    lat: f64,
    lon: f64,
}

fn unnamed_way_closest_points<'a>(
    segments: impl Iterator<Item = &'a RailSegment>,
) -> Vec<UnnamedWayClosestPoint> {
    let mut closest_by_way = std::collections::HashMap::new();
    for segment in segments.filter(|segment| is_unnamed_track(segment)) {
        let point = UnnamedWayClosestPoint {
            rail_type: segment.rail_type,
            osm_id: segment.osm_id,
            dist_m: segment.dist_m,
            lat: segment.cp_lat,
            lon: segment.cp_lon,
        };
        closest_by_way
            .entry((segment.rail_type, segment.osm_id))
            .and_modify(|current: &mut UnnamedWayClosestPoint| {
                if point.dist_m < current.dist_m {
                    *current = point;
                }
            })
            .or_insert(point);
    }

    let mut ways: Vec<_> = closest_by_way.into_values().collect();
    ways.sort_unstable_by_key(|way| (way.rail_type, way.osm_id));
    ways
}

#[inline]
fn union_find_root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

#[inline]
fn union_toward_smaller(parents: &mut [usize], left: usize, right: usize) {
    let left_root = union_find_root(parents, left);
    let right_root = union_find_root(parents, right);
    parents[left_root.max(right_root)] = left_root.min(right_root);
}

/// Assign every kept unnamed OSM way to a proximity track.
///
/// Each way contributes only the microsegment nearest the receiver. Sorting by
/// `(type, osm_id)` and always unioning toward the smaller index keep a component's
/// root at its smallest index, so the smallest OSM id is the component identity
/// whatever the edge order. Candidate pairs come from the same rail type's 3×3
/// neighbouring `RAIL_TRACK_LINK_M` grid cells; the linking predicate is
/// `geo::flat_dist` alone. The grid scales longitude at the highest |latitude| among
/// the ways, never more than `flat_dist` scales it at any pair's mid-latitude, so a
/// linkable pair is at most one cell apart on each axis at every latitude.
fn unnamed_track_cluster_ids<'a>(
    segments: impl Iterator<Item = &'a RailSegment>,
) -> std::collections::HashMap<(u8, i64), i64> {
    let ways = unnamed_way_closest_points(segments);
    let mut parents: Vec<_> = (0..ways.len()).collect();

    let widest_lat = ways.iter().map(|way| way.lat.abs()).fold(0.0, f64::max);
    let m_per_deg_lon = grid::geo::m_per_deg_lon(widest_lat.to_radians());
    let cell_of = |way: &UnnamedWayClosestPoint| {
        (
            way.rail_type,
            (way.lat * grid::geo::M_PER_DEG_LAT / RAIL_TRACK_LINK_M).floor() as i64,
            (way.lon * m_per_deg_lon / RAIL_TRACK_LINK_M).floor() as i64,
        )
    };
    let mut cells: std::collections::HashMap<(u8, i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (index, way) in ways.iter().enumerate() {
        cells.entry(cell_of(way)).or_default().push(index);
    }

    for (index, way) in ways.iter().enumerate() {
        let (rail_type, cell_y, cell_x) = cell_of(way);
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let Some(neighbours) = cells.get(&(rail_type, cell_y + dy, cell_x + dx)) else {
                    continue;
                };
                for &candidate in neighbours.iter().filter(|&&candidate| candidate > index) {
                    let other = &ways[candidate];
                    if geo::flat_dist(way.lat, way.lon, other.lat, other.lon) <= RAIL_TRACK_LINK_M {
                        union_toward_smaller(&mut parents, index, candidate);
                    }
                }
            }
        }
    }

    ways.iter()
        .enumerate()
        .map(|(index, way)| {
            let component = union_find_root(&mut parents, index);
            ((way.rail_type, way.osm_id), ways[component].osm_id)
        })
        .collect()
}

/// The total is intentionally folded in kept-segment order, not reconstructed
/// from contributor accumulators. Grouping can split or merge those accumulators;
/// this per-segment fold keeps the layer total bit-identical across key layouts.
fn add_segment_to_total(total_energy: &mut [f64; 3], variants: &[PropagationVariants; 3]) {
    for (total, variant) in total_energy.iter_mut().zip(variants) {
        *total += variant.full_energy;
    }
}

/// Compute railway noise — named tracks keep (ref, name, type); unnamed ways use
/// proximity-connected track clusters. Contributor geometry contains every segment.
///
/// Same three-pass parallel structure as `compute_roads` (see its docstring):
/// sequential gates + skyline growth chain, parallel per-segment evaluation
/// against frozen [`SkylineSnapshot`]s, sequential accumulation in segment
/// order. Bit-identical to the sequential loop by construction.
pub(crate) fn compute_railways(
    receiver: &Receiver,
    railways: &[RailSegment],
    obstacles: &crate::propagation::obstacle_index::ObstacleSet,
    rasters: &dyn RasterSampler,
    mut traces: Option<&mut TraceCollector>,
) -> (NoisePeriods, Vec<Contributor>) {
    use emission::railway::{self, RailType};
    use propagation::arc_screening::{ArcBounds, ArcScreeningScratch, ArcSkyline, SkylineSnapshot};
    use rayon::prelude::*;
    use std::collections::HashMap;

    let rcv_alt = receiver.altitude_m();
    let bounds = ArcBounds::shipped();
    // The set the arc rule clips against.
    let arc_set = obstacles;

    struct RailAccum {
        name: String,
        rail_type: RailType,
        rail_type_u8: u8,
        dominant_usage_u8: u8,
        dominant_osm_id: i64,
        min_dist: f64,
        min_d_slant: f64,
        min_ground_g: f64,
        cp_lat: f64,
        cp_lon: f64,
        src_height: f64,
        // Dominant-segment metadata — highest received-energy segment drives the
        // popup display, mirroring the road pattern. Earlier rail surfaced the
        // closest-segment fields, which misled whenever a busy/fast mainline
        // sat farther than a quiet siding (the siding's 30 km/h and 5 trains/day
        // looked like "the speed used" / "whole-line count" even though the
        // mainline produced ~all the energy). Closest-* fields stay in the
        // accumulator (kept harmless) but no longer feed `RailMetadata`.
        dominant_segment_idx: i16,
        dominant_distance_m: f64,
        dominant_trains_passenger_raw: f64,
        dominant_trains_freight_raw: f64,
        dominant_trains_passenger_effective: f64,
        dominant_trains_freight_effective: f64,
        dominant_trains_passenger_source: &'static str,
        dominant_trains_freight_source: &'static str,
        dominant_source_id: u16,
        dominant_maxspeed_posted: u16,
        dominant_speed_used: f64,
        dominant_speed_source: &'static str,
        dominant_service: bool,
        dominant_highspeed: bool,
        dominant_parallel_divisor: u8,
        // Aggregation
        segment_count: u32,
        total_length_m: f64,
        // Group-level screening obstacle histogram
        obstacle_segment_count: u32,
        obstacle_height_sum: f64,
        obstacle_max_height: f64,
        obstacle_max_segment_idx: i16,
        variants: [PropagationVariants; 3],
        emission_energy: f64,
        line_coords: Vec<[[f64; 2]; 2]>,
        dominant_bridge: bool,
        dominant_energy: f64,
        dominant_trace_idx: Option<usize>,
    }
    let mut rails_by_key: HashMap<(String, String, u8, Option<i64>), RailAccum> = HashMap::new();

    // Admin resolved once per call — the receiver position is constant across
    // segments. Drives the C1 per-region day/evening/night split (EU freight
    // runs ~55 % at night vs ~33 % world), shared with the heatmap loader + the
    // reach solver via `railway::rail_time_dist` (exact mirror of compute_roads).
    // M5: when source-reader installed the per-row channel (baked M3 columns),
    // each segment's OWN admin overrides this per segment below.
    let receiver_admin = crate::admin::admin_for_latlng(receiver.lat, receiver.lon);
    let reflection = rasters.building_enclosure(receiver.lat, receiver.lon);

    // ── Pass 1: admission gates + the skyline growth chain (sequential) ──
    //
    // Order-sensitive (the ensure chain) or thread-pinned (the row-admin
    // channel and REACH_CACHE are thread_locals) — see `compute_roads`.
    struct RailPre {
        rail_type: RailType,
        speed: f64,
        q_pax: f64,
        q_frt: f64,
        /// C1 per-region (pax_pct, frt_pct, hours) triplets, resolved from the
        /// segment's admin on the scheduler thread.
        periods: [(f64, f64, f64); 3],
        src_alt: f64,
        d_slant: f64,
        /// `Some` = arc-screened, against exactly this frozen growth state.
        snapshot: Option<SkylineSnapshot>,
    }
    let mut skyline = ArcSkyline::default();
    let mut epoch_snap: Option<SkylineSnapshot> = None;
    let mut pre: Vec<(usize, RailPre)> = Vec::with_capacity(railways.len());
    for (seg_i, seg) in railways.iter().enumerate() {
        if seg.tunnel {
            continue;
        }

        let rail_type = RailType::from_u8(seg.rail_type);
        let speed = if seg.speed_kmh > 0.0 {
            seg.speed_kmh
        } else {
            80.0
        };
        let q_pax = seg.trains_passenger.max(0.0);
        let q_frt = seg.trains_freight.max(0.0);
        if q_pax + q_frt <= 0.0 {
            continue;
        }
        // The segment's own baked admin when present (plan M5); `None` — no
        // channel, no columns on the row's batch, or a mis-aligned channel —
        // falls back to the receiver admin (pre-bake behaviour, unchanged).
        let admin = railway::rail_row_admin(seg_i, railways.len()).unwrap_or(receiver_admin);
        // Per-row audibility reach: this segment's own 25 dB Lden crossing,
        // clamped [2 km, 10 km]. The heatmap loader sets the identical value on
        // each `LineRow` from the SAME `rail_reach_m` solver (the popup's
        // `RailSegment.trains_*` are already the effective post-scaling counts),
        // so popup and heatmap cull at the same distance by construction — no
        // blanket constant, no magic-number drift.
        //
        // Memoized per worker thread: segments materialize per QUERY, and the
        // 40-step bisection (~µs) × thousands of in-ceiling segments would
        // re-pay ~5-10 ms on every popup (Codex /gg on 48085647). Effective
        // (type, speed, counts) tuples collapse onto a handful of defaults,
        // so an exact-key cache hits ~99%.
        let reach_m = REACH_CACHE.with(|c| {
            // Full admin triplet in the key: rail reach is ISO-only today, but
            // the moment a per-country override keyed on anything else lands,
            // an ISO-only key would serve a stale reach with no test failing
            // (/gg M4/M5 #5). A tuple of Copy primitives costs nothing extra.
            let key = (
                seg.rail_type,
                admin.country_iso,
                admin.city_id,
                admin.continent as u8,
                speed.to_bits(),
                q_pax.to_bits(),
                q_frt.to_bits(),
            );
            *c.borrow_mut()
                .entry(key)
                .or_insert_with(|| railway::rail_reach_m(admin, rail_type, speed, q_pax, q_frt))
        });
        if seg.dist_m > reach_m {
            continue;
        }

        let src_elev = rasters.elevation(seg.cp_lat, seg.cp_lon);
        let src_alt = src_elev + SOURCE_HEIGHT_RAIL;
        let d_slant = geo::slant_dist(seg.dist_m, src_alt, rcv_alt);
        if d_slant < 1.0 {
            continue;
        }

        // C1 per-region, per-category day/evening/night split for THIS segment's
        // type (trams take the urban pax curve; only RailType::Rail in an EU
        // region gets the night-heavy freight share). Same table the heatmap
        // loader + reach solver consume → popup-vs-heatmap parity by construction.
        let td = railway::rail_time_dist(admin, rail_type);
        let periods = td.periods();

        // Early exit: skip only if the LOUDEST period's free-field is below
        // threshold — a true upper bound, so no audible-in-any-period segment is
        // dropped. Pre-C1 the day block was always loudest (flat 65/20/15), but
        // C1's EU freight night share (0.5458 over 8 h) can beat day, so a
        // day-only gate would prune audible quiet/slow night-freight rows that
        // the heatmap (Lden over all periods) keeps — a parity break (Codex /gg).
        {
            let me = periods
                .iter()
                .map(|&(pax_pct, frt_pct, hours)| {
                    railway::railway_emission(
                        rail_type,
                        speed,
                        q_pax * pax_pct,
                        q_frt * frt_pct,
                        hours,
                    )
                    .iter()
                    .cloned()
                    .fold(f64::NEG_INFINITY, f64::max)
                })
                .fold(f64::NEG_INFINITY, f64::max);
            if geo::below_free_field_threshold_line(me, seg.dist_m, 0.0) {
                continue;
            }
        }

        // Arc pre-gate + growth-chain replay (shared step — see
        // `crate::arc_growth_chain_step`). Rail segments are the longest in
        // the extract (p90 182 m vs roads' 106 m), so this is where the
        // stripe defect was worst.
        let snapshot = crate::arc_growth_chain_step(
            &mut skyline,
            &mut epoch_snap,
            arc_set,
            receiver,
            seg.start_lat,
            seg.start_lon,
            seg.end_lat,
            seg.end_lon,
            seg.dist_m,
            seg.length_m as f64,
            SOURCE_HEIGHT_RAIL,
            bounds,
        );

        pre.push((
            seg_i,
            RailPre {
                rail_type,
                speed,
                q_pax,
                q_frt,
                periods,
                src_alt,
                d_slant,
                snapshot,
            },
        ));
    }

    // ── Pass 2: per-segment evaluation (parallel, bit-deterministic) ──
    struct RailSegOut {
        seg_variants: [PropagationVariants; 3],
        day_emission_energy: f64,
        ground_g: f64,
        /// Tallest vector obstacle on the characteristic-point path.
        seg_max_bh: f64,
        trace: Option<SegmentTrace>,
    }
    let collect_traces = traces.is_some();
    let outs: Vec<RailSegOut> = pre
        .par_iter()
        .map_init(
            // Per-worker scratch — see the twin comment in compute_roads.
            || {
                (
                    propagation::PathProfile::new(),
                    ArcScreeningScratch::new(),
                    Vec::new(),
                    Vec::new(),
                )
            },
            |(path_profile, arc_scratch, cand_scratch, hist_scratch), (seg_i, p)| {
                let seg = &railways[*seg_i];
                let (rail_type, speed, q_pax, q_frt) = (p.rail_type, p.speed, p.q_pax, p.q_frt);
                let (src_alt, d_slant) = (p.src_alt, p.d_slant);
                // Finite-line geometry runs on the perpendicular distance to
                // the segment's INFINITE line paired with the signed foot
                // position, while divergence/atmosphere stay on `seg.dist_m`
                // (fix-pack C). `seg.fraction` is the clamped foot — the
                // signed one comes from the recomputed decomposition.
                let pts = geo::point_to_segment_full(
                    receiver.lat,
                    receiver.lon,
                    seg.start_lat,
                    seg.start_lon,
                    seg.end_lat,
                    seg.end_lon,
                );
                let flc = geo::finite_line_correction_for_divergence(
                    seg.length_m as f64,
                    pts.d_perp_m,
                    pts.fraction,
                    seg.dist_m,
                );

                // Unified path profile — one sampling, four rasters. One buffer
                // per WORKER; `build_path_profile` clears before every fill.
                rasters.build_path_profile(
                    seg.cp_lat,
                    seg.cp_lon,
                    receiver.lat,
                    receiver.lon,
                    seg.dist_m,
                    path_profile,
                );
                // The current arc payload carries one CP ground vector for its
                // fan; form it from this ray's bare-earth OLS + IMD profile.
                // Node evaluation later removes that compatibility seam and
                // carries each fan ray's full composite directly.
                let ground_path = propagation::path_effects::cnossos_ground_path_from_profile(
                    path_profile,
                    src_alt,
                    rcv_alt,
                    seg.bridge,
                );
                let ground_g = ground_path.ground_path_g;
                let ground_bands = iso9613::ground_atten_bands(ground_path);
                let (terrain, _terrain_profile_points) =
                    propagation::path_effects::terrain_attenuation_with_meta(
                        path_profile,
                        src_alt,
                        rcv_alt,
                    );
                let obstacle_input = crate::obstacle_input_for_ray(
                    obstacles,
                    cand_scratch,
                    seg.cp_lat,
                    seg.cp_lon,
                    receiver.lat,
                    receiver.lon,
                    Some(&propagation::obstacle_index::CellPrune::for_profile(
                        path_profile,
                        src_alt,
                        rcv_alt,
                    )),
                );
                let (cp_screening_atten, obstacle_trace) =
                    propagation::path_effects::screening_attenuation_with_meta(
                        path_profile,
                        obstacle_input,
                        src_alt,
                        rcv_alt,
                        0.0, // railways: no exclusion radius
                        &terrain.attenuation_bands,
                        terrain.dominant_delta_m(),
                    );
                // Arc screening (fix-pack Fix 1) — the snapshot is pass 1's
                // verdict on whether (and against which growth state) this
                // segment is arc-screened; see the twin block in roads.rs.
                let (screening_atten, screening_fan) = match &p.snapshot {
                    None => (cp_screening_atten, None),
                    Some(snap) => crate::arc_screened_line_segment_prepared(
                        &crate::LineSegmentScreening {
                            receiver,
                            start_lat: seg.start_lat,
                            start_lon: seg.start_lon,
                            end_lat: seg.end_lat,
                            end_lon: seg.end_lon,
                            cp_lat: seg.cp_lat,
                            cp_lon: seg.cp_lon,
                            src_alt_m: src_alt,
                            cp_screening: &cp_screening_atten,
                            cp_terrain: &terrain.attenuation_bands,
                            ground_g,
                            ground_bands: &ground_bands,
                            source_height_m: SOURCE_HEIGHT_RAIL,
                            length_m: seg.length_m as f64,
                            dist_m: seg.dist_m,
                            obstacles,
                        },
                        rasters,
                        snap,
                        arc_scratch,
                        collect_traces.then_some(&obstacle_trace),
                    ),
                };
                let veg_atten =
                    propagation::path_effects::vegetation_attenuation_path(path_profile);

                let mut seg_variants = [
                    PropagationVariants::default(),
                    PropagationVariants::default(),
                    PropagationVariants::default(),
                ];
                let mut day_emission_energy = 0.0f64;
                let mut period_emissions: [[f64; NUM_BANDS]; 3] = [[0.0; NUM_BANDS]; 3];
                for (pi, &(pax_pct, frt_pct, hours)) in p.periods.iter().enumerate() {
                    let emission = railway::railway_emission(
                        rail_type,
                        speed,
                        q_pax * pax_pct,
                        q_frt * frt_pct,
                        hours,
                    );
                    let v = iso9613::propagate_variants_cnossos_ground_full(
                        &emission,
                        d_slant,
                        SourceGeometry::Line,
                        ground_path,
                        &terrain.attenuation_bands,
                        &screening_atten,
                        &veg_atten,
                        reflection,
                        flc,
                    );
                    seg_variants[pi].add(&v);
                    if pi == 0 {
                        // Band energy sum (`j` indexes `emission`); f64 accumulation
                        // order is part of popup byte parity — kept as an index loop.
                        #[allow(clippy::needless_range_loop)]
                        for j in 0..NUM_BANDS {
                            day_emission_energy += crate::propagation::iso9613::fast_exp_f64(
                                emission[j] * std::f64::consts::LN_10 * 0.1,
                            );
                        }
                    }
                    period_emissions[pi] = emission;
                }

                // Group-level obstacle histogram probe — vector crossings in
                // vector mode, raster walk only on the fallback path (twin of
                // the roads histogram; popup transparency only, no dB).
                let (seg_max_bh, _) = obstacles.max_height_crossed(
                    seg.cp_lat,
                    seg.cp_lon,
                    receiver.lat,
                    receiver.lon,
                    hist_scratch,
                );

                // Popup trace, built here so the allocation-heavy part runs in
                // parallel; pass 3 pushes it in segment order.
                let trace = collect_traces.then(|| {
                    build_rail_segment_trace(BuildRailTrace {
                        seg,
                        src_alt,
                        rcv_alt,
                        d_slant,
                        flc,
                        ground_g,
                        ground_bands,
                        reflection_boost_db: reflection,
                        q_pax,
                        q_frt,
                        speed_kmh: speed,
                        path_profile: std::mem::take(path_profile),
                        terrain,
                        screening_atten,
                        screening_fan,
                        obstacle_trace,
                        veg_atten,
                        seg_variants,
                        lw_bands: period_emissions,
                    })
                });

                RailSegOut {
                    seg_variants,
                    day_emission_energy,
                    ground_g,
                    seg_max_bh,
                    trace,
                }
            },
        )
        .collect();

    let unnamed_cluster_ids =
        unnamed_track_cluster_ids(pre.iter().map(|(seg_i, _)| &railways[*seg_i]));
    let mut total_energy = [0.0f64; 3];

    // ── Pass 3: accumulation, in segment order (sequential) ──
    for ((seg_i, p), mut out) in pre.iter().zip(outs) {
        let seg = &railways[*seg_i];
        let (rail_type, speed, q_pax, q_frt) = (p.rail_type, p.speed, p.q_pax, p.q_frt);
        let (src_alt, d_slant) = (p.src_alt, p.d_slant);
        let (seg_variants, ground_g) = (out.seg_variants, out.ground_g);
        add_segment_to_total(&mut total_energy, &seg_variants);

        // Named/ref'd tracks keep the exact historical tuple. Unnamed ways use
        // the precomputed transitive proximity component; accumulation below is
        // otherwise unchanged, including dominant/closest metadata and geometry.
        let cluster_id = if is_unnamed_track(seg) {
            Some(
                *unnamed_cluster_ids
                    .get(&(seg.rail_type, seg.osm_id))
                    .expect("every kept unnamed rail way is clustered"),
            )
        } else {
            None
        };
        let key = (
            seg.rail_ref.clone(),
            seg.name.clone(),
            seg.rail_type,
            cluster_id,
        );
        let acc = rails_by_key.entry(key).or_insert_with(|| RailAccum {
            name: {
                // Build display name: "trať 250 — Brno–Havlíčkův Brod" or "trať 250" or name or "Rail"
                if !seg.rail_ref.is_empty() && !seg.name.is_empty() {
                    format!("trať {} — {}", seg.rail_ref, seg.name)
                } else if !seg.rail_ref.is_empty() {
                    format!("trať {}", seg.rail_ref)
                } else if !seg.name.is_empty() {
                    seg.name.clone()
                } else {
                    String::new()
                }
            },
            rail_type,
            rail_type_u8: seg.rail_type,
            dominant_usage_u8: seg.usage,
            dominant_osm_id: seg.osm_id,
            min_dist: f64::MAX,
            min_d_slant: 0.0,
            min_ground_g: 0.5,
            cp_lat: seg.cp_lat,
            cp_lon: seg.cp_lon,
            src_height: src_alt,
            dominant_segment_idx: 0,
            dominant_distance_m: 0.0,
            dominant_trains_passenger_raw: 0.0,
            dominant_trains_freight_raw: 0.0,
            dominant_trains_passenger_effective: 0.0,
            dominant_trains_freight_effective: 0.0,
            dominant_trains_passenger_source: "default_by_type",
            dominant_trains_freight_source: "default_by_type",
            dominant_source_id: 0,
            dominant_maxspeed_posted: 0,
            dominant_speed_used: 0.0,
            dominant_speed_source: "type_default",
            dominant_service: false,
            dominant_highspeed: false,
            dominant_parallel_divisor: 1,
            segment_count: 0,
            total_length_m: 0.0,
            obstacle_segment_count: 0,
            obstacle_height_sum: 0.0,
            obstacle_max_height: 0.0,
            obstacle_max_segment_idx: 0,
            variants: [
                PropagationVariants::default(),
                PropagationVariants::default(),
                PropagationVariants::default(),
            ],
            emission_energy: 0.0,
            line_coords: Vec::new(),
            dominant_bridge: false,
            dominant_energy: 0.0,
            dominant_trace_idx: None,
        });
        // Aggregation
        acc.segment_count += 1;
        acc.total_length_m += seg.length_m as f64;
        // Group-level obstacle histogram — probed in pass 2 (pure raster read).
        {
            let seg_max_bh = out.seg_max_bh;
            if seg_max_bh > 2.0 {
                acc.obstacle_segment_count += 1;
                acc.obstacle_height_sum += seg_max_bh;
                if seg_max_bh > acc.obstacle_max_height {
                    acc.obstacle_max_height = seg_max_bh;
                    acc.obstacle_max_segment_idx = seg.segment_idx;
                }
            }
        }
        // Period-variant merge — `pi` indexes the two parallel variant arrays.
        #[allow(clippy::needless_range_loop)]
        for pi in 0..3 {
            acc.variants[pi].add(&seg_variants[pi]);
        }
        acc.emission_energy += out.day_emission_energy;
        if seg.dist_m < acc.min_dist {
            acc.min_dist = seg.dist_m;
            acc.min_d_slant = d_slant;
            acc.min_ground_g = ground_g;
            acc.cp_lat = seg.cp_lat;
            acc.cp_lon = seg.cp_lon;
            acc.src_height = src_alt;
        }
        acc.line_coords
            .push([[seg.start_lon, seg.start_lat], [seg.end_lon, seg.end_lat]]);

        // Dominant segment — highest received energy drives the popup display
        // metadata (speed, train counts, service, highspeed, parallel_divisor),
        // mirroring the road pattern at line ~720. The gate runs OUTSIDE the
        // trace block so the metadata is correct even when traces aren't being
        // collected. `crosses_dominant` is reused inside the trace block to
        // tag the corresponding `dominant_trace_idx` without re-comparing.
        let seg_energy: f64 = seg_variants[0].full_energy;
        let crosses_dominant = seg_energy > acc.dominant_energy;
        if crosses_dominant {
            acc.dominant_energy = seg_energy;
            acc.dominant_segment_idx = seg.segment_idx;
            acc.dominant_distance_m = seg.dist_m;
            acc.dominant_trains_passenger_raw = seg.trains_passenger;
            acc.dominant_trains_freight_raw = seg.trains_freight;
            acc.dominant_trains_passenger_effective = q_pax;
            acc.dominant_trains_freight_effective = q_frt;
            acc.dominant_trains_passenger_source = match seg.trains_passenger_source {
                0 => "arrow",
                _ => "default_by_type",
            };
            acc.dominant_trains_freight_source = match seg.trains_freight_source {
                0 => "arrow",
                _ => "default_by_type",
            };
            acc.dominant_source_id = seg.source_id;
            acc.dominant_maxspeed_posted = seg.maxspeed;
            acc.dominant_speed_used = speed;
            acc.dominant_speed_source = match seg.speed_source {
                0 => "osm_maxspeed",
                1 => "highspeed_default",
                _ => "type_default",
            };
            acc.dominant_service = seg.service;
            acc.dominant_highspeed = seg.highspeed;
            // A merged row's identity, usage and "(bridge)" follow its loudest way.
            acc.dominant_osm_id = seg.osm_id;
            acc.dominant_usage_u8 = seg.usage;
            acc.dominant_bridge = seg.bridge;
            acc.dominant_parallel_divisor = seg.parallel_divisor.max(1);
        }

        // Popup trace: push pass 2's prebuilt trace (segment order preserved)
        // + tag the dominant one so is_dominant_of_group flips after the loop.
        if let Some(t) = traces.as_deref_mut() {
            let trace = out
                .trace
                .take()
                .expect("pass 2 builds a trace for every kept segment when collecting");
            let trace_idx = t.segments.len();
            t.segments.push(trace);
            if crosses_dominant {
                acc.dominant_trace_idx = Some(trace_idx);
            }
        }
    }

    if let Some(t) = traces {
        for acc in rails_by_key.values() {
            if let Some(idx) = acc.dominant_trace_idx {
                if let Some(tr) = t.segments.get_mut(idx) {
                    tr.is_dominant_of_group = true;
                }
            }
        }
    }

    let mut contributors = Vec::new();
    // Ascending group key, not HashMap order — see `crate::compute::key_sorted`.
    for (_, acc) in crate::compute::key_sorted(&rails_by_key) {
        let ld = PropagationVariants::to_db(acc.variants[0].full_energy);
        let le = PropagationVariants::to_db(acc.variants[1].full_energy);
        let ln = PropagationVariants::to_db(acc.variants[2].full_energy);
        let rail_periods = periods::periods(ld, le, ln);

        let ld_free = PropagationVariants::to_db(acc.variants[0].free_field_energy);
        let le_free = PropagationVariants::to_db(acc.variants[1].free_field_energy);
        let ln_free = PropagationVariants::to_db(acc.variants[2].free_field_energy);
        let free_periods = periods::periods(ld_free, le_free, ln_free);

        let geometry = if !acc.line_coords.is_empty() {
            Some(serde_json::json!({"type": "MultiLineString", "coordinates": acc.line_coords}))
        } else {
            None
        };

        let rail_effects = compute_path_effects(
            rasters,
            obstacles,
            acc.cp_lat,
            acc.cp_lon,
            acc.src_height,
            receiver,
            acc.min_dist,
            0.0,
        );

        let impacts = PropagationVariants::impact_deltas(&acc.variants, rail_periods.lden_db);

        // Headline rail metadata: dominant (loudest) segment, mirroring the
        // road-contributor pattern. `closest_*` is still tracked on the
        // accumulator for the propagation baseline (`min_dist`, `cp_lat/lon`,
        // `min_d_slant`, `min_ground_g`) but no longer feeds these display
        // fields — closest mis-represented audible traffic whenever a busy
        // mainline sat farther than a quiet siding.
        let rail_meta = RailMetadata {
            trains_passenger_raw: acc.dominant_trains_passenger_raw,
            trains_freight_raw: acc.dominant_trains_freight_raw,
            trains_passenger_source: acc.dominant_trains_passenger_source,
            trains_freight_source: acc.dominant_trains_freight_source,
            source_id: acc.dominant_source_id,
            maxspeed_posted_kmh: acc.dominant_maxspeed_posted,
            trains_passenger_effective: acc.dominant_trains_passenger_effective,
            trains_freight_effective: acc.dominant_trains_freight_effective,
            speed_kmh: acc.dominant_speed_used,
            speed_source: acc.dominant_speed_source,
            rail_type: rail_type_name(acc.rail_type_u8),
            usage: rail_usage_name(acc.dominant_usage_u8),
            service: acc.dominant_service,
            highspeed: acc.dominant_highspeed,
            parallel_divisor: acc.dominant_parallel_divisor,
            dominant_segment_idx: acc.dominant_segment_idx,
            dominant_distance_m: acc.dominant_distance_m,
            closest_distance_m: acc.min_dist,
            bridge: acc.dominant_bridge,
            segment_count: acc.segment_count,
            total_length_m: acc.total_length_m,
            obstacle_segment_count: acc.obstacle_segment_count,
            obstacle_avg_height_m: if acc.obstacle_segment_count > 0 {
                (acc.obstacle_height_sum / acc.obstacle_segment_count as f64 * 10.0).round() / 10.0
            } else {
                0.0
            },
            obstacle_max_height_m: (acc.obstacle_max_height * 10.0).round() / 10.0,
            obstacle_max_segment_idx: acc.obstacle_max_segment_idx,
            provenance: crate::sources::dataset_meta(acc.dominant_source_id),
        };

        contributors.push(Contributor {
            osm_id: Some(acc.dominant_osm_id),
            geometry,
            source_type: LayerKind::Railway,
            name: if acc.name.is_empty() {
                String::new()
            } else {
                acc.name.clone()
            },
            subtype: {
                let base = format!("{:?}", acc.rail_type);
                if acc.dominant_bridge {
                    format!("{} (bridge)", base)
                } else {
                    base
                }
            },
            distance_m: acc.min_dist,
            periods: rail_periods,
            periods_free: free_periods,
            emission_db: 10.0 * acc.emission_energy.max(1e-12).log10(),
            baseline: iso9613::compute_baseline(
                acc.min_d_slant,
                SourceGeometry::Line,
                acc.min_ground_g,
            ),
            terrain: rail_effects.0,
            screening: rail_effects.1,
            vegetation: rail_effects.2,
            terrain_impact_db: round1(impacts.terrain),
            screening_impact_db: round1(impacts.screening),
            vegetation_impact_db: round1(impacts.vegetation),
            atmospheric_impact_db: round1(impacts.atmospheric),
            ground_impact_db: round1(impacts.ground),
            received_bands: std::array::from_fn(|j| {
                10.0 * acc.variants[0].band_energy[j].max(1e-30).log10()
            }),
            metadata: Some(SourceMetadata::Rail(rail_meta)),
        });
    }

    let ld = 10.0 * total_energy[0].max(1e-12).log10();
    let le = 10.0 * total_energy[1].max(1e-12).log10();
    let ln = 10.0 * total_energy[2].max(1e-12).log10();
    (periods::periods(ld, le, ln), contributors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::{Admin, Continent};

    /// Flat-ground rasters (200 m, G=0.5) mirroring lib.rs' MockRasters.
    struct FlatRasters;
    impl RasterSampler for FlatRasters {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            200.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.5
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    const CZ: Admin = Admin {
        continent: Continent::Europe,
        country_iso: *b"CZ",
        city_id: 0,
    };
    const TH: Admin = Admin {
        continent: Continent::Asia,
        country_iso: *b"TH",
        city_id: 0,
    };

    /// Freight-heavy mainline (100 pax + 40 freight @ 120 km/h) 500 m from
    /// the receiver — the shape the loader tests prove flips night/day under
    /// the EU split. Tests never init the admin table → receiver UNKNOWN →
    /// world split when the channel is unset.
    fn mainline_segment() -> RailSegment {
        RailSegment {
            osm_id: 1,
            segment_idx: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            end_lat: 50.0,
            end_lon: 14.007,
            length_m: 500.0,
            rail_type: 0,
            usage: 0,
            maxspeed: 120,
            trains_passenger: 100.0,
            trains_freight: 40.0,
            speed_kmh: 120.0,
            track_count: 1,
            name: String::new(),
            rail_ref: String::new(),
            bridge: false,
            tunnel: false,
            service: false,
            highspeed: false,
            parallel_divisor: 1,
            speed_source: 0,
            trains_passenger_source: 0,
            trains_freight_source: 0,
            source_id: 0,
            dist_m: 500.0,
            cp_lat: 50.0,
            cp_lon: 14.0035,
            fraction: 0.5,
        }
    }

    /// 500 m due north of the segment — the geometry has to AGREE with the
    /// fixture's `dist_m`/`cp`/`fraction`, since the finite-line correction
    /// reads the segment's real perpendicular distance (fix-pack C).
    fn receiver() -> Receiver {
        Receiver::new(50.004523, 14.0035, 200.0)
    }

    fn segment_at_receiver_distance(osm_id: i64, rail_type: u8, distance_m: f64) -> RailSegment {
        use grid::geo::M_PER_DEG_LAT;

        let mut segment = mainline_segment();
        let northward_shift_m = 500.0 - distance_m;
        let latitude_shift = northward_shift_m / M_PER_DEG_LAT;
        segment.osm_id = osm_id;
        segment.segment_idx = osm_id as i16;
        segment.rail_type = rail_type;
        segment.start_lat += latitude_shift;
        segment.end_lat += latitude_shift;
        segment.cp_lat += latitude_shift;
        segment.dist_m = distance_m;
        segment
    }

    fn periods_for(segs: &[RailSegment]) -> NoisePeriods {
        compute_railways(&receiver(), segs, &ObstacleSet::empty(), &FlatRasters, None).0
    }

    fn contributors_for(segs: &[RailSegment]) -> Vec<Contributor> {
        compute_railways(&receiver(), segs, &ObstacleSet::empty(), &FlatRasters, None).1
    }

    /// Test-only reference implementation of the former all-pairs scan. It
    /// shares the way collapse and union semantics with production, so the
    /// 10,000-way test isolates candidate enumeration rather than comparing
    /// two subtly different clustering rules.
    fn brute_force_cluster_ids<'a>(
        segments: impl Iterator<Item = &'a RailSegment>,
    ) -> std::collections::HashMap<(u8, i64), i64> {
        let ways = unnamed_way_closest_points(segments);
        let mut parents: Vec<_> = (0..ways.len()).collect();
        for left in 0..ways.len() {
            for right in (left + 1)..ways.len() {
                if ways[left].rail_type != ways[right].rail_type {
                    break;
                }
                if geo::flat_dist(
                    ways[left].lat,
                    ways[left].lon,
                    ways[right].lat,
                    ways[right].lon,
                ) <= RAIL_TRACK_LINK_M
                {
                    union_toward_smaller(&mut parents, left, right);
                }
            }
        }
        ways.iter()
            .enumerate()
            .map(|(index, way)| {
                (
                    (way.rail_type, way.osm_id),
                    ways[union_find_root(&mut parents, index)].osm_id,
                )
            })
            .collect()
    }

    #[test]
    fn bucketed_clustering_matches_bruteforce_for_10000_ways() {
        use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};

        let receiver = Receiver::new(50.0850, 14.4234, 200.0);
        let m_lon = m_per_deg_lon(receiver.lat.to_radians());
        let mut segments = Vec::with_capacity(10_000);
        for index in 0..10_000 {
            let row = index / 100;
            let column = index % 100;
            let x_m = (column as f64 - 49.5) * 100.0;
            let y_m = (row as f64 - 49.5) * 100.0;
            let mut segment = mainline_segment();
            segment.osm_id = 1_000_000 + index as i64;
            segment.segment_idx = index as i16;
            segment.rail_type = (index % 5) as u8;
            segment.dist_m = 1_000.0;
            segment.cp_lat = receiver.lat + y_m / M_PER_DEG_LAT;
            segment.cp_lon = receiver.lon + x_m / m_lon;
            segments.push(segment);
        }

        let bucketed = unnamed_track_cluster_ids(segments.iter());
        let brute_force = brute_force_cluster_ids(segments.iter());
        assert_eq!(bucketed, brute_force);
    }

    /// The grid is only a candidate filter and must not be coarser than `flat_dist`:
    /// a pair `flat_dist` links at 149.9 m sits 150.04 m apart in a frame scaled at a
    /// receiver 5 km south of it, which with 150 m cells in that frame put the two
    /// ways two cells apart, never tested, never linked (found 2026-09-03).
    #[test]
    fn bucket_cells_keep_flat_dist_linked_pairs_adjacent() {
        use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};

        let receiver = Receiver::new(50.0850, 14.4234, 200.0);
        let m_lon = m_per_deg_lon(receiver.lat.to_radians());
        let y_m = 5_000.0;
        let mut ways = Vec::new();
        for (osm_id, x_m) in [(700, 149.99), (701, 300.01)] {
            let mut segment = mainline_segment();
            segment.osm_id = osm_id;
            segment.segment_idx = (osm_id - 700) as i16;
            segment.dist_m = y_m;
            segment.cp_lat = receiver.lat + y_m / M_PER_DEG_LAT;
            segment.cp_lon = receiver.lon + x_m / m_lon;
            ways.push(segment);
        }
        let linked = geo::flat_dist(
            ways[0].cp_lat,
            ways[0].cp_lon,
            ways[1].cp_lat,
            ways[1].cp_lon,
        );
        assert!(
            linked <= RAIL_TRACK_LINK_M,
            "fixture must be linkable: {linked} m"
        );
        let bucketed = unnamed_track_cluster_ids(ways.iter());
        assert_eq!(bucketed, brute_force_cluster_ids(ways.iter()));
        assert_eq!(bucketed[&(0, 700)], bucketed[&(0, 701)]);
    }

    #[test]
    fn rail_total_is_bit_equal_when_group_key_changes() {
        // The same physical segments model the old unnamed single-group key
        // when given one shared name; their deliberately shuffled IDs force
        // the new proximity keys into a different group-fold order.
        let segments = vec![
            segment_at_receiver_distance(900, 0, 700.0),
            segment_at_receiver_distance(100, 0, 100.0),
            segment_at_receiver_distance(500, 0, 220.0),
        ];
        let clustered = periods_for(&segments);
        let mut single_group = segments.clone();
        for segment in &mut single_group {
            segment.name = "legacy single group".to_owned();
        }
        let single_group = periods_for(&single_group);
        let bits = |periods: NoisePeriods| {
            [periods.ld_db, periods.le_db, periods.ln_db, periods.lden_db].map(f64::to_bits)
        };
        assert_eq!(bits(clustered), bits(single_group));
    }

    /// Gate (d) popup: the EU vs world period split follows the SEGMENT's
    /// baked ISO, not the receiver's admin.
    #[test]
    fn baked_iso_drives_eu_split() {
        let seg = mainline_segment();
        crate::emission::railway::set_rail_row_admins(Some(vec![Some(CZ)]));
        let eu = periods_for(std::slice::from_ref(&seg));
        crate::emission::railway::set_rail_row_admins(Some(vec![Some(TH)]));
        let world = periods_for(std::slice::from_ref(&seg));
        crate::emission::railway::set_rail_row_admins(None);
        assert!(
            eu.ln_db > eu.ld_db,
            "baked CZ: EU freight night {:.2} must exceed day {:.2}",
            eu.ln_db,
            eu.ld_db
        );
        assert!(
            world.ld_db > world.ln_db,
            "baked TH: world day {:.2} must exceed night {:.2}",
            world.ld_db,
            world.ln_db
        );
    }

    /// Gate (b) popup rail: a channel of `None` entries ≡ no channel — the
    /// receiver path is bit-identical to the pre-bake kernel.
    #[test]
    fn none_channel_is_receiver_path_bit_identical() {
        let segs = vec![mainline_segment()];
        let plain = periods_for(&segs);
        crate::emission::railway::set_rail_row_admins(Some(vec![None]));
        let channeled = periods_for(&segs);
        crate::emission::railway::set_rail_row_admins(None);
        assert_eq!(plain.ld_db, channeled.ld_db);
        assert_eq!(plain.le_db, channeled.le_db);
        assert_eq!(plain.ln_db, channeled.ln_db);
        assert_eq!(plain.lden_db, channeled.lden_db);
    }

    /// Six ways across one tram street stay one visitor-facing source row.
    #[test]
    fn six_nearby_unnamed_tram_ways_share_a_row() {
        let mut segs: Vec<_> = (0..6)
            .map(|index| segment_at_receiver_distance(100 + index, 1, 80.0 + 18.0 * index as f64))
            .collect();
        // The closest points span 90 m. The closest/loudest way owns the
        // identity and bridge label while the row keeps all six geometries.
        segs[0].bridge = true;
        let contribs = contributors_for(&segs);
        assert_eq!(contribs.len(), 1);
        let tram = &contribs[0];
        assert!(tram.name.is_empty());
        assert_eq!(tram.osm_id, Some(100));
        assert_eq!(tram.distance_m, 80.0);
        assert_eq!(tram.subtype, "Tram (bridge)");
        let Some(SourceMetadata::Rail(metadata)) = &tram.metadata else {
            panic!("tram contributor must carry rail metadata");
        };
        assert_eq!(metadata.segment_count, 6);
    }

    /// Any name or ref keeps the historical (ref, name, type) identity.
    #[test]
    fn named_or_refd_tracks_keep_the_historical_key() {
        let mut same_name_far_apart = vec![
            segment_at_receiver_distance(150, 0, 100.0),
            segment_at_receiver_distance(151, 0, 700.0),
        ];
        for segment in &mut same_name_far_apart {
            segment.name = "Corridor A".to_owned();
        }
        assert_eq!(contributors_for(&same_name_far_apart).len(), 1);

        let mut different_names_nearby = vec![
            segment_at_receiver_distance(160, 0, 100.0),
            segment_at_receiver_distance(161, 0, 120.0),
        ];
        different_names_nearby[0].name = "Corridor A".to_owned();
        different_names_nearby[1].name = "Corridor B".to_owned();
        assert_eq!(contributors_for(&different_names_nearby).len(), 2);

        let mut same_ref_far_apart = vec![
            segment_at_receiver_distance(170, 0, 100.0),
            segment_at_receiver_distance(171, 0, 700.0),
        ];
        for segment in &mut same_ref_far_apart {
            segment.rail_ref = "120".to_owned();
        }
        assert_eq!(contributors_for(&same_ref_far_apart).len(), 1);
    }

    /// Rail type remains an identity boundary even when both ways are unnamed.
    #[test]
    fn nearby_unnamed_tram_and_distant_railway_stay_separate() {
        let segs = vec![
            segment_at_receiver_distance(200, 1, 85.0),
            segment_at_receiver_distance(201, 0, 784.0),
        ];
        let contribs = contributors_for(&segs);
        assert_eq!(contribs.len(), 2);
        assert!(contribs
            .iter()
            .any(|contributor| contributor.subtype == "Tram" && contributor.distance_m == 85.0));
        assert!(contribs.iter().any(|contributor| {
            contributor.subtype == "Rail" && contributor.distance_m == 784.0
        }));

        // A railway between two tram closest points must not bridge those
        // trams into one component through the union-find.
        let cross_type_bridge = vec![
            segment_at_receiver_distance(210, 1, 100.0),
            segment_at_receiver_distance(211, 0, 200.0),
            segment_at_receiver_distance(212, 1, 300.0),
        ];
        assert_eq!(contributors_for(&cross_type_bridge).len(), 3);
    }

    /// Proximity splits distant same-type corridors and closes transitively.
    #[test]
    fn unnamed_railway_tracks_use_transitive_proximity_clusters() {
        let separated = vec![
            segment_at_receiver_distance(300, 0, 100.0),
            segment_at_receiver_distance(301, 0, 700.0),
        ];
        let separated_contribs = contributors_for(&separated);
        assert_eq!(
            separated_contribs.len(),
            2,
            "closest points 600 m apart are different tracks"
        );

        // Relative closest-point positions are 0 / 120 / 240 m. The endpoints
        // do not link directly, so one row proves that closure is transitive.
        let chained = vec![
            segment_at_receiver_distance(400, 0, 100.0),
            segment_at_receiver_distance(401, 0, 220.0),
            segment_at_receiver_distance(402, 0, 340.0),
        ];
        let chained_contribs = contributors_for(&chained);
        assert_eq!(chained_contribs.len(), 1);
        let Some(SourceMetadata::Rail(metadata)) = &chained_contribs[0].metadata else {
            panic!("rail contributor must carry rail metadata");
        };
        assert_eq!(metadata.segment_count, 3);

        // The far microsegment comes first. Clustering per microsegment, or
        // taking a way's first segment instead of its closest, would merge the
        // two OSM ways through the 100 m far-segment gap.
        let mut far_first = segment_at_receiver_distance(500, 0, 900.0);
        far_first.segment_idx = 0;
        let mut near_second = segment_at_receiver_distance(500, 0, 100.0);
        near_second.segment_idx = 1;
        let closest_other_way = segment_at_receiver_distance(501, 0, 800.0);
        let per_way = contributors_for(&[far_first, near_second, closest_other_way]);
        assert_eq!(per_way.len(), 2);
        let two_segment_way = per_way
            .iter()
            .find(|contributor| contributor.osm_id == Some(500))
            .expect("the two-microsegment way keeps its own row");
        let Some(SourceMetadata::Rail(metadata)) = &two_segment_way.metadata else {
            panic!("rail contributor must carry rail metadata");
        };
        assert_eq!(metadata.segment_count, 2);

        let cluster_receiver = receiver();
        let mut at_limit = vec![
            segment_at_receiver_distance(600, 0, 100.0),
            segment_at_receiver_distance(601, 0, 250.0),
        ];
        at_limit[0].cp_lat = cluster_receiver.lat;
        at_limit[0].cp_lon = cluster_receiver.lon;
        at_limit[1].cp_lat = cluster_receiver.lat + RAIL_TRACK_LINK_M / grid::geo::M_PER_DEG_LAT;
        at_limit[1].cp_lon = cluster_receiver.lon;
        let at_limit_clusters = unnamed_track_cluster_ids(at_limit.iter());
        assert_eq!(at_limit_clusters[&(0, 600)], at_limit_clusters[&(0, 601)]);

        let mut above_limit = at_limit;
        above_limit[0].osm_id = 610;
        above_limit[1].osm_id = 611;
        above_limit[1].cp_lat =
            cluster_receiver.lat + (RAIL_TRACK_LINK_M + 0.01) / grid::geo::M_PER_DEG_LAT;
        let above_limit_clusters = unnamed_track_cluster_ids(above_limit.iter());
        assert_ne!(
            above_limit_clusters[&(0, 610)],
            above_limit_clusters[&(0, 611)]
        );
    }

    /// THE PARALLELISM GATE (rail twin of roads'
    /// `pool_size_never_changes_the_bits`): the rayon pool size must never
    /// move a bit — the three-pass kernel folds pass-2 results in segment
    /// order, so periods, contributors and traces are byte-stable.
    #[test]
    fn pool_size_never_changes_the_bits() {
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
        use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};
        let mut segs = Vec::new();
        for k in 0..12 {
            let mut seg = mainline_segment();
            seg.osm_id = 30 + (k as i64 % 3);
            seg.segment_idx = k as i16;
            let north_m = 120.0 + 140.0 * k as f64;
            let dlat = north_m / M_PER_DEG_LAT;
            seg.start_lat += dlat;
            seg.end_lat += dlat;
            seg.cp_lat += dlat;
            seg.dist_m = 500.0 - north_m.min(380.0);
            segs.push(seg);
        }
        let lat_of = |north_m: f64| 50.0 + north_m / M_PER_DEG_LAT;
        let lon_of = |east_m: f64| 14.0 + east_m / m_per_deg_lon(50.0_f64.to_radians());
        let mut b = ObstacleIndex::builder(50.0, 14.0);
        for c in -2i32..=2 {
            let x = 200.0 + c as f64 * 120.0;
            b.add_ring(
                &[
                    (lat_of(140.0), lon_of(x - 15.0)),
                    (lat_of(140.0), lon_of(x + 15.0)),
                    (lat_of(152.0), lon_of(x + 15.0)),
                    (lat_of(152.0), lon_of(x - 15.0)),
                ],
                7.0,
                ObstacleKind::Building,
                (c + 2) as u32,
            );
        }
        let obstacles = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };
        // FULL-output comparison in both trace modes — see the roads twin.
        let run = |threads: usize, with_traces: bool| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("test pool")
                .install(|| {
                    let mut traces = TraceCollector::new();
                    let (periods, contribs) = compute_railways(
                        &receiver(),
                        &segs,
                        &obstacles,
                        &FlatRasters,
                        with_traces.then_some(&mut traces),
                    );
                    (
                        [periods.ld_db, periods.le_db, periods.ln_db, periods.lden_db]
                            .map(f64::to_bits),
                        serde_json::to_string(&contribs).expect("serialize contributors"),
                        serde_json::to_string(&traces.segments).expect("serialize traces"),
                    )
                })
        };
        for with_traces in [true, false] {
            let (bits1, contribs1, traces1) = run(1, with_traces);
            let (bits8, contribs8, traces8) = run(5, with_traces);
            assert_eq!(bits1, bits8, "period bits (traces={with_traces})");
            assert_eq!(contribs1, contribs8, "contributors (traces={with_traces})");
            assert_eq!(traces1, traces8, "traces (traces={with_traces})");
            if with_traces {
                assert_ne!(traces1, "[]", "the scene must produce traces");
                assert_ne!(contribs1, "[]", "the scene must produce contributors");
            }
        }
    }
}
