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
pub mod country_speed_defaults_generated;
pub mod road_traffic_priors_generated;
pub mod defaults;
pub mod emission;
pub mod envelope;
pub mod flight_id;
pub mod low_profile;
pub mod normalize;
pub mod periods;
pub mod present;
pub mod propagation;
pub mod source_names;
pub mod square_country_city;
pub(crate) use source_names::*;
pub mod sources;
pub mod traces;
pub mod types;
pub mod wkb;

use constants::*;
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
    ships: &[PointSource],
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

    // The road and rail kernels run concurrently, each with its own trace
    // list. Traces are appended in the sequential order (roads, then
    // railways), so the answer is the sequential composition bit for bit.
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
        (LayerKind::Ship, ships, &mut timings.ship_ms),
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
            .any(|r| !r.traffic.is_silent());
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

/// The popup's terrain, obstacle and forest context of one source point: the detail of its
/// full CNOSSOS ray (the transfer itself comes from the layer's quadrature).
pub fn nearest_path_breakdown(
    rasters: &dyn RasterSampler,
    obstacles: &ObstacleSet,
    source: &propagation::ray_transfer::RaySource,
    receiver: &Receiver,
    weather: &propagation::meteorology::Meteorology,
) -> (TerrainBreakdown, ScreeningBreakdown, VegetationBreakdown) {
    let ray_receiver = propagation::ray_transfer::RayReceiver {
        lat: receiver.lat,
        lon: receiver.lon,
        altitude_m: receiver.altitude_m(),
    };
    let mut detail = None;
    propagation::ray_transfer::evaluate_ray_transfer(
        &ray_receiver,
        source,
        obstacles,
        true,
        rasters,
        weather,
        true,
        &mut propagation::ray_transfer::RayScratch::default(),
        Some(&mut detail),
    );
    let detail = detail.expect("evaluate_ray_transfer fills the requested detail");
    (
        TerrainBreakdown {
            delta_m: (detail.terrain.delta_m * 100.0).round() / 100.0,
            profile_points: detail.terrain.edges.len() as u32,
        },
        ScreeningBreakdown {
            obstacle: detail.obstacle.edge.is_some().then_some(detail.obstacle),
        },
        VegetationBreakdown {
            forest_depth_m: (detail.forest_depth_m * 10.0).round() / 10.0,
            sampled_path_m: (detail.profile.dist_m * 10.0).round() / 10.0,
        },
    )
}

