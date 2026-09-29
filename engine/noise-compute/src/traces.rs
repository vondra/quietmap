//! Helpers for building per-segment popup traces from already-computed engine
//! state. Every number returned here must come from values the engine already
//! holds; this module never re-runs emission or propagation.

use crate::propagation::iso9613;
use crate::propagation::meteorology::Meteorology;
use crate::propagation::ray_transfer::RayDetail;
use crate::propagation::relevance_bound::SourceSpread;
use crate::propagation::PathProfile;
use crate::types::{
    BaselineTrace, CnossosBreakdown, EmissionTrace, ForestRun, GroundTrace, LayerKind,
    LdenVariants, PathProfileTrace, PerPeriod, PointSource, PropagationBreakdown,
    PropagationVariants, RailSegment, RoadSegment, ScreeningFanTrace, ScreeningObstacleTrace,
    ScreeningTrace, SegmentTrace, TerrainTrace, VegetationTrace, NUM_BANDS,
};
use crate::{building_type_name, industrial_type_name, rail_type_name, ship_type_name};

mod aircraft;
pub use aircraft::{
    build_aircraft_airborne_subsegment_trace, build_aircraft_cruise_cell_trace,
    BuildAircraftAirborneSubSegmentTrace, BuildAircraftCruiseCellTrace,
};
// Emit-time Lden probe for the cruise pre-selection stub; crate-internal.
pub(crate) use aircraft::cruise_cell_lden_full;

/// Convert band-energies (linear, A-weighted) to band levels in dB(A). Non-finite or
/// negative energies fail closed like [`PropagationVariants::to_db`], never floor.
pub fn bands_energy_to_db(bands: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|j| {
        assert!(bands[j].is_finite() && bands[j] >= 0.0, "non-finite band energy: {}", bands[j]);
        10.0 * bands[j].max(1e-30).log10()
    })
}

/// Trace name for unnamed + ref-less OSM ways. Prefixing the class/type name
/// means SegmentRow shows a useful subtype hint instead of a bare "osm:<id>".
#[inline]
fn seg_name_from_tags(ref_tag: &str, name_tag: &str, subtype: &str, osm_id: i64) -> String {
    if !ref_tag.is_empty() {
        ref_tag.to_string()
    } else if !name_tag.is_empty() {
        name_tag.to_string()
    } else {
        format!("{} osm:{}", subtype, osm_id)
    }
}

/// Lden under each propagation-variant hypothesis (full / free-field /
/// no_terrain / no_screening / no_vegetation). Uses the existing
/// `PropagationVariants::lden_from_periods` helper, so values match what the
/// grouped contributor would show for the same segment.
pub fn variants_to_lden(variants: &[PropagationVariants; 3]) -> LdenVariants {
    let lden_of = |f: fn(&PropagationVariants) -> f64| {
        PropagationVariants::lden_from_periods(&variants[0], &variants[1], &variants[2], f)
    };
    LdenVariants {
        full: lden_of(|v| v.full_energy),
        free_field: lden_of(|v| v.free_field_energy),
        no_terrain: lden_of(|v| v.no_terrain_energy),
        no_screening: lden_of(|v| v.no_screening_energy),
        no_vegetation: lden_of(|v| v.no_vegetation_energy),
        no_ground: lden_of(|v| v.no_ground_energy),
        no_atmospheric: lden_of(|v| v.no_atmospheric_energy),
    }
}

/// Per-period received-band levels [dB(A) per band], derived from the variant
/// `band_energy` (full hypothesis — includes all path effects).
pub fn variants_to_received_bands(
    variants: &[PropagationVariants; 3],
) -> PerPeriod<[f64; NUM_BANDS]> {
    PerPeriod {
        day: bands_energy_to_db(&variants[0].band_energy),
        evening: bands_energy_to_db(&variants[1].band_energy),
        night: bands_energy_to_db(&variants[2].band_energy),
    }
}

