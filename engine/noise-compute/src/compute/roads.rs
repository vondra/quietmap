//! Road compute kernel — groups road segments by (ref, name, class), emits
//! per-period power and propagates to the receiver (CNOSSOS road). Shared by popup + heatmap.
use crate::*;

/// Compute road noise: emission per period → propagation → Lden per segment.
///
/// THREE PASSES, so the heavy per-segment work can run on every core while the
/// answer stays bit-identical to the sequential loop it replaced (the popup is
/// the acoustic reference — f64 accumulation order is part of the contract):
///
/// 1. **Gates + growth chain** (sequential): per-segment admission gates, and
///    the EXACT skyline-ensure sequence the sequential kernel would run — the
///    one piece that cannot parallelize, because a receiver's [`ArcSkyline`]
///    grows on demand and later segments read the grown result (SPEC §4.7
///    REPRODUCIBILITY). Between two growth steps the skyline is frozen; each
///    segment records a [`SkylineSnapshot`] of the state its sequential twin
///    would have read.
/// 2. **Evaluation** (parallel, rayon): path profile, cp-ray effects, arc
///    quadrature against the frozen snapshot, per-period propagation, the
///    ref-inference scan, the obstacle histogram probe, the popup trace. Pure
///    per segment — per-thread scratch, no shared mutable state — so thread
///    interleaving cannot move a bit; the ordered `collect` keeps results in
///    segment order.
/// 3. **Accumulation** (sequential): the original fold, in segment order —
///    grouping, dominant selection, trace push order, energy sums. Identical
///    statements over identical inputs ⇒ identical bits.
pub(crate) fn compute_roads(
    receiver: &Receiver,
    roads: &[RoadSegment],
    barriers: &[Barrier],
    obstacles: &crate::propagation::obstacle_index::ObstacleSet,
    rasters: &dyn RasterSampler,
    mut traces: Option<&mut TraceCollector>,
) -> (NoisePeriods, Vec<Contributor>) {
    use propagation::arc_screening::{ArcBounds, ArcScreeningScratch, ArcSkyline, SkylineSnapshot};
    use rayon::prelude::*;

    let reflection = rasters.building_enclosure(receiver.lat, receiver.lon);
    let rcv_alt = receiver.altitude_m();
    let bounds = ArcBounds::shipped();
    // The set the arc rule clips against.
    let arc_set = obstacles;

    use std::collections::HashMap;

    // Group segments by (ref, name, class): accumulate energy + collect geometry
    struct RoadAccum {
        class_name: &'static str,
        display_name: String,
        first_osm_id: i64,
        // Closest segment (for distance display, baseline, path-effect context)
        min_dist: f64,
        min_d_slant: f64,
        min_ground_g: f64,
        closest_cp_lat: f64,
        closest_cp_lon: f64,
        closest_src_height: f64,
        // Dominant-segment metadata (highest received energy — what drives the result)
        dominant_energy: f64,
        dominant_segment_idx: i16,
        dominant_distance_m: f64,
        dominant_aadt_light_raw: i32,
        dominant_aadt_medium_raw: i32,
        dominant_aadt_heavy_raw: i32,
        dominant_aadt_moto_raw: i32,
        dominant_aadt_light_nominal: f64,
        dominant_aadt_medium_nominal: f64,
        dominant_aadt_heavy_nominal: f64,
        dominant_aadt_moto_nominal: f64,
        dominant_aadt_light_effective: f64,
        dominant_aadt_medium_effective: f64,
        dominant_aadt_heavy_effective: f64,
        dominant_aadt_moto_effective: f64,
        dominant_traffic_source: &'static str, // "matched_external" | "estimated_service_tree" | "default_by_class"
        dominant_source_id: u16, // dataset identity from pipeline/lib/enrichment-datasets.ts
        dominant_speed_posted: u8,
        dominant_speed_used: f64,
        dominant_speed_source: &'static str, // "osm_posted" | "default_by_class" | "roundabout_cap"
        dominant_surface_type: u8,
        dominant_surface_corr_db: f64,
        dominant_lanes: u8,
        dominant_oneway: bool,
        // Aggregation across all grouped segments
        segment_count: u32,
        total_length_m: f64,
        bridge_count: u32,
        speed_min: f64,
        speed_max: f64,
        oneway_segment_count: u32,
        twoway_segment_count: u32,
        // Group-level screening obstacle histogram (popup transparency)
        obstacle_segment_count: u32,
        obstacle_height_sum: f64,
        obstacle_max_height: f64,
        obstacle_max_segment_idx: i16,
        // Per-period variant energies (full, free-field, no_terrain, no_screening, no_vegetation)
        variants: [PropagationVariants; 3], // day, evening, night
        emission_energy: f64,
        line_coords: Vec<[[f64; 2]; 2]>,
        // Index into the caller's traces.segments Vec of the dominant-segment
        // trace (highest received energy). Populated only when a TraceCollector
        // is active; flipped to `is_dominant_of_group = true` at the end.
        dominant_trace_idx: Option<usize>,
    }
    // Group by (ref, name, class) — not osm_id — so "D1" becomes one contributor.
    // For unnamed roads (ref="" && name=""): group per osm_id (like railway)
    // to avoid merging all unnamed residential streets into one mega-contributor.
    let mut roads_by_key: HashMap<(String, String, u8), RoadAccum> = HashMap::new();

    // Admin resolved once per compute_roads call — receiver position is
    // constant across segments. Uses the process-wide admin table
    // (see admin::init_admin_table at tile-painter/source-reader init).
    // Falls back to Admin::UNKNOWN → WORLD_DEFAULT when uninitialised.
    // M4: when source-reader installed the per-row channel (baked M3
    // columns), each segment's OWN admin overrides this per segment below.
    let receiver_admin = crate::admin::admin_for_latlng(receiver.lat, receiver.lon);

    // ── Pass 1: admission gates + the skyline growth chain (sequential) ──
    //
    // Everything here is either order-sensitive — the ensure chain, where a
    // later segment reads what earlier segments grew — or pinned to THIS
    // thread (the row-admin channel is a thread_local a rayon worker would
    // read as unset, silently flipping segments to the receiver admin).
    // `needs_growth` elides the ensures that are provably no-ops, which is
    // most of them: the ladder snap lands neighbouring segments on one rung.
    struct RoadPre {
        norm: normalize::NormalizedRoad,
        admin: crate::admin::Admin,
        src_alt: f64,
        d_slant: f64,
        /// `Some` = arc-screened, against exactly this frozen growth state.
        /// `None` = the cp-ray verdict stands (span pre-gate, degenerate
        /// span, or no obstacle store and no walls).
        snapshot: Option<SkylineSnapshot>,
    }
    let mut skyline = ArcSkyline::default();
    // One snapshot per growth epoch, shared by every segment inside it —
    // invalidated by each growth, cloned lazily on first use.
    let mut epoch_snap: Option<SkylineSnapshot> = None;
    let mut pre: Vec<(usize, RoadPre)> = Vec::with_capacity(roads.len());
    for (seg_i, seg) in roads.iter().enumerate() {
        // The segment's own baked admin when present (plan M4); `None` — no
        // channel, no columns on the row's batch, or a mis-aligned channel —
        // falls back to the receiver admin (pre-bake behaviour, unchanged).
        let admin = defaults::road_row_admin(seg_i, roads.len()).unwrap_or(receiver_admin);
        let Some(norm) = normalize::normalize_road_segment(seg, admin) else {
            continue;
        };
        if seg.dist_m > norm.max_distance_m {
            continue;
        }

        let src_elev = rasters.elevation(seg.cp_lat, seg.cp_lon);
        let src_alt = src_elev + norm.source_height_m;
        let d_slant = geo::slant_dist(seg.dist_m, src_alt, rcv_alt);
        if d_slant < 1.0 {
            continue;
        }

        // Early exit: skip if free-field < threshold (matching pipeline)
        {
            let ef = road::build_period_flows(
                norm.light_aadt,
                norm.medium_aadt,
                norm.heavy_aadt,
                norm.moto_aadt,
                norm.speed_kmh,
                norm.time_dist().day_pct,
                12.0,
            );
            let ee = road::line_source_emission(&ef, norm.surf_corr_db);
            let me = ee.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if geo::below_free_field_threshold_line(me, seg.dist_m, 0.0) {
                continue;
            }
        }

        // Arc pre-gate + growth-chain replay: growth ORDER is part of the
        // answer (SPEC §4.7 reproducibility), so the ensure its sequential
        // twin would run happens right here, on this thread, in segment order
        // (shared step — see `crate::arc_growth_chain_step`).
        let snapshot = crate::arc_growth_chain_step(
            &mut skyline,
            &mut epoch_snap,
            arc_set,
            receiver,
            barriers,
            seg.start_lat,
            seg.start_lon,
            seg.end_lat,
            seg.end_lon,
            seg.dist_m,
            seg.length_m as f64,
            norm.source_height_m,
            bounds,
        );

        pre.push((
            seg_i,
            RoadPre {
                norm,
                admin,
                src_alt,
                d_slant,
                snapshot,
            },
        ));
    }

    // ── Pass 2: per-segment evaluation (parallel, bit-deterministic) ──
    struct RoadSegOut {
        seg_variants: [PropagationVariants; 3],
        day_emission_energy: f64,
        ground_g: f64,
        /// Tallest vector obstacle on the characteristic-point path.
        seg_max_bh: f64,
        /// Ref inherited from the nearest refed mainline (orphan mainlines
        /// and their links) — the O(segments) scan, off the sequential path.
        effective_ref: String,
        trace: Option<SegmentTrace>,
    }
    let collect_traces = traces.is_some();
    let outs: Vec<RoadSegOut> = pre
        .par_iter()
        .map_init(
            // Per-worker scratch: profile + arc buffers + crossing candidates.
            // Every consumer clears its buffer before use, so results are
            // independent of scratch history — worker count and work-stealing
            // cannot move a bit; the ordered `collect` fixes result order.
            || {
                (
                    propagation::PathProfile::new(),
                    ArcScreeningScratch::new(),
                    Vec::new(),
                    Vec::new(),
                )
            },
            |(path_profile, arc_scratch, cand_scratch, hist_scratch), (seg_i, p)| {
                let seg = &roads[*seg_i];
                let norm = &p.norm;
                let (src_alt, d_slant) = (p.src_alt, p.d_slant);
                let class_name = norm.class_name;
                let light = norm.light_aadt;
                let medium = norm.medium_aadt;
                let heavy = norm.heavy_aadt;
                let moto = norm.moto_aadt;
                let speed = norm.speed_kmh;
                let surf_corr = norm.surf_corr_db;
                let time_dist = norm.time_dist();
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

                // Unified path profile — sampled once, shared across all
                // path-effect calls. One buffer per WORKER: `build_path_profile`
                // clears it before every fill, so reuse is bit-for-bit the same
                // as a fresh `PathProfile` (a São Paulo popup runs this closure
                // ~7.9 k times; the shared buffer only kills the alloc churn).
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
                        barriers,
                        obstacle_input,
                        src_alt,
                        rcv_alt,
                        0.0, // roads: no exclusion radius
                        &terrain.attenuation_bands,
                        terrain.dominant_delta_m(),
                    );
                // Arc screening (fix-pack Fix 1): the cp ray's verdict covers
                // only the directions it flies through; the segment's other
                // azimuths get their own evaluation, energy-averaged over the
                // span. The cp result stays the obstacle trace, the
                // degenerate-span fallback, and the evaluation reused for the
                // interval it falls in. `snapshot` is pass 1's verdict on
                // whether (and against which growth state) this segment is
                // arc-screened.
                let screening_atten = match &p.snapshot {
                    None => cp_screening_atten,
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
                            source_height_m: norm.source_height_m,
                            length_m: seg.length_m as f64,
                            dist_m: seg.dist_m,
                            barriers,
                            obstacles,
                        },
                        rasters,
                        snap,
                        arc_scratch,
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
                for (pi, (pct, hours)) in [
                    (time_dist.day_pct, 12.0),
                    (time_dist.evening_pct, 4.0),
                    (time_dist.night_pct, 8.0),
                ]
                .iter()
                .enumerate()
                {
                    let flows =
                        road::build_period_flows(light, medium, heavy, moto, speed, *pct, *hours);
                    let emission = road::line_source_emission(&flows, surf_corr);
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

                // Ref inheritance: an orphan mainline or its link inherits the
                // ref of the nearest mainline that carries one. Link classes
                // 10/11/12 map to their mainline parents 0/1/2 (so a GC-1
                // on-ramp with no OSM ref=* tag groups under "GC-1 (link)"
                // instead of "osm:123").
                let infer_target_class = match norm.class_idx {
                    0 | 10 => Some(0),
                    1 | 11 => Some(1),
                    2 | 12 => Some(2),
                    _ => None,
                };
                let effective_ref =
                    if let (true, Some(target)) = (seg.road_ref.is_empty(), infer_target_class) {
                        let mut best_ref = String::new();
                        let mut best_dist = f64::MAX;
                        for other in roads.iter() {
                            if (other.road_class as usize) != target {
                                continue;
                            }
                            if other.road_ref.is_empty() {
                                continue;
                            }
                            let d = ((seg.cp_lat - other.cp_lat).powi(2)
                                + (seg.cp_lon - other.cp_lon).powi(2))
                            .sqrt();
                            if d < best_dist {
                                best_dist = d;
                                best_ref = other.road_ref.clone();
                            }
                        }
                        best_ref
                    } else {
                        seg.road_ref.clone()
                    };

                // Group-level obstacle histogram — the popup's "N of M
                // segments had obstacles on path", from the exact footprint
                // crossings.
                let (seg_max_bh, _) = obstacles.max_height_crossed(
                    seg.cp_lat,
                    seg.cp_lon,
                    receiver.lat,
                    receiver.lon,
                    hist_scratch,
                );

                // Popup trace, built here so the allocation-heavy part runs in
                // parallel; pass 3 pushes it in segment order. `std::mem::take`
                // consumes the worker's path_profile (rebuilt from empty on its
                // next segment) so the trace owns the sample arrays without
                // clone — exactly the sequential kernel's behaviour.
                let trace = collect_traces.then(|| {
                    build_road_segment_trace(BuildRoadTrace {
                        seg,
                        class_name,
                        src_alt,
                        rcv_alt,
                        d_slant,
                        flc,
                        ground_g,
                        ground_bands,
                        reflection_boost_db: reflection,
                        light,
                        medium,
                        heavy,
                        moto,
                        speed_kmh: speed,
                        surf_corr,
                        path_profile: std::mem::take(path_profile),
                        terrain,
                        screening_atten,
                        obstacle_trace,
                        veg_atten,
                        seg_variants,
                        lw_bands: period_emissions,
                    })
                });

                RoadSegOut {
                    seg_variants,
                    day_emission_energy,
                    ground_g,
                    seg_max_bh,
                    effective_ref,
                    trace,
                }
            },
        )
        .collect();

    // ── Pass 3: accumulation, in segment order (sequential) ──
    //
    // The original fold, statement for statement: HashMap grouping, dominant
    // selection, trace push order, f64 energy sums. Identical statements over
    // identical inputs in identical order ⇒ identical bits.
    for ((seg_i, p), mut out) in pre.iter().zip(outs) {
        let seg = &roads[*seg_i];
        let class_idx = p.norm.class_idx;
        let class_name = p.norm.class_name;
        let speed = p.norm.speed_kmh;
        let base_speed = p.norm.base_speed_kmh;
        let surf_corr = p.norm.surf_corr_db;
        let (light, medium, heavy, moto) = (
            p.norm.light_aadt,
            p.norm.medium_aadt,
            p.norm.heavy_aadt,
            p.norm.moto_aadt,
        );
        let (admin, src_alt, d_slant) = (p.admin, p.src_alt, p.d_slant);
        let (seg_variants, ground_g) = (out.seg_variants, out.ground_g);
        let effective_ref = std::mem::take(&mut out.effective_ref);

        // For unnamed roads: group per osm_id (like railway), not catch-all
        let key_ref = if effective_ref.is_empty() && seg.name.is_empty() {
            format!("osm:{}", seg.osm_id)
        } else {
            effective_ref.clone()
        };

        let key = (key_ref.clone(), seg.name.clone(), seg.road_class);
        let link_suffix = match class_idx {
            10..=12 => " (link)",
            _ => "",
        };
        let acc = roads_by_key.entry(key).or_insert_with(|| {
            let display_name = if !effective_ref.is_empty() && !seg.name.is_empty() {
                format!("{} — {}{}", effective_ref, seg.name, link_suffix)
            } else if !effective_ref.is_empty() {
                format!("{}{}", effective_ref, link_suffix)
            } else if !seg.name.is_empty() {
                format!("{}{}", seg.name, link_suffix)
            } else {
                // Fallback with distance badge for unnamed roads
                let class_label = match class_idx {
                    0 => "Motorway",
                    1 => "Trunk road",
                    2 => "Primary road",
                    3 => "Secondary road",
                    4 => "Tertiary road",
                    5 => "Local road",
                    6 => "Living street",
                    7 => "Service road",
                    8 => "Track",
                    9 => "Unclassified road",
                    10 => "Motorway link",
                    11 => "Trunk link",
                    12 => "Primary link",
                    _ => "Road",
                };
                let dist = seg.dist_m;
                if dist < 100.0 {
                    format!("{} ({}m)", class_label, dist as u32)
                } else if dist < 1000.0 {
                    format!("{} ({}m)", class_label, (dist / 10.0).round() as u32 * 10)
                } else {
                    format!("{} ({:.1}km)", class_label, dist / 1000.0)
                }
            };
            RoadAccum {
                class_name,
                display_name,
                first_osm_id: seg.osm_id,
                min_dist: f64::MAX,
                min_d_slant: 0.0,
                min_ground_g: 0.5,
                closest_cp_lat: seg.cp_lat,
                closest_cp_lon: seg.cp_lon,
                closest_src_height: src_alt,
                dominant_energy: 0.0,
                dominant_segment_idx: 0,
                dominant_distance_m: 0.0,
                dominant_aadt_light_raw: 0,
                dominant_aadt_medium_raw: 0,
                dominant_aadt_heavy_raw: 0,
                dominant_aadt_moto_raw: 0,
                dominant_aadt_light_nominal: 0.0,
                dominant_aadt_medium_nominal: 0.0,
                dominant_aadt_heavy_nominal: 0.0,
                dominant_aadt_moto_nominal: 0.0,
                dominant_aadt_light_effective: 0.0,
                dominant_aadt_medium_effective: 0.0,
                dominant_aadt_heavy_effective: 0.0,
                dominant_aadt_moto_effective: 0.0,
                dominant_traffic_source: "default_by_class",
                dominant_source_id: 0,
                dominant_speed_posted: 0,
                dominant_speed_used: 0.0,
                dominant_speed_source: "default_by_class",
                dominant_surface_type: 0,
                dominant_surface_corr_db: 0.0,
                dominant_lanes: 0,
                dominant_oneway: false,
                segment_count: 0,
                total_length_m: 0.0,
                bridge_count: 0,
                speed_min: f64::MAX,
                speed_max: 0.0,
                oneway_segment_count: 0,
                twoway_segment_count: 0,
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
                dominant_trace_idx: None,
            }
        });
        // Aggregation across all grouped segments (independent of closest check)
        acc.segment_count += 1;
        acc.total_length_m += seg.length_m as f64;
        if seg.bridge {
            acc.bridge_count += 1;
        }
        // Group-level obstacle histogram — probed in pass 2 (pure raster
        // read). Popup shows "N of M segments had obstacles on path" from it.
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
        // Aggregate stats across all segments
        if speed < acc.speed_min {
            acc.speed_min = speed;
        }
        if speed > acc.speed_max {
            acc.speed_max = speed;
        }
        if seg.oneway {
            acc.oneway_segment_count += 1;
        } else {
            acc.twoway_segment_count += 1;
        }
        // Closest segment — for distance display, baseline, and path-effect context
        if seg.dist_m < acc.min_dist {
            acc.min_dist = seg.dist_m;
            acc.min_d_slant = d_slant;
            acc.min_ground_g = ground_g;
            acc.closest_cp_lat = seg.cp_lat;
            acc.closest_cp_lon = seg.cp_lon;
            acc.closest_src_height = src_alt;
        }
        // Popup trace: push pass 2's prebuilt SegmentTrace, in segment order —
        // the same order (and therefore the same top-K tie-breaking downstream)
        // the sequential kernel produced.
        let pushed_trace_idx: Option<usize> = if let Some(t) = traces.as_deref_mut() {
            let trace = out
                .trace
                .take()
                .expect("pass 2 builds a trace for every kept segment when collecting");
            let idx = t.segments.len();
            t.segments.push(trace);
            Some(idx)
        } else {
            None
        };

        // Dominant segment — highest received energy, drives the popup metadata
        let seg_received_energy: f64 = seg_variants[0].full_energy;
        if seg_received_energy > acc.dominant_energy {
            if let Some(idx) = pushed_trace_idx {
                acc.dominant_trace_idx = Some(idx);
            }
            acc.dominant_energy = seg_received_energy;
            acc.dominant_segment_idx = seg.segment_idx;
            acc.dominant_distance_m = seg.dist_m;
            acc.dominant_aadt_light_raw = seg.aadt_light;
            acc.dominant_aadt_medium_raw = seg.aadt_medium;
            acc.dominant_aadt_heavy_raw = seg.aadt_heavy;
            acc.dominant_aadt_moto_raw = seg.aadt_moto;
            let provenance = sources::provenance_of(seg.source_id);
            let (nom_l, nom_m, nom_h, nom_x) = normalize::nominal_road_aadt(
                seg.road_class,
                provenance,
                seg.aadt_light,
                seg.aadt_medium,
                seg.aadt_heavy,
                seg.aadt_moto,
                admin,
            );
            acc.dominant_aadt_light_nominal = nom_l;
            acc.dominant_aadt_medium_nominal = nom_m;
            acc.dominant_aadt_heavy_nominal = nom_h;
            acc.dominant_aadt_moto_nominal = nom_x;
            acc.dominant_aadt_light_effective = light;
            acc.dominant_aadt_medium_effective = medium;
            acc.dominant_aadt_heavy_effective = heavy;
            acc.dominant_aadt_moto_effective = moto;
            acc.dominant_traffic_source = if provenance.has_data() && seg.aadt_light > 0 {
                provenance.legacy_traffic_source_str()
            } else {
                "default_by_class"
            };
            acc.dominant_source_id = seg.source_id;
            acc.dominant_speed_posted = seg.speed_limit;
            acc.dominant_speed_used = speed;
            // Roundabout labels the source only when the cap actually REDUCED the speed —
            // an untagged roundabout whose default is already ≤30 falls through to the
            // real provenance chain below (/gg Codex: it read "osm_posted" with no tag).
            acc.dominant_speed_source = if seg.junction == 1 && speed < base_speed {
                "roundabout_cap"
            } else if seg.speed_limit == normalize::SPEED_LIMIT_DERESTRICTED {
                // maxspeed=none: no posted number exists; emission models
                // DERESTRICTED_SPEED_KMH. UI renders "no limit".
                "derestricted"
            } else if seg.speed_limit > 0 {
                "osm_posted"
            } else if seg.speed_taper > 0 {
                // R7 taper graded effective speed — NOT a legal limit; the
                // dedicated label keeps the popup honest ("osm_posted" here
                // would claim a sign that does not exist).
                "graded_transition"
            } else if crate::defaults::resolve_speed_default(seg.road_class, admin, seg.built_up)
                .is_some()
            {
                // Untagged, resolved from the country's legal implicit limit (task #15).
                "country_legal_default"
            } else {
                "default_by_class"
            };
            acc.dominant_surface_type = seg.surface_type;
            acc.dominant_surface_corr_db = surf_corr;
            acc.dominant_lanes = seg.lanes;
            acc.dominant_oneway = seg.oneway;
        }
        // Each segment is an independent 2-point LineString; no osm_id regrouping needed.
        acc.line_coords
            .push([[seg.start_lon, seg.start_lat], [seg.end_lon, seg.end_lat]]);
    }

    // Mark the dominant-of-group traces now that all segments are processed.
    if let Some(t) = traces {
        for acc in roads_by_key.values() {
            if let Some(idx) = acc.dominant_trace_idx {
                if let Some(tr) = t.segments.get_mut(idx) {
                    tr.is_dominant_of_group = true;
                }
            }
        }
    }

    // Emit grouped contributors
    let mut contributors = Vec::new();
    // Ascending group key, not HashMap order — the contributor sequence
    // is summed downstream and its JSON order is part of the popup
    // reference output. See `crate::compute::key_sorted`.
    for (_, acc) in crate::compute::key_sorted(&roads_by_key) {
        // Full energy from variants (includes all path effects per-band)
        let ld = PropagationVariants::to_db(acc.variants[0].full_energy);
        let le = PropagationVariants::to_db(acc.variants[1].full_energy);
        let ln = PropagationVariants::to_db(acc.variants[2].full_energy);
        let road_periods = periods::periods(ld, le, ln);

        // Free-field energy (for comparison)
        let ld_free = PropagationVariants::to_db(acc.variants[0].free_field_energy);
        let le_free = PropagationVariants::to_db(acc.variants[1].free_field_energy);
        let ln_free = PropagationVariants::to_db(acc.variants[2].free_field_energy);
        let free_periods = periods::periods(ld_free, le_free, ln_free);

        let emission_db = 10.0 * acc.emission_energy.max(1e-12).log10();
        let geometry = if !acc.line_coords.is_empty() {
            Some(serde_json::json!({"type": "MultiLineString", "coordinates": acc.line_coords}))
        } else {
            None
        };

        let impacts = PropagationVariants::impact_deltas(&acc.variants, road_periods.lden_db);

        let (nearest_terrain, nearest_screening, nearest_veg) = compute_path_effects(
            rasters,
            barriers,
            obstacles,
            acc.closest_cp_lat,
            acc.closest_cp_lon,
            acc.closest_src_height,
            receiver,
            acc.min_dist,
            0.0,
        );

        let road_meta = RoadMetadata {
            aadt_light_raw: acc.dominant_aadt_light_raw,
            aadt_medium_raw: acc.dominant_aadt_medium_raw,
            aadt_heavy_raw: acc.dominant_aadt_heavy_raw,
            aadt_moto_raw: acc.dominant_aadt_moto_raw,
            traffic_source: acc.dominant_traffic_source,
            dominant_source_id: acc.dominant_source_id,
            // Derestricted has no posted number — None keeps the popup from
            // rendering the 255 sentinel as "255 km/h" (/gg W4).
            speed_posted_kmh: (acc.dominant_speed_posted != normalize::SPEED_LIMIT_DERESTRICTED)
                .then_some(acc.dominant_speed_posted),
            aadt_light_nominal: acc.dominant_aadt_light_nominal,
            aadt_medium_nominal: acc.dominant_aadt_medium_nominal,
            aadt_heavy_nominal: acc.dominant_aadt_heavy_nominal,
            aadt_moto_nominal: acc.dominant_aadt_moto_nominal,
            aadt_light_effective: acc.dominant_aadt_light_effective,
            aadt_medium_effective: acc.dominant_aadt_medium_effective,
            aadt_heavy_effective: acc.dominant_aadt_heavy_effective,
            aadt_moto_effective: acc.dominant_aadt_moto_effective,
            speed_kmh: acc.dominant_speed_used,
            speed_source: acc.dominant_speed_source,
            road_class: acc.class_name,
            surface: surface_name(acc.dominant_surface_type),
            surface_corr_db: acc.dominant_surface_corr_db,
            lanes: acc.dominant_lanes,
            oneway: acc.dominant_oneway,
            dominant_segment_idx: acc.dominant_segment_idx,
            dominant_distance_m: acc.dominant_distance_m,
            closest_distance_m: acc.min_dist,
            speed_min_kmh: acc.speed_min,
            speed_max_kmh: acc.speed_max,
            oneway_segment_count: acc.oneway_segment_count,
            twoway_segment_count: acc.twoway_segment_count,
            segment_count: acc.segment_count,
            total_length_m: acc.total_length_m,
            bridge_count: acc.bridge_count,
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
            osm_id: Some(acc.first_osm_id),
            geometry,
            source_type: LayerKind::Road,
            name: acc.display_name.clone(),
            subtype: acc.class_name.to_string(),
            distance_m: acc.min_dist,
            periods: road_periods,
            periods_free: free_periods,
            emission_db,
            baseline: iso9613::compute_baseline(
                acc.min_d_slant,
                SourceGeometry::Line,
                acc.min_ground_g,
            ),
            terrain: nearest_terrain,
            screening: nearest_screening,
            vegetation: nearest_veg,
            terrain_impact_db: round1(impacts.terrain),
            screening_impact_db: round1(impacts.screening),
            vegetation_impact_db: round1(impacts.vegetation),
            atmospheric_impact_db: round1(impacts.atmospheric),
            ground_impact_db: round1(impacts.ground),
            received_bands: std::array::from_fn(|j| {
                10.0 * acc.variants[0].band_energy[j].max(1e-30).log10()
            }),
            metadata: Some(SourceMetadata::Road(road_meta)),
        });
    }

    // Total must come from all grouped energies, not from display-filtered contributors.
    let mut total_energy = [0.0f64; 3];
    // f64 addition is not associative: ascending key order, not HashMap
    // order, or this total moves ±1 ULP per query.
    for (_, acc) in crate::compute::key_sorted(&roads_by_key) {
        total_energy[0] += acc.variants[0].full_energy;
        total_energy[1] += acc.variants[1].full_energy;
        total_energy[2] += acc.variants[2].full_energy;
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

    /// TH admin (M6.3 measured DRR secondary arm 899.7/62.4/44.2/912.1 vs
    /// WORLD 2640/120/180/60 — the override is unambiguous on the fixture
    /// below).
    const TH: Admin = Admin {
        continent: Continent::Asia,
        country_iso: *b"TH",
        city_id: 0,
    };

    /// One unenriched secondary (class 3) segment 200 m from the receiver:
    /// tagged speed 50, so the admin affects ONLY the AADT cascade. Tests
    /// never init the process-wide admin table, so the receiver admin is
    /// UNKNOWN → WORLD — exactly the "oceanic receiver" shape of gate (a).
    fn secondary_segment() -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            segment_idx: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            end_lat: 50.0,
            end_lon: 14.003,
            length_m: 220.0,
            road_class: 3,
            speed_limit: 50,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 0,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0,
            source_id: 0,
            dist_m: 200.0,
            cp_lat: 50.0,
            cp_lon: 14.0015,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
            access: 0,
            junction: 0,
            built_up: 0,
        }
    }

    /// 200 m due north of the segment — the geometry has to AGREE with the
    /// fixture's `dist_m`/`cp`/`fraction`, since the finite-line correction
    /// reads the segment's real perpendicular distance (fix-pack C).
    fn receiver() -> Receiver {
        Receiver::new(50.001809, 14.0015, 200.0)
    }

    fn one_road_meta(roads: &[RoadSegment]) -> RoadMetadata {
        let (_periods, contribs) = compute_roads(
            &receiver(),
            roads,
            &[],
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        );
        assert_eq!(contribs.len(), 1, "single segment → single contributor");
        match contribs.into_iter().next().unwrap().metadata.unwrap() {
            SourceMetadata::Road(m) => m,
            other => panic!("expected road metadata, got {other:?}"),
        }
    }

    /// Gate (a) popup: a row whose baked admin is TH gets TH defaults even
    /// though the receiver resolves UNKNOWN — the segment's own country wins.
    #[test]
    fn baked_row_admin_wins_over_receiver() {
        let seg = secondary_segment();
        let world = one_road_meta(std::slice::from_ref(&seg));
        defaults::set_road_row_admins(Some(vec![Some(TH)]));
        let baked = one_road_meta(std::slice::from_ref(&seg));
        defaults::set_road_row_admins(None);
        assert_eq!(
            world.aadt_light_effective, 2640.0,
            "receiver UNKNOWN → WORLD"
        );
        assert_eq!(
            baked.aadt_light_effective, 3720.0,
            "baked TH → the hand-tuned TH rural arm (measured arm parked, /gg M6 Codex)"
        );
        // The popup's nominal (pre-factor) display surface follows the same
        // row admin (nominal_road_aadt call inside the segment loop).
        assert_eq!(baked.aadt_light_nominal, 3720.0);
        assert_eq!(world.aadt_light_nominal, 2640.0);
    }

    /// Gate (b) popup: a channel of `None` entries ≡ no channel — the
    /// receiver path is bit-identical to the pre-bake kernel.
    #[test]
    fn none_channel_is_receiver_path_bit_identical() {
        let roads = vec![secondary_segment()];
        let plain = compute_roads(
            &receiver(),
            &roads,
            &[],
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        )
        .0;
        defaults::set_road_row_admins(Some(vec![None]));
        let channeled = compute_roads(
            &receiver(),
            &roads,
            &[],
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        )
        .0;
        defaults::set_road_row_admins(None);
        assert_eq!(plain.ld_db, channeled.ld_db);
        assert_eq!(plain.le_db, channeled.le_db);
        assert_eq!(plain.ln_db, channeled.ln_db);
        assert_eq!(plain.lden_db, channeled.lden_db);
    }

    /// Stripe regression (fix-pack Fix 1): a 30 m building straddling the cp
    /// ray must NOT screen the whole 214 m segment. Its shadow covers ~22 % of
    /// the fan the receiver sees, so the loss is ~1 dB — the cp-ray verdict
    /// alone applied the full ~15 dB diffraction to every metre of the
    /// segment, which is the constant-width stripe behind buildings.
    #[test]
    fn box_on_the_cp_ray_screens_only_its_angular_share() {
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
        let lat_of = |north_m: f64| 50.0 + north_m / crate::constants::M_PER_DEG_LAT;
        let lon_of =
            |east_m: f64| 14.0 + east_m / crate::constants::m_per_deg_lon(50.0_f64.to_radians());
        // 30 m wide × 10 m deep × 8 m tall, 50 m north of the segment, centred
        // on the receiver's perpendicular foot.
        let mut b = ObstacleIndex::builder(50.0, 14.0);
        b.add_ring(
            &[
                (lat_of(50.0), lon_of(92.0)),
                (lat_of(50.0), lon_of(122.0)),
                (lat_of(60.0), lon_of(122.0)),
                (lat_of(60.0), lon_of(92.0)),
            ],
            8.0,
            ObstacleKind::Building,
            0,
        );
        let obstacles = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };

        let roads = vec![secondary_segment()];
        let clear = compute_roads(
            &receiver(),
            &roads,
            &[],
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        )
        .0
        .lden_db;
        let screened = compute_roads(&receiver(), &roads, &[], &obstacles, &FlatRasters, None)
            .0
            .lden_db;
        let loss = clear - screened;
        assert!(
            loss > 0.2,
            "the box must screen the arc it covers, got {loss:.2} dB"
        );
        assert!(
            loss < 3.0,
            "…but not the whole segment: {loss:.2} dB (cp-ray verdict was ~15 dB)"
        );
    }

    /// Gate (c) popup: a baked `\0\0` row is WORLD defaults — `Some(UNKNOWN)`
    /// never falls back to the receiver admin (indistinguishable here only
    /// because the test receiver is also UNKNOWN; the no-fallback contrast
    /// with a KNOWN region is pinned at the loader level).
    #[test]
    fn baked_zero_is_world_arm() {
        let seg = secondary_segment();
        defaults::set_road_row_admins(Some(vec![Some(Admin::UNKNOWN)]));
        let baked0 = one_road_meta(std::slice::from_ref(&seg));
        defaults::set_road_row_admins(None);
        assert_eq!(baked0.aadt_light_effective, 2640.0);
    }

    /// A dense-ish scene for the pool-size gate: a fan of secondary segments
    /// at varying ranges and offsets (several wide enough to arc-screen), a
    /// village of obstacle boxes, and one noise wall — every branch of the
    /// three-pass kernel (cp verdict, arc snapshot, grouping, dominant,
    /// traces) gets traffic.
    fn pool_gate_scene() -> (
        Vec<RoadSegment>,
        crate::propagation::obstacle_index::ObstacleSet,
    ) {
        use crate::constants::{m_per_deg_lon, M_PER_DEG_LAT};
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
        let mut segs = Vec::new();
        for k in 0..24 {
            let mut seg = secondary_segment();
            seg.osm_id = 100 + (k as i64 % 5); // several groups, shared osm_ids
            seg.segment_idx = k as i16;
            let north_m = 60.0 + 55.0 * k as f64;
            let east_m = -140.0 + 15.0 * k as f64;
            let dlat = north_m / M_PER_DEG_LAT;
            let dlon = east_m / m_per_deg_lon(50.0_f64.to_radians());
            seg.start_lat += dlat;
            seg.end_lat += dlat;
            seg.start_lon += dlon;
            seg.end_lon += dlon;
            seg.cp_lat += dlat;
            seg.cp_lon += dlon;
            seg.dist_m = north_m.hypot(east_m).max(20.0);
            if k % 3 == 0 {
                seg.name = format!("Street {}", k % 4);
            }
            segs.push(seg);
        }
        let lat_of = |north_m: f64| 50.0 + north_m / M_PER_DEG_LAT;
        let lon_of = |east_m: f64| 14.0 + east_m / m_per_deg_lon(50.0_f64.to_radians());
        let mut b = ObstacleIndex::builder(50.0, 14.0);
        for r in 0..4 {
            let y = 40.0 + 130.0 * r as f64;
            for c in -2i32..=2 {
                let x = c as f64 * 90.0;
                b.add_ring(
                    &[
                        (lat_of(y), lon_of(x - 14.0)),
                        (lat_of(y), lon_of(x + 14.0)),
                        (lat_of(y + 12.0), lon_of(x + 14.0)),
                        (lat_of(y + 12.0), lon_of(x - 14.0)),
                    ],
                    3.0 + 2.5 * r as f32,
                    ObstacleKind::Building,
                    (r * 8 + (c + 2) as usize) as u32,
                );
            }
        }
        let obstacles = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };
        (segs, obstacles)
    }

    /// THE PARALLELISM GATE: the rayon pool size (i.e. how work is split and
    /// interleaved across threads) must never move a bit — periods,
    /// contributors and traces all byte-stable between a 1-thread and a
    /// multi-thread run. The popup is the acoustic reference; f64 accumulation
    /// order is part of its contract, and the three-pass kernel keeps that
    /// order by folding pass-2 results in segment order.
    #[test]
    fn pool_size_never_changes_the_bits() {
        let (segs, obstacles) = pool_gate_scene();
        let wall = [Barrier {
            osm_id: 9,
            segment_idx: 0,
            height_m: 4.0,
            start_lat: 50.000_36,
            start_lon: 13.998,
            end_lat: 50.000_36,
            end_lon: 14.003,
            dist_m: 40.0,
        }];
        // FULL-output comparison: every period bit, the complete serialized
        // contributor list (all fields, exactly what the popup wire carries)
        // and the complete serialized trace list — in both trace modes, since
        // the no-trace path reuses the worker profile across segments.
        let run = |threads: usize, with_traces: bool| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("test pool")
                .install(|| {
                    let mut traces = TraceCollector::new();
                    let (periods, contribs) = compute_roads(
                        &receiver(),
                        &segs,
                        &wall,
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
            let (bits8, contribs8, traces8) = run(7, with_traces);
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
