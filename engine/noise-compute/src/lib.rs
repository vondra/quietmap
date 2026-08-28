//! noise-compute: Pure Rust noise computation engine.
//!
//! CNOSSOS-EU emission + ISO 9613-2 propagation + Doc 29 aircraft.
//! No I/O, no files, no napi. Pure computation.
//!
//! Single-receiver entry points: `compute_at_point` and
//! `compute_at_point_with_traces` (popup).

pub mod admin;
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
pub mod h0_production_selection;
mod h0_production_selection_parser;
pub mod low_profile;
pub mod normalize;
pub mod periods;
pub mod present;
pub mod propagation;
pub mod region_defaults_generated;
pub mod sources;
pub mod traces;
pub mod types;
pub mod wkb;

/// Checked-in numerical authority for a selected production H0 epoch. Absent
/// until `H0_QUADRATURE_ACCEPTED`; enabling the feature before that fails
/// closed in `build.rs`. Do not restore deleted `h0_v3_*` sources to mint it.
#[cfg(feature = "h0-production-selection")]
pub mod h0_production_selection_record {
    include!("h0_production_selection_record.rs");
}

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

mod source_names;
pub(crate) use source_names::*;

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
pub fn compute_at_point(
    receiver: &Receiver,
    roads: &[RoadSegment],
    railways: &[RailSegment],
    buildings: &[PointSource],
    industrial: &[PointSource],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    rasters: &dyn RasterSampler,
    config: &ComputeConfig,
) -> NoiseResult {
    compute_at_point_inner(
        receiver, roads, railways, buildings, industrial, barriers, obstacles, rasters, config,
        None,
    )
}

/// Variant that also takes a `TraceCollector` (popup uses this through
/// the source-reader to collect noise-segments traces alongside the
/// aggregate result).
pub fn compute_at_point_with_traces(
    receiver: &Receiver,
    roads: &[RoadSegment],
    railways: &[RailSegment],
    buildings: &[PointSource],
    industrial: &[PointSource],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    rasters: &dyn RasterSampler,
    config: &ComputeConfig,
    traces: Option<&mut TraceCollector>,
) -> NoiseResult {
    compute_at_point_inner(
        receiver, roads, railways, buildings, industrial, barriers, obstacles, rasters, config,
        traces,
    )
}