/// Atmospheric attenuation per period (per band, positive = dB removed) over a
/// distance: each period's A_atm of the ray transfer (propagation::air_absorption).
/// Ground ops pass the distance past the 25 m anchor, every other layer the slant.
/// The one chart path behind every popup atmospheric spectrum, so the chart and the
/// period-aware levels cannot disagree again.
pub fn atmospheric_bands(
    d_slant_m: f64,
    weather: &Meteorology,
) -> PerPeriod<[f64; NUM_BANDS]> {
    let bands = |period: usize| std::array::from_fn(|band| weather.absorption[period][band].attenuation_db(d_slant_m));
    PerPeriod { day: bands(0), evening: bands(1), night: bands(2) }
}

/// Consumes a `PathProfile` into a serializable `PathProfileTrace` (dropping
/// the internal `elevation_f64_scratch` buffer). Takes the profile by value
/// so callers can `std::mem::take(...)` to avoid cloning the sample arrays.
pub fn path_profile_into_trace(
    profile: PathProfile,
    src_alt_m: f64,
    rcv_alt_m: f64,
) -> PathProfileTrace {
    PathProfileTrace {
        t: profile.t,
        elevation_m: profile.elevation_m,
        forest_u8: profile.forest_u8,
        imd_u8: profile.imd_u8,
        dist_m: profile.dist_m,
        step_m_med: profile.step_m_med,
        src_lat: profile.src_lat,
        src_lon: profile.src_lon,
        rcv_lat: profile.rcv_lat,
        rcv_lon: profile.rcv_lon,
        src_alt_m,
        rcv_alt_m,
    }
}

/// Build a `ScreeningTrace` from the values already returned by
/// `screening_attenuation_with_meta`.
pub fn screening_trace(
    atten_bands: [f64; NUM_BANDS],
    obstacle: ScreeningObstacleTrace,
    fan: Option<ScreeningFanTrace>,
) -> ScreeningTrace {
    ScreeningTrace {
        attenuation_bands: atten_bands,
        obstacle: if obstacle.edge.is_none() {
            None
        } else {
            Some(obstacle)
        },
        fan,
    }
}

/// Build a `VegetationTrace` from the kernel's day-mixed foliage attenuation and the
/// homogeneous ray's runs and depth: the display shows the same metres the kernel
/// attenuated (the frontend renders only the total depth + run count and draws tufts
/// from raw `forest_u8` samples).
pub fn vegetation_trace(
    atten_bands: [f64; NUM_BANDS],
    forest_runs: Vec<ForestRun>,
    forest_depth_m: f64,
    dist_m: f64,
) -> VegetationTrace {
    VegetationTrace {
        forest_depth_m,
        sampled_path_m: dist_m,
        attenuation_bands: atten_bands,
        forest_runs,
    }
}

/// Build a `GroundTrace` from the kernel's already-formed direct-ground
/// vector. `factor_g` remains the path-mean display value; the bands include
/// the ray geometry, source-end correction, and meteorological mix used by the
/// surface kernel itself.
pub fn ground_trace(factor_g: f64, attenuation_bands: [f64; NUM_BANDS]) -> GroundTrace {
    GroundTrace {
        factor_g,
        attenuation_bands,
    }
}

/// Build a `BaselineTrace` from the slant distance, source height, ground G and urban
/// reflection boost. The reflection boost is included here for trace-API completeness even
/// though the internal `free_field` variant excludes it; popup derives the per-receiver A_refl
/// display from this field directly. The line quadrature has no finite-line correction.
pub fn baseline_trace(
    d_slant_m: f64,
    source_height_m: f64,
    ground_g: f64,
    reflection_boost_db: f64,
    source_spread: SourceSpread,
    weather: &Meteorology,
) -> BaselineTrace {
    BaselineTrace {
        geometric_db: source_spread.divergence_db(d_slant_m),
        atmospheric_bands: atmospheric_bands(d_slant_m, weather),
        ground_factor_g: ground_g,
        source_height_m,
        finite_line_corr_db: 0.0,
        reflection_boost_db,
    }
}