/// Exact vector-obstacle crossings for one source→receiver ray, as an
/// [`propagation::path_effects::ObstacleInput`] (airport ground operations' single-edge path).
pub(crate) fn obstacle_input_for_ray<'a>(
    obstacles: &ObstacleSet,
    scratch: &'a mut Vec<propagation::obstacle_index::CrossingCandidate>,
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    prune: Option<&propagation::obstacle_index::CellPrune<'_>>,
) -> propagation::path_effects::ObstacleInput<'a> {
    match prune {
        Some(p) => obstacles.crossings_pruned(src_lat, src_lon, rcv_lat, rcv_lon, p, scratch),
        None => obstacles.crossings(src_lat, src_lon, rcv_lat, rcv_lon, scratch),
    }
    propagation::path_effects::ObstacleInput {
        candidates: scratch,
    }
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

    /// A noise wall screens through the SAME obstacle store as the buildings: a
    /// `Barrier`-kind polyline between a road piece and the receiver lowers the
    /// piece's received energy, and an empty store leaves it open.
    #[test]
    fn a_wall_is_screened_through_the_obstacle_store() {
        use crate::compute::line_piece::{evaluate_line_piece, LinePiece, LinePieceScratch};
        use crate::propagation::ray_transfer::{RayReceiver, VARIANT_FULL};
        use propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
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
        let receiver = RayReceiver {
            lat: 50.08,
            lon: 14.42,
            altitude_m: 204.0,
        };
        let piece = LinePiece {
            start_lat: 50.0812,
            start_lon: 14.4180,
            end_lat: 50.0812,
            end_lon: 14.4220,
            source_height_m: 0.05,
            source_ground_factor: 0.0,
            platform_half_width_m: 5.0,
            directivity: propagation::line_quadrature::LineDirectivity::Omnidirectional,
        };
        let full_1k = |obstacles: &propagation::obstacle_index::ObstacleSet| {
            let transfer = evaluate_line_piece(
                &receiver,
                &piece,
                50.0812,
                14.42,
                obstacles,
                &MockRasters,
                &crate::propagation::meteorology::Meteorology::defaults(),
                &mut LinePieceScratch::default(),
                None,
            )
            .unwrap();
            transfer.periods[0][VARIANT_FULL][4]
        };
        let open = full_1k(&propagation::obstacle_index::ObstacleSet::empty());
        let screened = full_1k(&with_wall);
        assert!(
            10.0 * (open / screened).log10() > 3.0,
            "the wall must screen the piece: open {open:e}, screened {screened:e}"
        );
    }

    #[test]
    fn test_road_end_to_end() {
        let receiver = Receiver::new(50.08, 14.42, 200.0);
        let roads = vec![RoadSegment {
            osm_id: 1,
            square_country_city: None,
            time_profile_attribution: None,
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
            traffic: crate::normalize::RoadTraffic {
                light: 21_600.0,
                medium: 2_400.0,
                heavy: 5_700.0,
                moto: 300.0,
                estimated: 15,
                time_profile: None,
                cross_section_aadt: 0.0,
            },
            source_id: 0,
            dist_m: 500.0,
            cp_lat: 50.084523,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
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
            time_profile_attribution: None,
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
            traffic: crate::normalize::RoadTraffic {
                light: 7_470.0,
                medium: 540.0,
                heavy: 810.0,
                moto: 180.0,
                estimated: 15,
                time_profile: None,
                cross_section_aadt: 0.0,
            },
            source_id: 0,
            dist_m: 100.0,
            cp_lat: 50.080905,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
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
            traffic: crate::normalize::RailTraffic {
                passenger: crate::normalize::RailCategoryTraffic {
                    periods: [56.0, 16.0, 8.0],
                    status: 2,
                    ..Default::default()
                },
                freight: crate::normalize::RailCategoryTraffic {
                    periods: [10.0, 3.3333333333333335, 6.666666666666667],
                    status: 2,
                    ..Default::default()
                },
            },
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
            speed_source: 0,
        }
    }

    /// The road and rail kernels run on two threads; the answer must be the
    /// The same invariant on a scene that exercises what actually moved
    /// threads: 24 road segments in several osm groups, a 20-building
    /// obstacle set (wide-bucket masks and crossings) and six
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
            time_profile_attribution: None,
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
            traffic: crate::normalize::RoadTraffic {
                light: 480.0,
                medium: 5.0,
                heavy: 10.0,
                moto: 5.0,
                estimated: 15,
                time_profile: None,
                cross_section_aadt: 0.0,
            },
            source_id: 0,
            dist_m: 15.0,
            cp_lat: 50.08,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
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
            &crate::emission::aircraft::SamplingWindow {
                baseline_days: 365,
                increment_days: 0,
                baseline_days_sha256: String::new(),
                increment_days_sha256: String::new(),
            },
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
            time_profile_attribution: None,
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
            traffic: crate::normalize::RoadTraffic {
                light: 7_470.0,
                medium: 540.0,
                heavy: 810.0,
                moto: 180.0,
                estimated: 15,
                time_profile: None,
                cross_section_aadt: 0.0,
            },
            source_id: 0,
            dist_m: 100.0,
            cp_lat: 50.08,
            cp_lon: 14.42,
            fraction: 0.5,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
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
            traffic: crate::normalize::RailTraffic {
                passenger: crate::normalize::RailCategoryTraffic {
                    periods: [56.0, 16.0, 8.0],
                    status: 2,
                    ..Default::default()
                },
                freight: crate::normalize::RailCategoryTraffic {
                    periods: [10.0, 3.3333333333333335, 6.666666666666667],
                    status: 2,
                    ..Default::default()
                },
            },
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
            speed_source: 0,
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
