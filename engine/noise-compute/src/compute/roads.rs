//! Road compute kernel — groups road segments by (ref, name, class), emits
//! per-period power and propagates to the receiver (CNOSSOS road). Shared by popup + heatmap.
use crate::*;

/// Compute road noise: emission per period → propagation → Lden per segment.
///
/// Two passes, so the per-piece work runs on every core while the answer stays
/// bit-identical run to run (the popup is the acoustic reference — f64
/// accumulation order is part of the contract):
///
/// 1. **Evaluation** (parallel, rayon): gates, the line quadrature of each piece
///    (every node on its own ray), per-period propagation, the ref-inference
///    scan, the obstacle histogram probe, the popup trace. Pure per segment —
///    per-thread scratch, no shared mutable state — and the ordered `collect`
///    keeps results in segment order.
/// 2. **Accumulation** (sequential): grouping, dominant selection, trace push
///    order, energy sums, in segment order.
pub(crate) fn compute_roads(
    receiver: &Receiver,
    roads: &[RoadSegment],
    obstacles: &crate::propagation::obstacle_index::ObstacleSet,
    rasters: &dyn RasterSampler,
    mut traces: Option<&mut TraceCollector>,
) -> (NoisePeriods, Vec<Contributor>) {
    use crate::compute::line_piece::{evaluate_line_piece, LinePiece, LinePieceScratch};
    use crate::propagation::ray_transfer::{received_variants, RayReceiver, RaySource, SourceGround};
    use crate::propagation::relevance_bound::{SourceSpread, LINE_REACH_CEILING_M};
    use rayon::prelude::*;

    let timing_on = std::env::var("POPUP_TIMING").as_deref() == Ok("1");
    let t_road_start = std::time::Instant::now();
    let reflection = rasters.building_enclosure(receiver.lat, receiver.lon);
    let rcv_alt = receiver.altitude_m();
    let ray_receiver = RayReceiver {
        lat: receiver.lat,
        lon: receiver.lon,
        altitude_m: rcv_alt,
    };
    let weather = crate::propagation::meteorology::Meteorology::defaults();
    let bound = crate::propagation::relevance_bound::surface_relevance_bound(&weather);

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
        closest_source: RaySource,
        // Dominant-segment metadata (highest received energy — what drives the result)
        dominant_energy: f64,
        dominant_segment_idx: i16,
        dominant_distance_m: f64,
        /// Prepared traffic of the dominant segment (counts + estimated bitmask).
        dominant_traffic: normalize::RoadTraffic,
        /// Observed timing attribution of the dominant segment (display).
        dominant_time_profile_attribution: Option<normalize::RoadTimeProfileAttribution>,
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
        profiled_segment_count: u32,
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
    // For unnamed roads (ref="" && name=""): group per osm_id to avoid merging
    // all unnamed residential streets into one mega-contributor (unnamed rail
    // tracks, by contrast, merge per type — see compute/railways.rs).
    let mut roads_by_key: HashMap<(String, String, u8), RoadAccum> = HashMap::new();

    // SquareCountryCity resolved once per compute_roads call — receiver position is
    // constant across segments. Uses the process-wide square-country-city cache
    // (see square_country_city::set_square_country_city_prepared_directory at source-reader init).
    // Falls back to SquareCountryCity::UNKNOWN → WORLD_DEFAULT when uninitialised.
    // M4: a row's own baked SquareCountryCity overrides this per segment below.
    let receiver_square_country_city =
        crate::square_country_city::square_country_city_for_latlng(receiver.lat, receiver.lon);

    struct RoadPre {
        norm: normalize::NormalizedRoad,
        square_country_city: crate::square_country_city::SquareCountryCity,
        d_slant: f64,
    }
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
    // ── Pass 1: per-segment evaluation (parallel, bit-deterministic) ──
    let kept: Vec<Option<(RoadPre, RoadSegOut)>> = roads
        .par_iter()
        .map_init(LinePieceScratch::default, |scratch, seg| {
            // The row's own baked SquareCountryCity (plan M4) when its batch carried one,
            // else the receiver SquareCountryCity (pre-bake behaviour, unchanged).
            let square_country_city = seg
                .square_country_city
                .unwrap_or(receiver_square_country_city);
            let norm = normalize::normalize_road_segment(seg, square_country_city)?;
            let period_emissions = norm.period_emissions_db();
            // The row's reach and the all-period pair gate (#31: a night-only road is never
            // dropped by a day gate), both from the one relevance bound.
            if seg.dist_m > LINE_REACH_CEILING_M
                || !bound.within_reach(&period_emissions, SourceSpread::Line, seg.dist_m)
                || bound.pair_is_inaudible(&period_emissions, SourceSpread::Line, seg.dist_m)
            {
                return None;
            }
            let src_alt = rasters.elevation(seg.cp_lat, seg.cp_lon) + norm.source_height_m;
            let d_slant = geo::slant_dist(seg.dist_m, src_alt, rcv_alt);
            let day_weights: [f64; NUM_BANDS] = std::array::from_fn(|b| {
                10f64.powf((period_emissions[0][b] + A_WEIGHTING[b]) / 10.0)
            });
            let piece = evaluate_line_piece(
                &ray_receiver,
                &LinePiece {
                    start_lat: seg.start_lat,
                    start_lon: seg.start_lon,
                    end_lat: seg.end_lat,
                    end_lon: seg.end_lon,
                    source_height_m: norm.source_height_m,
                    source_ground_factor: normalize::road::ROAD_SOURCE_GROUND_FACTOR,
                    platform_half_width_m: normalize::road::road_platform_half_width_m(seg.lanes),
                    directivity: crate::propagation::line_quadrature::LineDirectivity::Omnidirectional,
                },
                seg.cp_lat,
                seg.cp_lon,
                obstacles,
                rasters,
                &weather,
                scratch,
                collect_traces.then_some(&day_weights),
            )?;
            let seg_variants: [PropagationVariants; 3] = std::array::from_fn(|pi| {
                received_variants(&piece.periods[pi], &period_emissions[pi], reflection)
            });
            // Band energy sum (`j` indexes `emission`); f64 accumulation
            // order is part of popup byte parity — kept as an index loop.
            let mut day_emission_energy = 0.0f64;
            #[allow(clippy::needless_range_loop)]
            for j in 0..NUM_BANDS {
                day_emission_energy += crate::propagation::iso9613::fast_exp_f64(
                    period_emissions[0][j] * std::f64::consts::LN_10 * 0.1,
                );
            }
            let ground_g = piece.loudest_node.as_ref().map_or(0.5, |node| node.ground_factor);

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
            let seg_max_bh =
                obstacles.max_height_crossed(seg.cp_lat, seg.cp_lon, receiver.lat, receiver.lon);

            let trace = match (collect_traces, piece.loudest_node) {
                (true, Some(node)) => Some(build_road_segment_trace(BuildRoadTrace {
                    seg,
                    class_name: norm.class_name,
                    rcv_alt,
                    d_slant,
                    reflection_boost_db: reflection,
                    traffic: seg.traffic,
                    speed_kmh: norm.speed_kmh,
                    surf_corr: norm.surf_corr_db,
                    node,
                    fan: piece.fan,
                    seg_variants,
                    lw_bands: period_emissions,
                })),
                _ => None,
            };
            Some((
                RoadPre {
                    norm,
                    square_country_city,
                    d_slant,
                },
                RoadSegOut {
                    seg_variants,
                    day_emission_energy,
                    ground_g,
                    seg_max_bh,
                    effective_ref,
                    trace,
                },
            ))
        })
        .collect();
    let (pre, outs): (Vec<(usize, RoadPre)>, Vec<RoadSegOut>) = kept
        .into_iter()
        .enumerate()
        .filter_map(|(seg_i, kept)| kept.map(|(p, out)| ((seg_i, p), out)))
        .unzip();
    if timing_on {
        eprintln!(
            "popup-stage road evaluation={:.0}ms kept={}",
            t_road_start.elapsed().as_secs_f64() * 1000.0,
            pre.len()
        );
    }

    // ── Pass 2: accumulation, in segment order (sequential) ──
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
        let (square_country_city, d_slant) = (p.square_country_city, p.d_slant);
        let (seg_variants, ground_g) = (out.seg_variants, out.ground_g);
        let effective_ref = std::mem::take(&mut out.effective_ref);

        // For unnamed roads: group per osm_id, not catch-all
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
                closest_source: RaySource {
                    lat: seg.cp_lat,
                    lon: seg.cp_lon,
                    height_m: p.norm.source_height_m,
                    ground: SourceGround::Fixed(normalize::road::ROAD_SOURCE_GROUND_FACTOR),
                    platform_half_width_m: normalize::road::road_platform_half_width_m(seg.lanes),
                    exclusion_radius_m: 0.0,
                },
                dominant_energy: 0.0,
                dominant_segment_idx: 0,
                dominant_distance_m: 0.0,
                dominant_traffic: normalize::RoadTraffic::default(),
                dominant_time_profile_attribution: None,
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
                profiled_segment_count: 0,
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
        if seg.traffic.time_profile.is_some() {
            acc.profiled_segment_count += 1;
        }
        if seg.bridge {
            acc.bridge_count += 1;
        }
        // Group-level obstacle histogram — probed in pass 2 by
        // `max_height_crossed` (exact footprint crossings). Popup shows
        // "N of M segments had obstacles on path" from it.
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
            acc.closest_source = RaySource {
                lat: seg.cp_lat,
                lon: seg.cp_lon,
                height_m: p.norm.source_height_m,
                ground: SourceGround::Fixed(normalize::road::ROAD_SOURCE_GROUND_FACTOR),
                platform_half_width_m: normalize::road::road_platform_half_width_m(seg.lanes),
                exclusion_radius_m: 0.0,
            };
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
            acc.dominant_traffic = seg.traffic;
            acc.dominant_time_profile_attribution = seg.time_profile_attribution.clone();
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
            } else if crate::defaults::resolve_speed_default(
                seg.road_class,
                square_country_city,
                seg.built_up,
            )
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

        let emission_db = PropagationVariants::to_db(acc.emission_energy);
        let geometry = if !acc.line_coords.is_empty() {
            Some(serde_json::json!({"type": "MultiLineString", "coordinates": acc.line_coords}))
        } else {
            None
        };

        let impacts = PropagationVariants::impact_deltas(&acc.variants, road_periods.lden_db);

        let (nearest_terrain, nearest_screening, nearest_veg) =
            nearest_path_breakdown(rasters, obstacles, &acc.closest_source, receiver, &weather);

        let road_meta = RoadMetadata {
            aadt_light: acc.dominant_traffic.light,
            aadt_medium: acc.dominant_traffic.medium,
            aadt_heavy: acc.dominant_traffic.heavy,
            aadt_moto: acc.dominant_traffic.moto,
            traffic_estimated: acc.dominant_traffic.estimated,
            cross_section_aadt: acc.dominant_traffic.cross_section_aadt,
            dominant_source_id: acc.dominant_source_id,
            // Derestricted has no posted number — None keeps the popup from
            // rendering the 255 sentinel as "255 km/h" (/gg W4).
            speed_posted_kmh: (acc.dominant_speed_posted != normalize::SPEED_LIMIT_DERESTRICTED)
                .then_some(acc.dominant_speed_posted),
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
            time_profile_attribution: acc.dominant_time_profile_attribution.clone(),
            profiled_segment_count: acc.profiled_segment_count,
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
                let energy = acc.variants[0].band_energy[j];
                assert!(energy.is_finite() && energy >= 0.0, "non-finite band energy: {energy}");
                10.0 * energy.max(1e-30).log10()
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
    let ld = PropagationVariants::to_db(total_energy[0]);
    let le = PropagationVariants::to_db(total_energy[1]);
    let ln = PropagationVariants::to_db(total_energy[2]);

    (periods::periods(ld, le, ln), contributors)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::square_country_city::{Continent, SquareCountryCity};

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

    /// TH square-country-city for the baked-row speed gate below.
    const TH: SquareCountryCity = SquareCountryCity {
        continent: Continent::Asia,
        country_iso: *b"TH",
        city_id: 0,
    };

    /// One prepared secondary (class 3) segment 200 m from the receiver:
    /// tagged speed 50 and an all-estimated prior traffic block (the shape
    /// roads-finalize publishes for an unobserved section).
    fn secondary_segment() -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            square_country_city: None,
            time_profile_attribution: None,
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
            traffic: crate::normalize::RoadTraffic {
                light: 2640.0,
                medium: 120.0,
                heavy: 180.0,
                moto: 60.0,
                estimated: 15,
                time_profile: None,
                cross_section_aadt: 0.0,
            },
            source_id: 0,
            dist_m: 200.0,
            cp_lat: 50.0,
            cp_lon: 14.0015,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
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

    /// Prepared traffic is location-independent: a row whose baked
    /// SquareCountryCity is TH reports the SAME prepared counts as one under
    /// an UNKNOWN receiver — the country cascade now lives in the producer.
    /// The row country still drives the untagged SPEED default (gate below).
    #[test]
    fn prepared_traffic_passes_through_regardless_of_square() {
        let seg = secondary_segment();
        let world = one_road_meta(std::slice::from_ref(&seg));
        let baked = one_road_meta(&[RoadSegment {
            square_country_city: Some(TH),
            ..seg.clone()
        }]);
        for meta in [&world, &baked] {
            assert_eq!(meta.aadt_light, 2640.0);
            assert_eq!(meta.aadt_heavy, 180.0);
            assert_eq!(meta.traffic_estimated, 15);
        }
        assert_eq!(world.speed_source, "osm_posted");
    }

    /// Observed timing attribution rides the dominant segment into
    /// RoadMetadata verbatim; the group counter separates profiled from
    /// unprofiled segments so the popup never claims one profile for a
    /// mixed group.
    #[test]
    fn timing_attribution_and_profiled_count_reach_road_metadata() {
        let seg = secondary_segment();
        let profiled = RoadSegment {
            traffic: crate::normalize::RoadTraffic {
                // Doubled counts keep this segment deterministically dominant
                // in the mixed-group case below (dominance is day energy).
                light: seg.traffic.light * 2.0,
                time_profile: Some(crate::normalize::RoadTimeProfile {
                    light: Some([0.75, 0.18, 0.07]),
                    heavy: Some([0.55, 0.18, 0.27]),
                    ..Default::default()
                }),
                ..seg.traffic
            },
            time_profile_attribution: Some(crate::normalize::RoadTimeProfileAttribution {
                source: "https://example.org/counts".to_owned(),
                window: "2025-01..2025-12".to_owned(),
                total_transfer: false,
            }),
            ..seg.clone()
        };
        let meta = one_road_meta(std::slice::from_ref(&profiled));
        let attr = meta.time_profile_attribution.as_ref().unwrap();
        assert_eq!(attr.source, "https://example.org/counts");
        assert_eq!(attr.window, "2025-01..2025-12");
        assert!(!attr.total_transfer);
        assert_eq!(meta.profiled_segment_count, 1);

        // Mixed group: dominant attribution stays scoped to the profiled
        // segment while the counter reports only one profiled of two.
        let mixed = one_road_meta(&[profiled, seg.clone()]);
        assert_eq!(mixed.segment_count, 2);
        assert_eq!(mixed.profiled_segment_count, 1);
        assert!(mixed.time_profile_attribution.is_some());

        let plain = one_road_meta(std::slice::from_ref(&seg));
        assert!(plain.time_profile_attribution.is_none());
        assert_eq!(plain.profiled_segment_count, 0);
    }

    /// A prepared total zero is a TRUE zero: no contributor, no class-default
    /// resurrection at runtime.
    #[test]
    fn prepared_true_zero_produces_no_contributor() {
        let seg = RoadSegment {
            traffic: crate::normalize::RoadTraffic::default(),
            ..secondary_segment()
        };
        let (_periods, contribs) = compute_roads(
            &receiver(),
            std::slice::from_ref(&seg),
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        );
        assert!(contribs.is_empty(), "true zero stays silent");
    }

    /// Stripe regression (fix-pack Fix 1): a 30 m building straddling the cp
    /// ray must NOT screen the whole 214 m segment. Its shadow covers ~22 % of
    /// the fan the receiver sees, so the loss is ~1 dB — the cp-ray verdict
    /// alone applied the full ~15 dB diffraction to every metre of the
    /// segment, which is the constant-width stripe behind buildings.
    #[test]
    fn box_on_the_cp_ray_screens_only_its_angular_share() {
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
        let lat_of = |north_m: f64| 50.0 + north_m / grid::geo::M_PER_DEG_LAT;
        let lon_of = |east_m: f64| 14.0 + east_m / grid::geo::m_per_deg_lon(50.0_f64.to_radians());
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
            &ObstacleSet::empty(),
            &FlatRasters,
            None,
        )
        .0
        .lden_db;
        let screened = compute_roads(&receiver(), &roads, &obstacles, &FlatRasters, None)
            .0
            .lden_db;
        let mut traces = TraceCollector::new();
        let traced = compute_roads(
            &receiver(),
            &roads,
            &obstacles,
            &FlatRasters,
            Some(&mut traces),
        )
        .0
        .lden_db;
        assert_eq!(
            traced, screened,
            "collecting the fan must not move the level"
        );
        let loss = clear - screened;
        assert!(
            loss > 0.2,
            "the box must screen the arc it covers, got {loss:.2} dB"
        );
        assert!(
            loss < 3.0,
            "…but not the whole segment: {loss:.2} dB (cp-ray verdict was ~15 dB)"
        );

        let PropagationBreakdown::Cnossos(propagation) = &traces.segments[0].propagation else {
            panic!("road trace must use CNOSSOS propagation")
        };
        let fan = propagation
            .screening
            .fan
            .as_ref()
            .expect("every line piece carries its quadrature nodes as the fan");
        let wire = serde_json::to_value(&traces.segments[0]).unwrap();
        assert_eq!(wire["propagation"]["screening"]["fan"]["quadrature"], "line_point_sum");
        // The box covers about a fifth of the piece's azimuths; its blocked nodes carry about
        // that share of the in-plane angle (the mask quantizes the box edges to its bins).
        assert!(
            (0.05..0.5).contains(&fan.blocked_fraction),
            "blocked fraction {}",
            fan.blocked_fraction
        );
        assert_eq!(fan.intervals.iter().filter(|interval| interval.contains_cp).count(), 1);
    }

    /// A dense-ish scene for the pool-size gate: a fan of secondary segments
    /// at varying ranges and offsets (several wide enough to arc-screen), a
    /// village of obstacle boxes, and one noise wall (a Barrier-kind polyline
    /// in the same index) — every branch of the kernel (narrow and wide buckets,
    /// grouping, dominant, traces) gets traffic.
    pub(crate) fn pool_gate_scene() -> (
        Vec<RoadSegment>,
        crate::propagation::obstacle_index::ObstacleSet,
    ) {
        use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
        use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};
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
        // One noise wall: an east-west polyline 40 m north of the origin,
        // crossing several segment rays — the next dense id after the boxes.
        b.add_polyline(
            &[(50.000_36, 13.998), (50.000_36, 14.003)],
            4.0,
            ObstacleKind::Barrier,
            29,
        );
        let obstacles = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };
        (segs, obstacles)
    }

    /// THE PARALLELISM GATE: the rayon pool size (i.e. how work is split and
    /// interleaved across threads) must never move a bit — periods,
    /// contributors and traces all byte-stable between a 1-thread and a
    /// multi-thread run. The popup is the acoustic reference; f64 accumulation
    /// order is part of its contract, and the two-pass kernel keeps that
    /// order by folding pass-2 results in segment order.
    #[test]
    fn pool_size_never_changes_the_bits() {
        let (segs, obstacles) = pool_gate_scene();
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