#[allow(clippy::too_many_arguments)]
fn compute_at_point_inner(
    receiver: &Receiver,
    roads: &[RoadSegment],
    railways: &[RailSegment],
    buildings: &[PointSource],
    industrial: &[PointSource],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    rasters: &dyn RasterSampler,
    _config: &ComputeConfig,
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
    let contrib_periods_free = |contribs: &[Contributor]| -> NoisePeriods {
        periods::sum_periods(
            &contribs
                .iter()
                .map(|c| c.periods_free.clone())
                .collect::<Vec<_>>(),
        )
    };

    if !roads.is_empty() {
        let t = std::time::Instant::now();
        let (road_periods, road_contributors) = compute_roads(
            receiver,
            roads,
            barriers,
            obstacles,
            rasters,
            traces.as_deref_mut(),
        );
        timings.road_ms = t.elapsed().as_secs_f64() * 1000.0;
        source_results.push(SourceResult {
            source_type: LayerKind::Road,
            periods: road_periods.clone(),
            periods_free: contrib_periods_free(&road_contributors),
            segment_count: roads.len(),
            displayed_count: present::display_count(&road_contributors),
        });
        all_contributors.extend(road_contributors);
    }

    if !railways.is_empty() {
        let t = std::time::Instant::now();
        let (rail_periods, rail_contributors) = compute_railways(
            receiver,
            railways,
            barriers,
            obstacles,
            rasters,
            traces.as_deref_mut(),
        );
        timings.rail_ms = t.elapsed().as_secs_f64() * 1000.0;
        source_results.push(SourceResult {
            source_type: LayerKind::Railway,
            periods: rail_periods,
            periods_free: contrib_periods_free(&rail_contributors),
            segment_count: railways.len(),
            displayed_count: present::display_count(&rail_contributors),
        });
        all_contributors.extend(rail_contributors);
    }

    if !buildings.is_empty() {
        let t = std::time::Instant::now();
        let (bld_periods, bld_contributors) = compute_point_sources(
            receiver,
            buildings,
            barriers,
            obstacles,
            rasters,
            LayerKind::Building,
            traces.as_deref_mut(),
        );
        timings.building_ms = t.elapsed().as_secs_f64() * 1000.0;
        source_results.push(SourceResult {
            source_type: LayerKind::Building,
            periods: bld_periods,
            periods_free: contrib_periods_free(&bld_contributors),
            segment_count: buildings.len(),
            displayed_count: present::display_count(&bld_contributors),
        });
        all_contributors.extend(bld_contributors);
    }

    if !industrial.is_empty() {
        let t = std::time::Instant::now();
        let (ind_periods, ind_contributors) = compute_point_sources(
            receiver,
            industrial,
            barriers,
            obstacles,
            rasters,
            LayerKind::Industrial,
            traces,
        );
        timings.industrial_ms = t.elapsed().as_secs_f64() * 1000.0;
        source_results.push(SourceResult {
            source_type: LayerKind::Industrial,
            periods: ind_periods,
            periods_free: contrib_periods_free(&ind_contributors),
            segment_count: industrial.len(),
            displayed_count: present::display_count(&ind_contributors),
        });
        all_contributors.extend(ind_contributors);
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
    let has_building_heights = rasters.building_height(receiver.lat, receiver.lon) != 0.0;
    let conf = confidence::Confidence::assess(
        has_census,
        has_railway,
        has_aircraft,
        has_terrain,
        has_building_heights,
    );

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

/// Road [`NoisePeriods`] at a receiver — the popup road path without trace
/// collection. Each segment's closest-point (`dist_m`/`cp_lat`/`cp_lon`/
/// `fraction`) must already be filled for THIS receiver. Exposed so the
/// surface-heatmap road parity validator can compare against the exact
/// popup reference instead of re-implementing the physics.
pub fn road_periods(
    receiver: &Receiver,
    roads: &[RoadSegment],
    barriers: &[Barrier],
    rasters: &dyn RasterSampler,
) -> NoisePeriods {
    compute_roads(receiver, roads, barriers, None, rasters, None).0
}

/// Railway [`NoisePeriods`] at a receiver — the popup rail path without trace
/// collection. Each segment's closest-point (`dist_m`/`cp_lat`/`cp_lon`/
/// `fraction`) and effective (post `service`/`parallel_divisor`) train counts
/// must already be filled for THIS receiver. Exposed so the surface-heatmap
/// rail parity validator compares against the exact popup reference.
pub fn rail_periods(
    receiver: &Receiver,
    railways: &[RailSegment],
    barriers: &[Barrier],
    rasters: &dyn RasterSampler,
) -> NoisePeriods {
    compute_railways(receiver, railways, barriers, None, rasters, None).0
}

/// Industrial [`NoisePeriods`] at a receiver — the popup point-source path
/// (`LayerKind::Industrial`) without trace collection. Each `PointSource`'s
/// `dist_m` must already be filled for THIS receiver. Exposed so the
/// surface-heatmap industrial parity validator compares against the exact
/// popup reference.
pub fn industrial_periods(
    receiver: &Receiver,
    sources: &[PointSource],
    barriers: &[Barrier],
    rasters: &dyn RasterSampler,
) -> NoisePeriods {
    compute_point_sources(
        receiver,
        sources,
        barriers,
        None,
        rasters,
        LayerKind::Industrial,
        None,
    )
    .0
}

/// Building [`NoisePeriods`] at a receiver — the popup point-source path
/// (`LayerKind::Building`) without trace collection. Each `PointSource`'s
/// `dist_m` must already be filled for THIS receiver. Exposed so the
/// surface-heatmap building parity validator compares against the exact popup
/// reference.
pub fn building_periods(
    receiver: &Receiver,
    sources: &[PointSource],
    barriers: &[Barrier],
    rasters: &dyn RasterSampler,
) -> NoisePeriods {
    compute_point_sources(
        receiver,
        sources,
        barriers,
        None,
        rasters,
        LayerKind::Building,
        None,
    )
    .0
}

/// Compute terrain/screening/vegetation path effects for one source-receiver pair.
/// Returns (TerrainBreakdown, ScreeningBreakdown, VegetationBreakdown).
pub fn compute_path_effects(
    rasters: &dyn RasterSampler,
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
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
            barriers,
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
            building_path_m: (obstacle_trace.height_m * 10.0).round() / 10.0,
            obstacle: if obstacle_trace.kind == "none" {
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
/// [`path_effects::ObstacleInput`]. With no index (raster mode) this is
/// `CANDIDATES_OFF` — byte-identical legacy behavior.
fn obstacle_input_for_ray<'a>(
    obstacles: Option<&crate::propagation::obstacle_index::ObstacleSet>,
    scratch: &'a mut Vec<crate::propagation::obstacle_index::CrossingCandidate>,
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    prune: Option<&crate::propagation::obstacle_index::CellPrune<'_>>,
) -> propagation::path_effects::ObstacleInput<'a> {
    match obstacles {
        Some(idx) => {
            match prune {
                Some(p) => idx.crossings_pruned(src_lat, src_lon, rcv_lat, rcv_lon, p, scratch),
                None => idx.crossings(src_lat, src_lon, rcv_lat, rcv_lon, scratch),
            }
            propagation::path_effects::ObstacleInput {
                candidates: scratch,
                replace_sample_buildings: true,
            }
        }
        None => propagation::path_effects::ObstacleInput::CANDIDATES_OFF,
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
    pub barriers: &'a [Barrier],
    pub obstacles: Option<&'a ObstacleSet>,
}

/// Popup-only wall authority for an absent vector store. Popup road/rail
/// evaluation has no raster-building fallback to preserve, so its barrier slice
/// is a complete wall skyline and an absent store can safely mean an EMPTY set.
/// This fixed the D4 wall at Voznice: returning the cp bands on
/// `obstacles: None` had applied one closest-point verdict to the whole 250 m
/// microsegment. Do not copy this substitution into the tile-painter/CUDA
/// raster fallback: those lanes would erase real raster-building screening by
/// replacing it with an incomplete wall-only fan.
static NO_VECTOR_OBSTACLES: propagation::obstacle_index::ObstacleSet =
    propagation::obstacle_index::ObstacleSet {
        indexes: Vec::new(),
    };

/// The obstacle set the arc rule clips against, or `None` when the arc rule
/// does not run at all ("no store AND no barriers" — the cp verdict stands).
/// ONE resolution shared by the mutable path, the prepared path and the
/// kernels' growth schedulers, so they cannot disagree on whether a segment
/// is arc-screened.
pub(crate) fn arc_obstacle_set<'a>(
    obstacles: Option<&'a ObstacleSet>,
    barriers: &[Barrier],
) -> Option<&'a ObstacleSet> {
    match obstacles {
        Some(set) => Some(set),
        None if !barriers.is_empty() => Some(&NO_VECTOR_OBSTACLES),
        None => None,
    }
}

/// ONE pass-1 scheduler step of the parallel line kernels, shared by roads and
/// railways so the growth chain cannot drift between them (their blocks were
/// identical except the source height): replay the skyline ensure this
/// segment's SEQUENTIAL twin would run — eliding the calls `needs_growth`
/// proves to be no-ops — and hand back the frozen state its parallel
/// evaluation must read. `None` = the segment is not arc-screened (span
/// pre-gate, degenerate span, or no obstacle store and no walls).
#[allow(clippy::too_many_arguments)]
pub(crate) fn arc_growth_chain_step(
    skyline: &mut propagation::arc_screening::ArcSkyline,
    epoch_snap: &mut Option<propagation::arc_screening::SkylineSnapshot>,
    arc_set: Option<&ObstacleSet>,
    receiver: &Receiver,
    barriers: &[Barrier],
    seg_start_lat: f64,
    seg_start_lon: f64,
    seg_end_lat: f64,
    seg_end_lon: f64,
    seg_dist_m: f64,
    seg_length_m: f64,
    source_height_m: f64,
    bounds: propagation::arc_screening::ArcBounds,
) -> Option<propagation::arc_screening::SkylineSnapshot> {
    let set = arc_set.filter(|_| {
        propagation::arc_screening::segment_can_span(seg_length_m, seg_dist_m, bounds)
    })?;
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
    if skyline.needs_growth(receiver.lat, receiver.lon, &p, bounds) {
        *epoch_snap = None;
        skyline.ensure_planned(
            receiver.lat,
            receiver.lon,
            &p,
            set,
            barriers,
            source_height_m,
            bounds,
        );
    }
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
        barriers: q.barriers,
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
) -> [f64; NUM_BANDS] {
    let Some(set) = arc_obstacle_set(q.obstacles, q.barriers) else {
        return *q.cp_screening;
    };
    propagation::arc_screening::arc_screened_attenuation_prepared_with_ground(
        &line_segment_arc_query(q, set),
        rasters,
        snapshot,
        q.ground_bands,
        scratch,
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
        fn building_height(&self, _lat: f64, _lon: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.5
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    /// An ABSENT vector obstacle store must behave exactly like an EMPTY one.
    /// Walls do not live in the obstacle index — they reach the skyline through
    /// their own slice — so an early return on `obstacles: None` silently denied
    /// every noise wall its angular treatment wherever the vector store has not
    /// been ingested, which is most of the world. The wall below covers part of
    /// the span and must move the bands away from the caller's cp verdict in
    /// BOTH configurations, identically. (Review 2026-08-04.)
    #[test]
    fn a_wall_is_screened_with_or_without_a_vector_store() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        // Wall 60 m north of the receiver, running east-west across the span.
        let barriers = [Barrier {
            osm_id: 1,
            segment_idx: 0,
            height_m: 6.0,
            start_lat: 50.08054,
            start_lon: 14.4188,
            end_lat: 50.08054,
            end_lon: 14.4212,
            dist_m: 60.0,
        }];
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
            barriers: &barriers,
            obstacles,
        };
        // The sequential composition of the same pieces the parallel kernels
        // use: `arc_obstacle_set` resolves the store, `line_segment_arc_query`
        // builds the query, the mutable arc kernel grows + evaluates.
        let run = |obstacles| {
            let q = mk(obstacles);
            let mut skyline = propagation::arc_screening::ArcSkyline::default();
            let mut scratch = propagation::arc_screening::ArcScreeningScratch::default();
            match arc_obstacle_set(q.obstacles, q.barriers) {
                None => *q.cp_screening,
                Some(set) => propagation::arc_screening::arc_screened_attenuation(
                    &line_segment_arc_query(&q, set),
                    &MockRasters,
                    &mut skyline,
                    &mut scratch,
                ),
            }
        };
        let without = run(None);
        let with_empty = run(Some(&empty));
        assert_eq!(
            without, with_empty,
            "absent store must equal empty store: {without:?} vs {with_empty:?}"
        );
        assert!(
            without.iter().any(|&b| b > 0.1),
            "the wall must screen SOMETHING — got the untouched cp bands {without:?}"
        );
    }

    #[test]
    fn test_road_end_to_end() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 1,
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

        let result = compute_at_point(
            &receiver,
            &roads,
            &[],
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &ComputeConfig::default(),
        );

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

    #[test]
    fn test_multi_source() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 1,
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
        }];
        let railways = vec![RailSegment {
            osm_id: 2,
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
        }];

        let result = compute_at_point(
            &receiver,
            &roads,
            &railways,
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &ComputeConfig::default(),
        );

        // Should have both road and railway sources
        assert_eq!(result.sources.len(), 2);
        assert!(
            result.total.lden_db > 40.0,
            "multi-source Lden={:.1}",
            result.total.lden_db
        );

        // Total should be louder than either source alone
        let road_only = compute_at_point(
            &receiver,
            &roads,
            &[],
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &ComputeConfig::default(),
        );
        let rail_only = compute_at_point(
            &receiver,
            &[],
            &railways,
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &ComputeConfig::default(),
        );

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

        let result = compute_at_point(
            &receiver,
            &roads,
            &[],
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &ComputeConfig::default(),
        );

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
        // 5 flights/day × 365 d B738 approach traffic as
        // `AirborneRowView`s and assert Lden via the v6 entry point.
        use crate::compute::aircraft_v6::{
            compute_aircraft_v6, AirborneRowView, BBox, SubSegmentSlice,
        };

        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let total_flights = 1825u64;
        let subs_per_flight = 3usize;
        let total_subs = total_flights as usize * subs_per_flight;

        let mut start_lat = Vec::with_capacity(total_subs);
        let mut start_lon = Vec::with_capacity(total_subs);
        let mut start_alt_m = Vec::with_capacity(total_subs);
        let mut end_lat = Vec::with_capacity(total_subs);
        let mut end_lon = Vec::with_capacity(total_subs);
        let mut end_alt_m = Vec::with_capacity(total_subs);
        let mut speed_kt = Vec::with_capacity(total_subs);
        let mut length_m = Vec::with_capacity(total_subs);
        let mut period_col = Vec::with_capacity(total_subs);
        let mut date_id_col = Vec::with_capacity(total_subs);
        let mut flags_col = Vec::with_capacity(total_subs);
        let mut terrain_start = Vec::with_capacity(total_subs);
        let mut terrain_end = Vec::with_capacity(total_subs);

        // Column buffers above stay alive for the whole compute call —
        // the row views borrow into them via slice indices.
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
                start_lat.push(50.08_f32 + 0.003 * s as f32);
                start_lon.push(14.43_f32);
                start_alt_m.push(500.0 - 50.0 * s as f32);
                end_lat.push(50.08_f32 + 0.003 * (s + 1) as f32);
                end_lon.push(14.43_f32);
                end_alt_m.push(500.0 - 50.0 * (s + 1) as f32);
                speed_kt.push(150.0);
                length_m.push(330.0);
                period_col.push(period);
                date_id_col.push(date_id);
                flags_col.push(0);
                terrain_start.push(0.0_f32);
                terrain_end.push(0.0_f32);
            }
        }

        // Build per-flight row views by slicing the shared buffers.
        let mut row_views: Vec<AirborneRowView<'_>> = Vec::with_capacity(total_flights as usize);
        for flight in 0..total_flights {
            let lo = flight as usize * subs_per_flight;
            let hi = lo + subs_per_flight;
            row_views.push(AirborneRowView {
                flight_id: flight,
                callsign: "",
                aircraft_type: [0u8; 4],
                profile_idx: 0,
                source_id: AIRCRAFT_ADSB_SOURCE_ID as u8,
                origin: 0,
                sub_segments: SubSegmentSlice {
                    start_lat: &start_lat[lo..hi],
                    start_lon: &start_lon[lo..hi],
                    start_alt_m: &start_alt_m[lo..hi],
                    end_lat: &end_lat[lo..hi],
                    end_lon: &end_lon[lo..hi],
                    end_alt_m: &end_alt_m[lo..hi],
                    speed_kt: &speed_kt[lo..hi],
                    length_m: &length_m[lo..hi],
                    period: &period_col[lo..hi],
                    date_id: &date_id_col[lo..hi],
                    flags: &flags_col[lo..hi],
                    terrain_start_elev_m: &terrain_start[lo..hi],
                    terrain_end_elev_m: &terrain_end[lo..hi],
                },
                bbox: BBox {
                    min_lat: 50.08,
                    max_lat: 50.10,
                    min_lon: 14.43,
                    max_lon: 14.44,
                },
            });
        }
        let (periods, _contribs, _band) = compute_aircraft_v6(
            &receiver,
            &row_views,
            &[],
            &MockRasters,
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
        let config = ComputeConfig {
            n_days: 365,
            ..Default::default()
        };
        let result = compute_at_point(
            &receiver,
            &roads,
            &railways,
            &[],
            &[],
            &[],
            None,
            &MockRasters,
            &config,
        );

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