/// Already-computed common inputs for the CNOSSOS portion of a popup trace.
/// Source builders retain their emission and segment metadata; this only keeps
/// the shared propagation trace assembly in one place.
struct BuildCnossosPropagation {
    d_slant_m: f64,
    src_alt_m: f64,
    rcv_alt_m: f64,
    ground_g: f64,
    ground_bands: [f64; NUM_BANDS],
    reflection_boost_db: f64,
    source_spread: SourceSpread,
    path_profile: PathProfile,
    terrain: TerrainTrace,
    screening_atten: [f64; NUM_BANDS],
    screening_fan: Option<ScreeningFanTrace>,
    obstacle_trace: ScreeningObstacleTrace,
    veg_atten: [f64; NUM_BANDS],
    veg_runs: Vec<ForestRun>,
    veg_depth_m: f64,
    weather: Meteorology,
    variants: [PropagationVariants; 3],
    lw_bands: [[f64; NUM_BANDS]; 3],
}

/// Assemble the propagation fields shared by point, road, and rail popup
/// traces from values each kernel has already computed.
fn build_cnossos_propagation(inputs: BuildCnossosPropagation) -> PropagationBreakdown {
    let BuildCnossosPropagation {
        d_slant_m,
        src_alt_m,
        rcv_alt_m,
        ground_g,
        ground_bands,
        reflection_boost_db,
        source_spread,
        path_profile,
        terrain,
        screening_atten,
        screening_fan,
        obstacle_trace,
        veg_atten,
        veg_runs,
        veg_depth_m,
        weather,
        variants,
        lw_bands,
    } = inputs;
    let vegetation = vegetation_trace(veg_atten, veg_runs, veg_depth_m, path_profile.dist_m);
    PropagationBreakdown::Cnossos(Box::new(CnossosBreakdown {
        baseline: baseline_trace(d_slant_m, src_alt_m, ground_g, reflection_boost_db, source_spread, &weather),
        path_profile: path_profile_into_trace(path_profile, src_alt_m, rcv_alt_m),
        terrain,
        screening: screening_trace(screening_atten, obstacle_trace, screening_fan),
        vegetation,
        ground: ground_trace(ground_g, ground_bands),
        lw_bands: PerPeriod {
            day: lw_bands[0],
            evening: lw_bands[1],
            night: lw_bands[2],
        },
        lw_db_a: PerPeriod {
            day: iso9613::a_weighted_total(&lw_bands[0]),
            evening: iso9613::a_weighted_total(&lw_bands[1]),
            night: iso9613::a_weighted_total(&lw_bands[2]),
        },
        received_bands: variants_to_received_bands(&variants),
    }))
}

/// Inputs for building a per-segment road trace. Keyword-struct style to avoid
/// a 30-argument function. Consumed (not borrowed) for heavy fields that can
/// be moved into the resulting SegmentTrace.
pub(crate) struct BuildRoadTrace<'a> {
    pub seg: &'a RoadSegment,
    pub class_name: &'static str,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub reflection_boost_db: f64,
    /// Prepared traffic as consumed (counts + estimated bitmask).
    pub traffic: crate::normalize::RoadTraffic,
    pub speed_kmh: f64,
    pub surf_corr: f64,
    /// The piece's loudest quadrature node, on its own ray.
    pub node: RayDetail,
    pub fan: Option<ScreeningFanTrace>,
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
    pub weather: Meteorology,
    /// Stable per-kind row index for the top-K total-order tiebreak.
    pub sort_seq: u64,
}

pub(crate) struct BuildPointTrace<'a> {
    pub src: &'a PointSource,
    pub source_kind: LayerKind,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub prop_dist: f64,
    pub reflection_boost_db: f64,
    /// The source's ray.
    pub node: RayDetail,
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
    pub weather: Meteorology,
    /// Stable per-kind row index for the top-K total-order tiebreak.
    pub sort_seq: u64,
}

