//! noise-compute: Pure Rust noise computation engine.
//!
//! CNOSSOS-EU emission + ISO 9613-2 propagation + Doc 29 aircraft.
//! No I/O, no files, no napi. Pure computation.
//!
//! Single-receiver entry point: `compute_at_point` (the popup passes a
//! `TraceCollector`).

pub mod city_consts_generated;
pub mod compute;
pub mod confidence;
pub mod constants;
pub mod country_defaults_generated;
pub mod country_speed_defaults_generated;
pub mod defaults;
pub mod emission;
pub mod envelope;
pub mod flight_id;
pub mod low_profile;
pub mod normalize;
pub mod periods;
pub mod present;
pub mod propagation;
pub mod region_defaults_generated;
pub mod source_names;
pub mod square_country_city;
pub(crate) use source_names::*;
pub mod sources;
pub mod traces;
pub mod types;
pub mod wkb;

use constants::*;
use emission::road::{self};
use propagation::geo;
use propagation::iso9613::{self, SourceGeometry};
use propagation::obstacle_index::ObstacleSet;
use traces::{
    build_point_segment_trace, build_rail_segment_trace, build_road_segment_trace, BuildPointTrace,
    BuildRailTrace, BuildRoadTrace,
};
use types::*;

use compute::point_sources::compute_point_sources;
use compute::railways::compute_railways;
use compute::roads::compute_roads;

/// Round to one decimal place (0.1 dB granularity — matches UI precision).
#[inline]
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Decode WKB hex string (Polygon type 3) to GeoJSON.
/// WKB format: byte_order(1) + type(4) + num_rings(4) + [num_points(4) + [x(8)+y(8)]*N]*R
///
/// Currently no in-crate caller: the point-source geometry path moved to z30
/// integer rings (`compute::point_sources::grid_ring_to_geojson`). Kept for
/// WKB-carrying callers outside this crate's ported kernels.
#[allow(dead_code)]
fn wkb_to_geojson(hex: &str) -> Option<serde_json::Value> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    if bytes.len() < 9 {
        return None;
    }
    let le = bytes[0] == 1;
    let wkb_type = if le {
        u32::from_le_bytes(bytes[1..5].try_into().ok()?)
    } else {
        u32::from_be_bytes(bytes[1..5].try_into().ok()?)
    };
    if wkb_type != 3 {
        return None;
    } // Only Polygon
    let num_rings = if le {
        u32::from_le_bytes(bytes[5..9].try_into().ok()?)
    } else {
        u32::from_be_bytes(bytes[5..9].try_into().ok()?)
    } as usize;
    let mut pos = 9;
    let mut rings = Vec::with_capacity(num_rings);
    for _ in 0..num_rings {
        if pos + 4 > bytes.len() {
            return None;
        }
        let np = if le {
            u32::from_le_bytes(bytes[pos..pos + 4].try_into().ok()?)
        } else {
            u32::from_be_bytes(bytes[pos..pos + 4].try_into().ok()?)
        } as usize;
        pos += 4;
        let mut coords = Vec::with_capacity(np);
        for _ in 0..np {
            if pos + 16 > bytes.len() {
                return None;
            }
            let x = if le {
                f64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?)
            } else {
                f64::from_be_bytes(bytes[pos..pos + 8].try_into().ok()?)
            };
            let y = if le {
                f64::from_le_bytes(bytes[pos + 8..pos + 16].try_into().ok()?)
            } else {
                f64::from_be_bytes(bytes[pos + 8..pos + 16].try_into().ok()?)
            };
            pos += 16;
            coords.push(serde_json::json!([x, y]));
        }
        rings.push(serde_json::Value::Array(coords));
    }
    Some(serde_json::json!({"type": "Polygon", "coordinates": rings}))
}

