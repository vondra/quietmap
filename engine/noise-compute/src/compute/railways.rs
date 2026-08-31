//! Railway compute kernel — groups rail segments by osm_id, emits per-period
//! power and propagates to the receiver (CNOSSOS rail). Shared by popup + heatmap.
use crate::*;

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

/// Compute railway noise — grouped by osm_id with geometry.
///
/// Same three-pass parallel structure as `compute_roads` (see its docstring):
/// sequential gates + skyline growth chain, parallel per-segment evaluation
/// against frozen [`SkylineSnapshot`]s, sequential accumulation in segment
/// order. Bit-identical to the sequential loop by construction.
pub(crate) fn compute_railways(
    receiver: &Receiver,
    railways: &[RailSegment],
    barriers: &[Barrier],
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
        usage_u8: u8,
        first_osm_id: i64,
        min_dist: f64,
        min_d_slant: f64,
        min_ground_g: f64,
        cp_lat: f64,
        cp_lon: f64,
        src_height: f64,
        // Closest-segment metadata (for popup)
        closest_trains_passenger_raw: f64,
        closest_trains_freight_raw: f64,
        closest_trains_passenger_effective: f64,
        closest_trains_freight_effective: f64,
        closest_trains_passenger_source: &'static str,
        closest_trains_freight_source: &'static str,
        closest_source_id: u16,
        closest_maxspeed_posted: u16,
        closest_speed_used: f64,
        closest_speed_source: &'static str,
        closest_service: bool,
        closest_highspeed: bool,
        closest_parallel_divisor: u8,
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
        has_bridge: bool,
        dominant_energy: f64,
        dominant_trace_idx: Option<usize>,
    }
    let mut rails_by_key: HashMap<(String, u8), RailAccum> = HashMap::new();

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
            barriers,
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
                        barriers,
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
                            source_height_m: SOURCE_HEIGHT_RAIL,
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

    // ── Pass 3: accumulation, in segment order (sequential) ──
    for ((seg_i, p), mut out) in pre.iter().zip(outs) {
        let seg = &railways[*seg_i];
        let (rail_type, speed, q_pax, q_frt) = (p.rail_type, p.speed, p.q_pax, p.q_frt);
        let (src_alt, d_slant) = (p.src_alt, p.d_slant);
        let (seg_variants, ground_g) = (out.seg_variants, out.ground_g);

        // Group by (ref, name, type). When both ref+name empty, group by osm_id
        // (each OSM way is a logical track segment — avoids merging entire city tram network).
        let key_str = if !seg.rail_ref.is_empty() || !seg.name.is_empty() {
            format!("{}|{}", seg.rail_ref, seg.name)
        } else {
            format!("osm:{}", seg.osm_id)
        };
        let key = (key_str, seg.rail_type);
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
            usage_u8: seg.usage,
            first_osm_id: seg.osm_id,
            min_dist: f64::MAX,
            min_d_slant: 0.0,
            min_ground_g: 0.5,
            cp_lat: seg.cp_lat,
            cp_lon: seg.cp_lon,
            src_height: src_alt,
            closest_trains_passenger_raw: 0.0,
            closest_trains_freight_raw: 0.0,
            closest_trains_passenger_effective: 0.0,
            closest_trains_freight_effective: 0.0,
            closest_trains_passenger_source: "default_by_type",
            closest_trains_freight_source: "default_by_type",
            closest_source_id: 0,
            closest_maxspeed_posted: 0,
            closest_speed_used: 0.0,
            closest_speed_source: "type_default",
            closest_service: false,
            closest_highspeed: false,
            closest_parallel_divisor: 1,
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
            has_bridge: false,
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
        if seg.bridge {
            acc.has_bridge = true;
        }
        if seg.dist_m < acc.min_dist {
            acc.min_dist = seg.dist_m;
            acc.min_d_slant = d_slant;
            acc.min_ground_g = ground_g;
            acc.cp_lat = seg.cp_lat;
            acc.cp_lon = seg.cp_lon;
            acc.src_height = src_alt;
            // Closest-segment metadata for popup
            acc.closest_trains_passenger_raw = seg.trains_passenger;
            acc.closest_trains_freight_raw = seg.trains_freight;
            acc.closest_trains_passenger_effective = q_pax;
            acc.closest_trains_freight_effective = q_frt;
            acc.closest_trains_passenger_source = match seg.trains_passenger_source {
                0 => "arrow",
                _ => "default_by_type",
            };
            acc.closest_trains_freight_source = match seg.trains_freight_source {
                0 => "arrow",
                _ => "default_by_type",
            };
            acc.closest_source_id = seg.source_id;
            acc.closest_maxspeed_posted = seg.maxspeed;
            acc.closest_speed_used = speed;
            acc.closest_speed_source = match seg.speed_source {
                0 => "osm_maxspeed",
                1 => "highspeed_default",
                _ => "type_default",
            };
            acc.closest_service = seg.service;
            acc.closest_highspeed = seg.highspeed;
            acc.closest_parallel_divisor = seg.parallel_divisor.max(1);
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
            barriers,
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
            usage: rail_usage_name(acc.usage_u8),
            service: acc.dominant_service,
            highspeed: acc.dominant_highspeed,
            parallel_divisor: acc.dominant_parallel_divisor,
            dominant_segment_idx: acc.dominant_segment_idx,
            dominant_distance_m: acc.dominant_distance_m,
            closest_distance_m: acc.min_dist,
            bridge: acc.has_bridge,
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
            osm_id: Some(acc.first_osm_id),
            geometry,
            source_type: LayerKind::Railway,
            name: if acc.name.is_empty() {
                String::new()
            } else {
                acc.name.clone()
            },
            subtype: {
                let base = format!("{:?}", acc.rail_type);
                if acc.has_bridge {
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

    let mut total_energy = [0.0f64; 3];
    // f64 addition is not associative: ascending key order, not HashMap
    // order, or this total moves ±1 ULP per query.
    for (_, acc) in crate::compute::key_sorted(&rails_by_key) {
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

    fn periods_for(segs: &[RailSegment]) -> NoisePeriods {
        compute_railways(
            &receiver(),
            segs,
            &[],
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        )
        .0
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

    /// THE PARALLELISM GATE (rail twin of roads'
    /// `pool_size_never_changes_the_bits`): the rayon pool size must never
    /// move a bit — the three-pass kernel folds pass-2 results in segment
    /// order, so periods, contributors and traces are byte-stable.
    #[test]
    fn pool_size_never_changes_the_bits() {
        use crate::constants::{m_per_deg_lon, M_PER_DEG_LAT};
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
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
                        &[],
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