pub(crate) fn build_point_segment_trace(inputs: BuildPointTrace<'_>) -> SegmentTrace {
    let BuildPointTrace {
        src,
        source_kind,
        rcv_alt,
        d_slant,
        prop_dist,
        reflection_boost_db,
        node,
        seg_variants,
        lw_bands,
        weather,
        sort_seq,
    } = inputs;

    let (subtype_label, emission) = match source_kind {
        LayerKind::Ship => {
            let label = ship_type_name(src.source_type);
            (
                label,
                EmissionTrace::Ship {
                    source_type: label,
                    area_m2: src.area_m2 as f64,
                    hours_per_month: src.ship_hours.unwrap_or([0.0; 3]),
                    effective_area_source_dist_m: prop_dist,
                },
            )
        }
        LayerKind::Industrial => {
            let label = industrial_type_name(src.source_type);
            (
                label,
                EmissionTrace::Industrial {
                    source_type: label,
                    area_m2: src.area_m2 as f64,
                    nace: None,
                    hub_height_m: src.hub_height_m,
                    rated_power_kw: src.rated_power_kw,
                    effective_area_source_dist_m: prop_dist,
                },
            )
        }
        _ => {
            let label = building_type_name(src.source_type);
            (
                label,
                EmissionTrace::Building {
                    building_type: label,
                    height_m: crate::emission::settlement::building_height_from_source(
                        src.source_height_m as f64, src.floors,
                    ) as f32,
                    floors: src.floors,
                    area_m2: src.area_m2 as f64,
                },
            )
        }
    };

    SegmentTrace {
        kind: source_kind,
        osm_id: Some(src.osm_id),
        segment_idx: 0,
        name: src.name.clone(),
        subtype: subtype_label.to_string(),
        is_dominant_of_group: false,
        start_lat: src.lat,
        start_lon: src.lon,
        end_lat: src.lat,
        end_lon: src.lon,
        cp_lat: src.lat,
        cp_lon: src.lon,
        length_m: 0.0,
        dist_m: src.dist_m,
        d_slant_m: d_slant,
        bridge: false,
        tunnel: false,
        emission,
        propagation: build_cnossos_propagation(BuildCnossosPropagation {
            d_slant_m: d_slant,
            src_alt_m: node.source_altitude_m,
            rcv_alt_m: rcv_alt,
            ground_g: node.ground_factor,
            reflection_boost_db,
            source_spread: SourceSpread::Point,
            path_profile: node.profile,
            terrain: node.terrain,
            ground_bands: node.ground_bands,
            screening_atten: node.screening_bands,
            screening_fan: None,
            obstacle_trace: node.obstacle,
            veg_atten: node.vegetation_bands,
            veg_runs: node.foliage_runs,
            veg_depth_m: node.forest_depth_m,
            weather,
            variants: seg_variants,
            lw_bands,
        }),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        cell_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
        sort_seq,
    }
}

pub(crate) struct BuildRailTrace<'a> {
    pub seg: &'a RailSegment,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub reflection_boost_db: f64,
    pub speed_kmh: f64,
    /// Stable per-kind row index for the top-K total-order tiebreak.
    pub sort_seq: u64,
    /// The piece's loudest quadrature node, on its own ray.
    pub node: RayDetail,
    pub fan: Option<ScreeningFanTrace>,
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
    pub weather: Meteorology,
}

/// The CNOSSOS breakdown of a line piece from its loudest node's ray.
fn line_node_propagation(
    node: RayDetail,
    fan: Option<ScreeningFanTrace>,
    d_slant_m: f64,
    rcv_alt_m: f64,
    reflection_boost_db: f64,
    variants: [PropagationVariants; 3],
    lw_bands: [[f64; NUM_BANDS]; 3],
    weather: Meteorology,
) -> PropagationBreakdown {
    build_cnossos_propagation(BuildCnossosPropagation {
        d_slant_m,
        src_alt_m: node.source_altitude_m,
        rcv_alt_m,
        ground_g: node.ground_factor,
        reflection_boost_db,
        source_spread: SourceSpread::Line,
        path_profile: node.profile,
        terrain: node.terrain,
        ground_bands: node.ground_bands,
        screening_atten: node.screening_bands,
        screening_fan: fan,
        obstacle_trace: node.obstacle,
        veg_atten: node.vegetation_bands,
        veg_runs: node.foliage_runs,
        veg_depth_m: node.forest_depth_m,
        weather,
        variants,
        lw_bands,
    })
}