/// Compute noise at a single receiver point from all nearby sources.
/// Aircraft go through `compute::aircraft_v6::compute_aircraft_v6`,
/// invoked separately by the popup (see
/// `source-reader/src/aircraft_v6/mod.rs::add_v6_aircraft_to_result`).
/// The popup passes a `TraceCollector` for its per-segment traces.
#[allow(clippy::too_many_arguments)]
pub fn compute_at_point(
    receiver: &Receiver,
    roads: &[RoadSegment],
    railways: &[RailSegment],
    buildings: &[PointSource],
    industrial: &[PointSource],
    obstacles: &ObstacleSet,
    rasters: &dyn RasterSampler,
    mut traces: Option<&mut TraceCollector>,
) -> NoiseResult {
    let mut source_results = Vec::new();
    let mut all_contributors = Vec::new();
    let mut timings = crate::types::LayerTimings::default();

    // Free-field aggregate is summed from each Contributor's `periods_free`
    // (already computed alongside `periods`). Accepts a small under-count
    // vs the kernel's true source-wide total because Contributors below the
    // display threshold are dropped — comparable error to the existing
    // `other_sources_lden` accounting; never user-facing (wire field is the
    // new `lden_free`, previously always null).
    let mut push_layer = |kind: LayerKind,
                          segment_count: usize,
                          periods: NoisePeriods,
                          contributors: Vec<Contributor>| {
        let periods_free = periods::sum_periods(
            &contributors
                .iter()
                .map(|c| c.periods_free.clone())
                .collect::<Vec<_>>(),
        );
        source_results.push(SourceResult {
            source_type: kind,
            periods,
            periods_free,
            segment_count,
            displayed_count: present::display_count(&contributors),
        });
        all_contributors.extend(contributors);
    };

    // The road and rail kernels run concurrently: each one's pass 1 (the
    // sequential skyline growth chain) is 80–85 % of its wall time and the
    // two chains are independent — own skyline, own seen-edge set, own
    // trace list. Traces are appended in the sequential order (roads,
    // then railways), so the answer is the sequential composition bit for
    // bit. A scoped thread, not `rayon::join`: a multi-second non-yielding
    // chain must not sit on a pool worker that pass 2 of every concurrent
    // popup wants to steal from. Rail stays on the calling thread, whose
    // REACH_CACHE memo it fills.
    let collecting = traces.is_some();
    let (road, rail) = std::thread::scope(|scope| {
        let road = (!roads.is_empty()).then(|| {
            scope.spawn(|| {
                run_line_layer(collecting, |t| {
                    compute_roads(receiver, roads, obstacles, rasters, t)
                })
            })
        });
        let rail = (!railways.is_empty()).then(|| {
            run_line_layer(collecting, |t| {
                compute_railways(receiver, railways, obstacles, rasters, t)
            })
        });
        // A road-kernel panic keeps its own message on the caller thread.
        let road = road.map(|handle| {
            handle
                .join()
                .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
        });
        (road, rail)
    });
    for (kind, segment_count, layer, wall_ms) in [
        (LayerKind::Road, roads.len(), road, &mut timings.road_ms),
        (
            LayerKind::Railway,
            railways.len(),
            rail,
            &mut timings.rail_ms,
        ),
    ] {
        let Some(layer) = layer else {
            continue;
        };
        *wall_ms = layer.wall_ms;
        if let Some(t) = traces.as_deref_mut() {
            t.segments.extend(layer.traces);
        }
        push_layer(kind, segment_count, layer.periods, layer.contributors);
    }

    for (kind, sources, wall_ms) in [
        (LayerKind::Building, buildings, &mut timings.building_ms),
        (
            LayerKind::Industrial,
            industrial,
            &mut timings.industrial_ms,
        ),
    ] {
        if sources.is_empty() {
            continue;
        }
        let t = std::time::Instant::now();
        let (periods, contributors) = compute_point_sources(
            receiver,
            sources,
            obstacles,
            rasters,
            kind,
            traces.as_deref_mut(),
        );
        *wall_ms = t.elapsed().as_secs_f64() * 1000.0;
        push_layer(kind, sources.len(), periods, contributors);
    }

    // Aircraft are computed by `compute::aircraft_v6::compute_aircraft_v6`
    // and merged into the result downstream via
    // `source-reader::aircraft_v6::add_v6_aircraft_to_result`.

    // ── Total ──
    let total = periods::sum_periods(
        &source_results
            .iter()
            .map(|s| s.periods.clone())
            .collect::<Vec<_>>(),
    );
    let total_free = periods::sum_periods(
        &source_results
            .iter()
            .map(|s| s.periods_free.clone())
            .collect::<Vec<_>>(),
    );

    let finalized = present::finalize_popup_contributors(all_contributors, 30);
    all_contributors = finalized.shown;
    let other_sources_lden = finalized.other_lden_db;

    // Confidence assessment
    let has_census = roads
        .iter()
        .any(|r| sources::provenance_of(r.source_id).is_measured());
    let has_railway = !railways.is_empty()
        && railways
            .iter()
            .any(|r| r.trains_passenger > 0.0 || r.trains_freight > 0.0);
    // Aircraft visibility is downstream — `add_v6_aircraft_to_result`
    // sees the popup arrows and bumps confidence after merging.
    let has_aircraft = false;
    let has_terrain = rasters.elevation(receiver.lat, receiver.lon) != 200.0; // StubRasters returns 200.0
    let conf = confidence::Confidence::assess(has_census, has_railway, has_aircraft, has_terrain);

    NoiseResult {
        total,
        total_free,
        sources: source_results,
        contributors: all_contributors,
        other_sources_lden,
        confidence: conf,
        aircraft_detail: None,
        segments: Vec::new(),
        segments_meta: None,
        timings: Some(timings),
    }
}

/// One line layer's kernel run: its own trace list (the caller appends it in
/// layer order) and wall time.
struct LineLayerRun {
    periods: NoisePeriods,
    contributors: Vec<Contributor>,
    traces: Vec<SegmentTrace>,
    wall_ms: f64,
}