pub(crate) fn build_rail_segment_trace(inputs: BuildRailTrace<'_>) -> SegmentTrace {
    let BuildRailTrace {
        seg,
        rcv_alt,
        d_slant,
        reflection_boost_db,
        speed_kmh,
        node,
        fan,
        seg_variants,
        lw_bands,
        weather,
        sort_seq,
    } = inputs;

    let rail_type = rail_type_name(seg.rail_type);
    let seg_name = seg_name_from_tags(&seg.rail_ref, &seg.name, rail_type, seg.osm_id);

    SegmentTrace {
        kind: LayerKind::Railway,
        osm_id: Some(seg.osm_id),
        segment_idx: seg.segment_idx,
        name: seg_name,
        subtype: rail_type.to_string(),
        is_dominant_of_group: false,
        start_lat: seg.start_lat,
        start_lon: seg.start_lon,
        end_lat: seg.end_lat,
        end_lon: seg.end_lon,
        cp_lat: seg.cp_lat,
        cp_lon: seg.cp_lon,
        length_m: seg.length_m as f64,
        dist_m: seg.dist_m,
        d_slant_m: d_slant,
        bridge: seg.bridge,
        tunnel: seg.tunnel,
        emission: EmissionTrace::Railway {
            traffic: seg.traffic,
            passenger_provenance: crate::sources::dataset_meta(seg.traffic.passenger.source_id),
            freight_provenance: crate::sources::dataset_meta(seg.traffic.freight.source_id),
            speed_kmh,
            bridge: seg.bridge,
            highspeed: seg.highspeed,
            rail_type,
            service: seg.service,
        },
        propagation: line_node_propagation(
            node,
            fan,
            d_slant,
            rcv_alt,
            reflection_boost_db,
            seg_variants,
            lw_bands,
            weather,
        ),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        cell_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
        sort_seq,
    }
}