fn run_line_layer(
    collecting: bool,
    kernel: impl FnOnce(Option<&mut TraceCollector>) -> (NoisePeriods, Vec<Contributor>),
) -> LineLayerRun {
    let t = std::time::Instant::now();
    let mut traces = collecting.then(TraceCollector::new);
    let (periods, contributors) = kernel(traces.as_mut());
    LineLayerRun {
        periods,
        contributors,
        traces: traces.map(|t| t.segments).unwrap_or_default(),
        wall_ms: t.elapsed().as_secs_f64() * 1000.0,
    }
}

/// Compute terrain/screening/vegetation path effects for one source-receiver pair.
/// Returns (TerrainBreakdown, ScreeningBreakdown, VegetationBreakdown).
#[allow(clippy::too_many_arguments)]
pub fn compute_path_effects(
    rasters: &dyn RasterSampler,
    obstacles: &ObstacleSet,
    src_lat: f64,
    src_lon: f64,
    src_height: f64,
    receiver: &Receiver,
    dist_m: f64,
    exclusion_radius_m: f64,
) -> (TerrainBreakdown, ScreeningBreakdown, VegetationBreakdown) {
    let rcv_alt = receiver.altitude_m();
    let mut cand_scratch = Vec::new();

    // Unified path profile — one sampling, all four rasters + all metadata.
    let mut path_profile = propagation::PathProfile::new();
    rasters.build_path_profile(
        src_lat,
        src_lon,
        receiver.lat,
        receiver.lon,
        dist_m,
        &mut path_profile,
    );

    // Metadata only — the per-band attenuation arrays are consumed inside
    // `propagate_variants_full`; popup derives A-weighted `ΔL_A` from the
    // Contributor-level variant Lden deltas instead of any scalar here.
    let (terrain, terrain_profile_points) =
        propagation::path_effects::terrain_attenuation_with_meta(
            &mut path_profile,
            src_height,
            rcv_alt,
        );

    let obstacle_input = obstacle_input_for_ray(
        obstacles,
        &mut cand_scratch,
        src_lat,
        src_lon,
        receiver.lat,
        receiver.lon,
        None,
    );
    let (_screening_atten, obstacle_trace) =
        propagation::path_effects::screening_attenuation_with_meta(
            &mut path_profile,
            obstacle_input,
            src_height,
            rcv_alt,
            exclusion_radius_m,
            &terrain.attenuation_bands,
        );

    let forest_depth = propagation::path_profile::vegetation_run_length(
        &path_profile.t,
        &path_profile.forest_u8,
        path_profile.dist_m,
    );
    let sampled_path_m = dist_m;

    (
        TerrainBreakdown {
            delta_m: (terrain.delta_m * 100.0).round() / 100.0,
            profile_points: terrain_profile_points,
        },
        ScreeningBreakdown {
            obstacle: if obstacle_trace.edge.is_none() {
                None
            } else {
                Some(obstacle_trace)
            },
        },
        VegetationBreakdown {
            forest_depth_m: (forest_depth * 10.0).round() / 10.0,
            sampled_path_m: (sampled_path_m * 10.0).round() / 10.0,
        },
    )
}

/// Exact vector-obstacle crossings for one source→receiver ray, as an
/// [`path_effects::ObstacleInput`]. An empty index yields an empty candidate slice.
fn obstacle_input_for_ray<'a>(
    obstacles: &crate::propagation::obstacle_index::ObstacleSet,
    scratch: &'a mut Vec<crate::propagation::obstacle_index::CrossingCandidate>,
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    prune: Option<&crate::propagation::obstacle_index::CellPrune<'_>>,
) -> propagation::path_effects::ObstacleInput<'a> {
    match prune {
        Some(p) => obstacles.crossings_pruned(src_lat, src_lon, rcv_lat, rcv_lon, p, scratch),
        None => obstacles.crossings(src_lat, src_lon, rcv_lat, rcv_lon, scratch),
    }
    propagation::path_effects::ObstacleInput {
        candidates: scratch,
    }
}

/// One line microsegment's inputs to [`arc_screened_line_segment_prepared`].
pub(crate) struct LineSegmentScreening<'a> {
    pub receiver: &'a Receiver,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    /// The characteristic point the caller already evaluated…
    pub cp_lat: f64,
    pub cp_lon: f64,
    /// …its absolute source altitude (DEM + source height), which fixes the
    /// sight line the skyline's grazing prune measures obstacles against…
    pub src_alt_m: f64,
    /// …its screening and terrain bands, plus the CP ground vector that the
    /// current arc increment channel uses for the whole fan.
    pub cp_screening: &'a [f64; NUM_BANDS],
    pub cp_terrain: &'a [f64; NUM_BANDS],
    pub ground_g: f64,
    pub ground_bands: &'a [f64; NUM_BANDS],
    /// Source height above ground at any point of this segment.
    pub source_height_m: f64,
    /// Segment length and the receiver's distance to its nearest point.
    pub length_m: f64,
    pub dist_m: f64,
    pub obstacles: &'a ObstacleSet,
}

/// ONE pass-1 scheduler step of the parallel line kernels, shared by roads and
/// railways so the growth chain cannot drift between them (their blocks were
/// identical except the source height): replay the skyline ensure this
/// segment's SEQUENTIAL twin would run — eliding the calls `needs_growth`
/// proves to be no-ops — and hand back the frozen state its parallel
/// evaluation must read. `None` = the segment is not arc-screened (span
/// pre-gate or degenerate span).
#[allow(clippy::too_many_arguments)]
pub(crate) fn arc_growth_chain_step(
    skyline: &mut propagation::arc_screening::ArcSkyline,
    epoch_snap: &mut Option<propagation::arc_screening::SkylineSnapshot>,
    arc_set: &ObstacleSet,
    receiver: &Receiver,
    seg_start_lat: f64,
    seg_start_lon: f64,
    seg_end_lat: f64,
    seg_end_lon: f64,
    seg_dist_m: f64,
    seg_length_m: f64,
    source_height_m: f64,
    bounds: propagation::arc_screening::ArcBounds,
) -> Option<propagation::arc_screening::SkylineSnapshot> {
    if !propagation::arc_screening::segment_can_span(seg_length_m, seg_dist_m, bounds) {
        return None;
    }
    let set = arc_set;
    let p = propagation::arc_screening::planned_ensure(
        receiver.lat,
        receiver.lon,
        seg_start_lat,
        seg_start_lon,
        seg_end_lat,
        seg_end_lon,
        seg_dist_m,
        seg_length_m,
        bounds,
    )?;
    let grew = skyline.needs_growth(receiver.lat, receiver.lon, &p, bounds);
    if grew {
        *epoch_snap = None;
        let t_grow = std::time::Instant::now();
        skyline.ensure_planned(receiver.lat, receiver.lon, &p, set, source_height_m, bounds);
        propagation::arc_screening::note_growth_time(t_grow.elapsed().as_secs_f64() * 1000.0);
    }
    propagation::arc_screening::note_growth_step(grew);
    Some(epoch_snap.get_or_insert_with(|| skyline.snapshot()).clone())
}

/// The [`propagation::arc_screening::ArcScreening`] query for one line
/// microsegment — the ONE place the popup's line kernels (and any sequential
/// caller composing `arc_screened_attenuation` directly, see the wall test)
/// build it, so every path asks bit-identical questions.
fn line_segment_arc_query<'a>(
    q: &'a LineSegmentScreening<'a>,
    set: &'a ObstacleSet,
) -> propagation::arc_screening::ArcScreening<'a> {
    propagation::arc_screening::ArcScreening {
        receiver_lat: q.receiver.lat,
        receiver_lon: q.receiver.lon,
        receiver_alt_m: q.receiver.altitude_m(),
        start_lat: q.start_lat,
        start_lon: q.start_lon,
        end_lat: q.end_lat,
        end_lon: q.end_lon,
        source_height_m: q.source_height_m,
        cp_lat: q.cp_lat,
        cp_lon: q.cp_lon,
        src_alt_m: q.src_alt_m,
        cp_screening: q.cp_screening,
        cp_terrain: q.cp_terrain,
        ground_g: q.ground_g,
        obstacles: set,
        length_m: q.length_m,
        dist_m: q.dist_m,
        // Line sources never self-screen: a road has no footprint of its
        // own to exclude (unlike an industrial area source).
        exclusion_radius_m: 0.0,
        bounds: propagation::arc_screening::ArcBounds::shipped(),
    }
}