pub(crate) fn build_road_segment_trace(inputs: BuildRoadTrace<'_>) -> SegmentTrace {
    let BuildRoadTrace {
        seg,
        class_name,
        rcv_alt,
        d_slant,
        reflection_boost_db,
        traffic,
        speed_kmh,
        surf_corr,
        node,
        fan,
        seg_variants,
        lw_bands,
        weather,
        sort_seq,
    } = inputs;

    let seg_name = seg_name_from_tags(&seg.road_ref, &seg.name, class_name, seg.osm_id);

    let emission = EmissionTrace::Road {
        aadt_light: traffic.light,
        aadt_medium: traffic.medium,
        aadt_heavy: traffic.heavy,
        aadt_moto: traffic.moto,
        traffic_estimated: traffic.estimated,
        speed_kmh,
        surface_corr_db: surf_corr,
        surface: crate::surface_name(seg.surface_type),
        source_id: seg.source_id,
        provenance: crate::sources::dataset_meta(seg.source_id),
        // Observed timing attribution, omitted when the segment has none.
        time_profile_attribution: seg.time_profile_attribution.clone(),
        road_class: class_name,
        bridge: seg.bridge,
        tunnel: seg.tunnel,
        oneway: seg.oneway,
        lanes: seg.lanes,
    };

    SegmentTrace {
        kind: LayerKind::Road,
        osm_id: Some(seg.osm_id),
        segment_idx: seg.segment_idx,
        name: seg_name,
        subtype: class_name.to_string(),
        is_dominant_of_group: false,
        start_lat: seg.start_lat,
        start_lon: seg.start_lon,
        end_lat: seg.end_lat,
        end_lon: seg.end_lon,
        cp_lat: seg.cp_lat,
        cp_lon: seg.cp_lon,
        length_m: seg.length_m as f64,
        dist_m: seg.dist_m,
        d_slant_m: d_slant,
        bridge: seg.bridge,
        tunnel: seg.tunnel,
        emission,
        propagation: line_node_propagation(
            node,
            fan,
            d_slant,
            rcv_alt,
            reflection_boost_db,
            seg_variants,
            lw_bands,
            weather,
        ),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        cell_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
        sort_seq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScreeningObstacleTrace;
    use serde::Serialize;

    // Guard regression: traces carry only per-band attenuation, no band[4] proxy.
    fn assert_bands_no_scalar<T: Serialize>(value: &T) {
        let json = serde_json::to_string(value).unwrap();
        assert!(json.contains("attenuation_bands"));
        assert!(!json.contains("attenuation_db_a"));
    }

    #[test]
    fn terrain_trace_shape() {
        let trace = TerrainTrace {
            delta_m: 0.0,
            attenuation_bands: [0.0; NUM_BANDS],
            edges: Vec::new(),
            delta_star_m: 0.0,
        };
        assert_bands_no_scalar(&trace);
    }

    #[test]
    fn screening_trace_shape() {
        let obstacle = ScreeningObstacleTrace::default();
        assert_bands_no_scalar(&screening_trace([0.0; NUM_BANDS], obstacle, None));
    }

    #[test]
    fn screening_fan_round_trips_through_json_wire() {
        let fan = ScreeningFanTrace {
            span_deg: 42.0,
            blocked_fraction: 0.25,
            intervals: vec![crate::types::ScreeningFanIntervalTrace {
                from_deg: -4.0,
                to_deg: 6.5,
                blocked: true,
                obstacle: Some(crate::types::ScreeningFanObstacleTrace {
                    kind: "building",
                    height_m: 8.0,
                }),
                terrain_db: 0.0,
                screen_db: 12.5,
                contains_cp: true,
            }],
            intervals_omitted: 0,
            omitted_fraction: 0.0,
            quadrature: "arc",
        };
        let encoded = serde_json::to_string(&screening_trace(
            [1.0; NUM_BANDS],
            ScreeningObstacleTrace::default(),
            Some(fan),
        ))
        .unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded["fan"]["quadrature"], "arc");
        assert_eq!(
            decoded["fan"]["intervals"][0]["obstacle"]["kind"],
            "building"
        );
        assert_eq!(decoded["fan"]["intervals"][0]["contains_cp"], true);
        assert!(decoded["fan"].get("intervals_omitted").is_none());
        assert!(decoded["fan"].get("omitted_fraction").is_none());

        let without_fan = serde_json::to_value(screening_trace(
            [0.0; NUM_BANDS],
            ScreeningObstacleTrace::default(),
            None,
        ))
        .unwrap();
        assert!(without_fan.get("fan").is_none());
    }

    #[test]
    fn vegetation_trace_shape() {
        assert_bands_no_scalar(&vegetation_trace([0.0; NUM_BANDS], Vec::new(), 0.0, 0.0));
    }

    #[test]
    fn ground_trace_shape() {
        assert_bands_no_scalar(&ground_trace(0.5, [0.0; NUM_BANDS]));
    }

    /// The atmospheric chart follows each period's own climate: with 1/2/3 dB/km
    /// day/evening/night, one kilometre reads 1/2/3 dB per band. A single day
    /// array cannot describe an evening or night row.
    #[test]
    fn atmospheric_chart_follows_each_period_climate() {
        use crate::propagation::air_absorption::AbsorptionClimate;
        let mut weather = Meteorology::defaults();
        weather.absorption = [
            [AbsorptionClimate::steady(1.0); NUM_BANDS],
            [AbsorptionClimate::steady(2.0); NUM_BANDS],
            [AbsorptionClimate::steady(3.0); NUM_BANDS],
        ];
        let bands = atmospheric_bands(1000.0, &weather);
        for band in 0..NUM_BANDS {
            assert_eq!(bands.day[band], 1.0);
            assert_eq!(bands.evening[band], 2.0);
            assert_eq!(bands.night[band], 3.0);
        }
    }

    /// The trace shows the kernel's own runs and depth (no second walk): a run's
    /// `len_m` sums to the total, geometry stays in t_start/t_end.
    #[test]
    fn vegetation_trace_shows_the_kernel_runs() {
        let runs = vec![
            ForestRun { t_start: 0.1, t_end: 0.4, len_m: 60.0 },
            ForestRun { t_start: 0.7, t_end: 0.8, len_m: 20.0 },
        ];
        let trace = vegetation_trace([1.0; NUM_BANDS], runs, 80.0, 2000.0);
        assert_eq!(trace.forest_depth_m, 80.0);
        assert_eq!(trace.sampled_path_m, 2000.0);
        assert_eq!(trace.forest_runs.len(), 2);
        let sum: f64 = trace.forest_runs.iter().map(|r| r.len_m).sum();
        assert_eq!(sum, trace.forest_depth_m);
    }
}