/// Arc-clipped screening for ONE road/rail microsegment (fix-pack Fix 1),
/// against a [`propagation::arc_screening::SkylineSnapshot`] the kernel's
/// growth scheduler froze at exactly the state this segment's sequential twin
/// would have read (see `compute_roads` pass 1). Both line kernels call THIS —
/// one implementation is what keeps road and rail from drifting apart. The
/// equivalent sequential form is `arc_screened_attenuation` on
/// [`line_segment_arc_query`], which the growth chain + snapshot replay
/// reproduce bit for bit.
pub(crate) fn arc_screened_line_segment_prepared(
    q: &LineSegmentScreening<'_>,
    rasters: &dyn RasterSampler,
    snapshot: &propagation::arc_screening::SkylineSnapshot,
    scratch: &mut propagation::arc_screening::ArcScreeningScratch,
    cp_obstacle: Option<&ScreeningObstacleTrace>,
) -> ([f64; NUM_BANDS], Option<ScreeningFanTrace>) {
    let query = line_segment_arc_query(q, q.obstacles);
    propagation::arc_screening::arc_screened_attenuation_prepared_with_ground(
        &query,
        rasters,
        snapshot,
        q.ground_bands,
        scratch,
        cp_obstacle,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::AIRCRAFT_ADSB_SOURCE_ID;

    /// Mock raster sampler for testing.
    struct MockRasters;
    impl RasterSampler for MockRasters {
        fn elevation(&self, _lat: f64, _lon: f64) -> f64 {
            200.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.5
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    /// A noise wall screens through the SAME obstacle store as the buildings:
    /// it is a `Barrier`-kind polyline in the index (there is no other wall
    /// channel any more), and the line-kernel composition
    /// (`line_segment_arc_query` + the mutable arc kernel, the pieces the
    /// parallel kernels replay) must move the bands off the caller's cp verdict
    /// — and leave them untouched when the store holds nothing. (Review
    /// 2026-08-04 found the slice-era form of this defect: walls then arrived
    /// by a side channel an absent store would silently skip.)
    #[test]
    fn a_wall_is_screened_through_the_obstacle_store() {
        use propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        // Wall 60 m north of the receiver, running east-west across the span.
        let mut wall_index = ObstacleIndex::builder(50.08, 14.42);
        wall_index.add_polyline(
            &[(50.08054, 14.4188), (50.08054, 14.4212)],
            6.0,
            ObstacleKind::Barrier,
            0,
        );
        let with_wall = propagation::obstacle_index::ObstacleSet {
            indexes: vec![std::sync::Arc::new(wall_index.build())],
        };
        let cp_screening = [0.0f64; NUM_BANDS];
        let cp_terrain = [0.0f64; NUM_BANDS];
        let ground_bands = propagation::iso9613::legacy_ground_atten_bands(0.5);
        let empty = propagation::obstacle_index::ObstacleSet {
            indexes: Vec::new(),
        };
        let mk = |obstacles| LineSegmentScreening {
            receiver: &receiver,
            start_lat: 50.0812,
            start_lon: 14.4180,
            end_lat: 50.0812,
            end_lon: 14.4220,
            cp_lat: 50.0812,
            cp_lon: 14.42,
            src_alt_m: 200.05,
            cp_screening: &cp_screening,
            cp_terrain: &cp_terrain,
            ground_g: 0.5,
            ground_bands: &ground_bands,
            source_height_m: 0.05,
            length_m: 285.0,
            dist_m: 133.0,
            obstacles,
        };
        // The sequential composition of the same pieces the parallel kernels
        // use: `line_segment_arc_query` builds the query, the mutable arc
        // kernel grows + evaluates.
        let run = |obstacles| {
            let q = mk(obstacles);
            let mut skyline = propagation::arc_screening::ArcSkyline::default();
            let mut scratch = propagation::arc_screening::ArcScreeningScratch::default();
            propagation::arc_screening::arc_screened_attenuation(
                &line_segment_arc_query(&q, q.obstacles),
                &MockRasters,
                &mut skyline,
                &mut scratch,
            )
        };
        let screened = run(&with_wall);
        assert!(
            screened.iter().any(|&b| b > 0.1),
            "the wall must screen SOMETHING — got the untouched cp bands {screened:?}"
        );
        let without = run(&empty);
        assert_eq!(
            without, cp_screening,
            "an empty store leaves the caller's cp verdict untouched"
        );
    }

    #[test]
    fn test_road_end_to_end() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 1,
            square_country_city: None,
            segment_idx: 0,
            // 500 m due north of the receiver, running east-west: the
            // declared dist_m/cp/fraction must AGREE with the geometry —
            // the finite-line correction reads the real perpendicular.
            start_lat: 50.084523,
            start_lon: 14.418460,
            end_lat: 50.084523,
            end_lon: 14.421540,
            length_m: 220.0,
            road_class: 0, // motorway
            speed_limit: 100,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 2,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0, // defaults
            source_id: 0,
            dist_m: 500.0,
            cp_lat: 50.084523,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
            access: 0,
            junction: 0,
            built_up: 0,
        }];

        let result = popup(&receiver, &roads, &[]);

        // Motorway at 500m with 30K AADT should produce ~55-65 dB Lden
        assert!(
            result.total.lden_db > 45.0 && result.total.lden_db < 75.0,
            "Motorway 500m: expected 45-75 dB, got {:.1}",
            result.total.lden_db
        );

        // Should have period decomposition
        assert!(
            result.total.ld_db > result.total.ln_db,
            "Day should be louder than night (Ld={:.1}, Ln={:.1})",
            result.total.ld_db,
            result.total.ln_db
        );

        // Should have at least one source result
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].source_type, LayerKind::Road);

        println!(
            "Motorway 500m: Ld={:.1} Le={:.1} Ln={:.1} Lden={:.1}",
            result.total.ld_db, result.total.le_db, result.total.ln_db, result.total.lden_db
        );
    }

    /// The popup's call for a road/rail scene: no point sources, no
    /// obstacles, no traces.
    fn popup(receiver: &Receiver, roads: &[RoadSegment], railways: &[RailSegment]) -> NoiseResult {
        compute_at_point(
            receiver,
            roads,
            railways,
            &[],
            &[],
            &crate::propagation::obstacle_index::ObstacleSet::empty(),
            &MockRasters,
            None,
        )
    }

    /// Residential street 100 m north of the popup's test receiver.
    fn residential_100m_north() -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            square_country_city: None,
            segment_idx: 0,
            start_lat: 50.080905,
            start_lon: 14.418460,
            end_lat: 50.080905,
            end_lon: 14.421540,
            length_m: 220.0,
            road_class: 2,
            speed_limit: 50,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 2,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0,
            source_id: 0,
            dist_m: 100.0,
            cp_lat: 50.080905,
            cp_lon: 14.42,
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

    /// Mainline track 200 m north of the popup's test receiver.
    fn mainline_200m_north() -> RailSegment {
        RailSegment {
            osm_id: 2,
            square_country_city: None,
            segment_idx: 0,
            start_lat: 50.081809,
            start_lon: 14.416920,
            end_lat: 50.081809,
            end_lon: 14.423080,
            length_m: 440.0,
            rail_type: 0,
            usage: 0,
            maxspeed: 100,
            trains_passenger: 80.0,
            trains_freight: 20.0,
            speed_kmh: 100.0,
            track_count: 2,
            name: String::new(),
            rail_ref: String::new(),
            dist_m: 200.0,
            cp_lat: 50.081809,
            cp_lon: 14.42,
            fraction: 0.5,
            bridge: false,
            tunnel: false,
            service: false,
            highspeed: false,
            parallel_divisor: 1,
            speed_source: 0,
            trains_passenger_source: 0,
            trains_freight_source: 0,
            source_id: 0,
        }
    }

    /// The road and rail kernels run on two threads; the answer must be the
    /// The same invariant on a scene that exercises what actually moved
    /// threads: 24 road segments in several osm groups, a 20-building
    /// obstacle set (skyline growth, seen-edge skip, census) and six
    /// railway segments at varied offsets, in both trace modes.
    #[test]
    fn joined_line_layers_match_the_sequential_composition_with_obstacles() {
        use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};
        let (roads, obstacles) = crate::compute::roads::tests::pool_gate_scene();
        let receiver = Receiver::new(50.0, 14.0, 200.0);
        let railways: Vec<RailSegment> = (0..6)
            .map(|k| {
                let mut rail = mainline_200m_north();
                let north_m = 150.0 + 90.0 * k as f64;
                let east_m = -120.0 + 40.0 * k as f64;
                let dlat = north_m / M_PER_DEG_LAT;
                let dlon = east_m / m_per_deg_lon(50.0_f64.to_radians());
                let span = rail.end_lon - rail.start_lon;
                rail.osm_id = 2 + k as i64 / 2;
                rail.segment_idx = k as i16;
                rail.start_lat = 50.0 + dlat;
                rail.end_lat = 50.0 + dlat;
                rail.start_lon = 14.0 + dlon - span / 2.0;
                rail.end_lon = 14.0 + dlon + span / 2.0;
                rail.cp_lat = 50.0 + dlat;
                rail.cp_lon = 14.0 + dlon;
                rail.dist_m = north_m.hypot(east_m);
                rail
            })
            .collect();
        let bits = |p: &NoisePeriods| [p.ld_db, p.le_db, p.ln_db, p.lden_db].map(f64::to_bits);
        for with_traces in [true, false] {
            let mut expected = TraceCollector::new();
            let (road_periods, road_contribs) = compute_roads(
                &receiver,
                &roads,
                &obstacles,
                &MockRasters,
                with_traces.then_some(&mut expected),
            );
            let (rail_periods, rail_contribs) = compute_railways(
                &receiver,
                &railways,
                &obstacles,
                &MockRasters,
                with_traces.then_some(&mut expected),
            );
            let mut traces = TraceCollector::new();
            let result = compute_at_point(
                &receiver,
                &roads,
                &railways,
                &[],
                &[],
                &obstacles,
                &MockRasters,
                with_traces.then_some(&mut traces),
            );
            assert_eq!(bits(&result.sources[0].periods), bits(&road_periods));
            assert_eq!(bits(&result.sources[1].periods), bits(&rail_periods));
            assert!(!road_contribs.is_empty() && !rail_contribs.is_empty());
            // compute_at_point re-orders contributors for display; the set
            // and every field must match the two isolated kernels.
            let sorted = |items: Vec<Contributor>| {
                let mut json: Vec<String> = items
                    .iter()
                    .map(|c| serde_json::to_string(c).unwrap())
                    .collect();
                json.sort();
                json
            };
            assert_eq!(
                sorted(result.contributors),
                sorted(road_contribs.into_iter().chain(rail_contribs).collect())
            );
            if with_traces {
                assert!(
                    expected.segments.len() > 24,
                    "scene must produce many traces"
                );
                assert_eq!(
                    serde_json::to_string(&traces.segments).unwrap(),
                    serde_json::to_string(&expected.segments).unwrap()
                );
            }
        }
    }

    /// sequential composition bit for bit — periods, and traces in layer
    /// order (roads, then railways).
    #[test]
    fn joined_line_layers_match_the_sequential_composition() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![residential_100m_north()];
        let railways = vec![mainline_200m_north()];
        let obstacles = crate::propagation::obstacle_index::ObstacleSet::empty();
        let bits = |p: &NoisePeriods| [p.ld_db, p.le_db, p.ln_db, p.lden_db].map(f64::to_bits);
        let mut expected = TraceCollector::new();
        let (road_periods, _) = compute_roads(
            &receiver,
            &roads,
            &obstacles,
            &MockRasters,
            Some(&mut expected),
        );
        let (rail_periods, _) = compute_railways(
            &receiver,
            &railways,
            &obstacles,
            &MockRasters,
            Some(&mut expected),
        );
        let mut traces = TraceCollector::new();
        let result = compute_at_point(
            &receiver,
            &roads,
            &railways,
            &[],
            &[],
            &obstacles,
            &MockRasters,
            Some(&mut traces),
        );
        assert_eq!(bits(&result.sources[0].periods), bits(&road_periods));
        assert_eq!(bits(&result.sources[1].periods), bits(&rail_periods));
        assert!(
            !expected.segments.is_empty(),
            "the scene must produce traces"
        );
        assert_eq!(
            serde_json::to_string(&traces.segments).unwrap(),
            serde_json::to_string(&expected.segments).unwrap(),
            "traces in layer order"
        );
    }

    #[test]
    fn test_multi_source() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![residential_100m_north()];
        let railways = vec![mainline_200m_north()];

        let result = popup(&receiver, &roads, &railways);

        // Should have both road and railway sources
        assert_eq!(result.sources.len(), 2);
        assert!(
            result.total.lden_db > 40.0,
            "multi-source Lden={:.1}",
            result.total.lden_db
        );

        // Total should be louder than either source alone
        let road_only = popup(&receiver, &roads, &[]);
        let rail_only = popup(&receiver, &[], &railways);

        assert!(
            result.total.lden_db > road_only.total.lden_db,
            "combined should be louder than road alone"
        );
        assert!(
            result.total.lden_db > rail_only.total.lden_db,
            "combined should be louder than rail alone"
        );

        println!(
            "Multi: road={:.1} rail={:.1} combined={:.1} dB Lden",
            road_only.total.lden_db, rail_only.total.lden_db, result.total.lden_db
        );
    }

    #[test]
    fn test_residential_nearby() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 2,
            square_country_city: None,
            segment_idx: 0,
            start_lat: 50.0801,
            start_lon: 14.42,
            end_lat: 50.0799,
            end_lon: 14.42,
            length_m: 22.0,
            road_class: 5, // residential
            speed_limit: 30,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 1,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0,
            source_id: 0,
            dist_m: 15.0,
            cp_lat: 50.08,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
            access: 0,
            junction: 0,
            built_up: 0,
        }];

        let result = popup(&receiver, &roads, &[]);

        // Residential at 15m with 500 AADT: ~40-55 dB
        assert!(
            result.total.lden_db > 30.0 && result.total.lden_db < 65.0,
            "Residential 15m: expected 30-65 dB, got {:.1}",
            result.total.lden_db
        );

        println!(
            "Residential 15m: Ld={:.1} Le={:.1} Ln={:.1} Lden={:.1}",
            result.total.ld_db, result.total.le_db, result.total.ln_db, result.total.lden_db
        );
    }

    #[test]
    fn test_aircraft_end_to_end() {
        // Aircraft path went via compute_aircraft_v6 in C2/C4 — the
        // legacy compute_aircraft was deleted. Reconstruct the same
        // 5 flights/day × 365 d B738 approach traffic as one flattened
        // `AirborneSegmentBatch` and assert Lden via the v6 entry point.
        use crate::compute::aircraft_v6::{
            compute_aircraft_v6, AirborneFlightTable, AirborneSegmentBatch,
        };

        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let total_flights = 1825u64;
        let subs_per_flight = 3usize;
        let total_subs = total_flights as usize * subs_per_flight;

        let mut flight_id = Vec::with_capacity(total_subs);
        let mut flight_key = Vec::with_capacity(total_subs);
        let mut start_gy = Vec::with_capacity(total_subs);
        let mut start_gx = Vec::with_capacity(total_subs);
        let mut start_alt_m = Vec::with_capacity(total_subs);
        let mut end_gy = Vec::with_capacity(total_subs);
        let mut end_gx = Vec::with_capacity(total_subs);
        let mut end_alt_m = Vec::with_capacity(total_subs);
        let mut period_col = Vec::with_capacity(total_subs);
        let mut date_id_col = Vec::with_capacity(total_subs);

        for flight in 0..total_flights {
            let period = if flight % 100 < 65 {
                0u8
            } else if flight % 100 < 85 {
                1
            } else {
                2
            };
            let date_id = (flight / 5) as i16;
            for s in 0..subs_per_flight {
                flight_id.push(flight);
                flight_key.push(flight as i32);
                let (gx, gy) = grid::lonlat_to_grid(
                    f64::from(14.43_f32),
                    f64::from(50.08_f32 + 0.003 * s as f32),
                );
                start_gx.push(gx);
                start_gy.push(gy);
                start_alt_m.push(500 - 50 * s as i16);
                let (gx, gy) = grid::lonlat_to_grid(
                    f64::from(14.43_f32),
                    f64::from(50.08_f32 + 0.003 * (s + 1) as f32),
                );
                end_gx.push(gx);
                end_gy.push(gy);
                end_alt_m.push(500 - 50 * (s + 1) as i16);
                period_col.push(period);
                date_id_col.push(date_id);
            }
        }
        let n_flights = total_flights as usize;
        let row_views = [AirborneSegmentBatch {
            flight_id: &flight_id,
            flight_key: &flight_key,
            flights: AirborneFlightTable {
                callsign_offsets: &vec![0i32; n_flights + 1],
                callsign_bytes: &[],
                aircraft_type: &vec![0u8; 4 * n_flights],
                profile_idx: &vec![0u8; n_flights],
                source_id: &vec![AIRCRAFT_ADSB_SOURCE_ID as u8; n_flights],
                origin: &vec![0u8; n_flights],
            },
            start_gy: &start_gy,
            start_gx: &start_gx,
            start_alt_m: &start_alt_m,
            end_gy: &end_gy,
            end_gx: &end_gx,
            end_alt_m: &end_alt_m,
            speed_kt: &vec![150.0f32; total_subs],
            length_m: &vec![330.0f32; total_subs],
            period: &period_col,
            date_id: &date_id_col,
            flags: &vec![0u8; total_subs],
            terrain_start_elev_m: &vec![0i16; total_subs],
            terrain_end_elev_m: &vec![0i16; total_subs],
        }];
        let horizon = emission::aircraft::ReceiverHorizon::build(
            |lat, lon| MockRasters.elevation(lat, lon),
            receiver.lat,
            receiver.lon,
            receiver.altitude_m(),
        );
        let (periods, _contribs, _band) = compute_aircraft_v6(
            &receiver,
            &row_views,
            &[],
            &MockRasters,
            Some(&horizon),
            None,
            365,
            &crate::emission::aircraft::ClassWeights::uniform(),
            0,
            None,
            None,
        );

        assert!(
            periods.lden_db > 25.0 && periods.lden_db < 75.0,
            "Aircraft Lden: expected 25-75, got {:.1}",
            periods.lden_db
        );
        assert!(
            periods.ld_db > periods.ln_db || periods.ln_db == f64::NEG_INFINITY,
            "Day should be louder: Ld={:.1} Ln={:.1}",
            periods.ld_db,
            periods.ln_db
        );
    }

    #[test]
    fn test_all_sources_combined() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 1,
            square_country_city: None,
            segment_idx: 0,
            start_lat: 50.081,
            start_lon: 14.42,
            end_lat: 50.079,
            end_lon: 14.42,
            length_m: 220.0,
            road_class: 2,
            speed_limit: 50,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 2,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0,
            source_id: 0,
            dist_m: 100.0,
            cp_lat: 50.08,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
            access: 0,
            junction: 0,
            built_up: 0,
        }];
        let railways = vec![RailSegment {
            osm_id: 2,
            square_country_city: None,
            segment_idx: 0,
            start_lat: 50.082,
            start_lon: 14.42,
            end_lat: 50.078,
            end_lon: 14.42,
            length_m: 440.0,
            rail_type: 0,
            usage: 0,
            maxspeed: 100,
            trains_passenger: 80.0,
            trains_freight: 20.0,
            speed_kmh: 100.0,
            track_count: 2,
            name: String::new(),
            rail_ref: String::new(),
            dist_m: 200.0,
            cp_lat: 50.08,
            cp_lon: 14.42,
            fraction: 0.5,
            bridge: false,
            tunnel: false,
            service: false,
            highspeed: false,
            parallel_divisor: 1,
            speed_source: 0,
            trains_passenger_source: 0,
            trains_freight_source: 0,
            source_id: 0,
        }];
        let result = popup(&receiver, &roads, &railways);

        // Should have road + railway
        assert!(
            result.sources.len() >= 2,
            "sources = {:?}",
            result
                .sources
                .iter()
                .map(|s| &s.source_type)
                .collect::<Vec<_>>()
        );
        assert!(
            result.total.lden_db > 40.0,
            "combined Lden = {:.1}",
            result.total.lden_db
        );

        for s in &result.sources {
            println!("  {}: Lden={:.1}", s.source_type, s.periods.lden_db);
        }
        println!("  TOTAL: Lden={:.1}", result.total.lden_db);
    }
}
