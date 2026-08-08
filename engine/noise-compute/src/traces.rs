//! Helpers for building per-segment popup traces from already-computed engine
//! state. Every number returned here must come from values the engine already
//! holds; this module never re-runs emission or propagation.

use crate::constants::ALPHA_ATM;
use crate::propagation::iso9613;
use crate::propagation::PathProfile;
use crate::types::{
    BaselineTrace, CnossosBreakdown, EmissionTrace, ForestRun, GroundTrace, LayerKind,
    LdenVariants, PathProfileTrace, PerPeriod, PointSource, PropagationBreakdown,
    PropagationVariants, RailSegment, RoadSegment, ScreeningObstacleTrace, ScreeningTrace,
    SegmentTrace, TerrainTrace, VegetationTrace, NUM_BANDS,
};
use crate::{building_type_name, industrial_type_name, rail_type_name};

mod aircraft;
pub use aircraft::{
    build_aircraft_airborne_subsegment_trace, build_aircraft_cruise_r7_trace,
    BuildAircraftAirborneSubSegmentTrace, BuildAircraftCruiseR7Trace,
};

/// Convert band-energies (linear, A-weighted) to band levels in dB(A).
pub fn bands_energy_to_db(bands: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|j| 10.0 * bands[j].max(1e-30).log10())
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

/// A-weighted dB(A) summary of a per-band *level* array (emission Lw or
/// received bands). Matches `iso9613::a_weighted_total` which is the same
/// aggregation the pipeline uses for totals.
pub fn bands_to_db_a(bands: &[f64; NUM_BANDS]) -> f64 {
    iso9613::a_weighted_total(bands)
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

/// Atmospheric attenuation (per band, positive = dB removed) for a given
/// slant distance. Reproduces the `ALPHA_ATM[i] * d/1000` term that
/// [`iso9613::propagate_variants`] applies internally.
pub fn atmospheric_bands(d_slant_m: f64) -> [f64; NUM_BANDS] {
    let d_over_1000 = d_slant_m / 1000.0;
    std::array::from_fn(|i| ALPHA_ATM[i] * d_over_1000)
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
        building_h_m: profile.building_h_m,
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

/// Contiguous forested intervals along a path plus their total depth.
/// `ForestRun.t_start/t_end` are fractional (0..1); `len_m` is the run's
/// DENSITY-WEIGHTED depth in metres (geodata-v2 2a: `Σ len × v/100` — equal
/// to the physical extent on binary rasters, the effective foliage metres
/// once continuous density tiles land; the frontend renders only the total
/// depth + run count and draws tufts from raw `forest_u8` samples — no
/// consumer reads `len_m` as geometry; gates stay `> 0`). The ≥10 m scattered-tree gate
/// stays on the PHYSICAL extent — the same threshold, mirror and parity
/// contract as `vegetation_run_length`; single pass so callers don't walk
/// the path twice.
pub fn vegetation_runs_and_depth(t: &[f64], forest: &[u8], dist_m: f64) -> (Vec<ForestRun>, f64) {
    if t.len() < 2 || forest.len() < 2 {
        return (Vec::new(), 0.0);
    }
    let mut runs = Vec::new();
    let mut total_depth = 0.0;
    let mut run_start_t: Option<f64> = None;
    let mut run_phys = 0.0;
    let mut run_weighted = 0.0;
    for i in 1..t.len() {
        let len = (t[i] - t[i - 1]) * dist_m;
        if forest[i] > 0 {
            if run_start_t.is_none() {
                run_start_t = Some(t[i - 1]);
            }
            run_phys += len;
            run_weighted += len * (forest[i] as f64 / 100.0);
        } else {
            if run_phys >= 10.0 {
                if let Some(start) = run_start_t {
                    runs.push(ForestRun {
                        t_start: start,
                        t_end: t[i - 1],
                        len_m: run_weighted,
                    });
                    total_depth += run_weighted;
                }
            }
            run_start_t = None;
            run_phys = 0.0;
            run_weighted = 0.0;
        }
    }
    if run_phys >= 10.0 {
        if let Some(start) = run_start_t {
            runs.push(ForestRun {
                t_start: start,
                t_end: t[t.len() - 1],
                len_m: run_weighted,
            });
            total_depth += run_weighted;
        }
    }
    (runs, total_depth)
}

/// Build a `ScreeningTrace` from the values already returned by
/// `screening_attenuation_with_meta`.
pub fn screening_trace(
    atten_bands: [f64; NUM_BANDS],
    obstacle: ScreeningObstacleTrace,
) -> ScreeningTrace {
    ScreeningTrace {
        attenuation_bands: atten_bands,
        obstacle: if obstacle.kind == "none" {
            None
        } else {
            Some(obstacle)
        },
    }
}

/// Build a `VegetationTrace` from per-band attenuation + forest sample arrays.
/// Computes forest depth (total) and runs on the way in so callers don't have
/// to walk the profile a second time.
pub fn vegetation_trace(
    atten_bands: [f64; NUM_BANDS],
    t: &[f64],
    forest_u8: &[u8],
    dist_m: f64,
) -> VegetationTrace {
    let (forest_runs, forest_depth_m) = vegetation_runs_and_depth(t, forest_u8, dist_m);
    VegetationTrace {
        forest_depth_m,
        sampled_path_m: dist_m,
        attenuation_bands: atten_bands,
        forest_runs,
    }
}

/// Build a `GroundTrace` from path-averaged ground factor G. Shares
/// [`crate::propagation::iso9613::ground_atten_db`] with the kernels, so the
/// popup's per-band ground row is the term the heatmap actually applied.
pub fn ground_trace(factor_g: f64) -> GroundTrace {
    let attenuation_bands = iso9613::ground_atten_bands(factor_g);
    GroundTrace {
        factor_g,
        attenuation_bands,
    }
}

/// Build a `BaselineTrace` from the slant distance, source height, ground G,
/// finite-line correction, and urban reflection boost. The reflection boost
/// is included here for trace-API completeness even though the internal
/// `free_field` variant (see iso9613.rs) excludes it; popup derives the
/// per-receiver A_refl display from this field directly.
pub fn baseline_trace(
    d_slant_m: f64,
    source_height_m: f64,
    ground_g: f64,
    finite_line_corr_db: f64,
    reflection_boost_db: f64,
    source_geometry: iso9613::SourceGeometry,
) -> BaselineTrace {
    let d = d_slant_m.max(1.0);
    let geometric_db = match source_geometry {
        iso9613::SourceGeometry::Line => 10.0 * (2.0 * std::f64::consts::PI * d).log10(),
        iso9613::SourceGeometry::Point => 20.0 * d.log10() + 11.0,
    };
    BaselineTrace {
        geometric_db,
        atmospheric_bands: atmospheric_bands(d_slant_m),
        ground_factor_g: ground_g,
        source_height_m,
        finite_line_corr_db,
        reflection_boost_db,
    }
}

/// Inputs for building a per-segment road trace. Keyword-struct style to avoid
/// a 30-argument function. Consumed (not borrowed) for heavy fields that can
/// be moved into the resulting SegmentTrace.
pub(crate) struct BuildRoadTrace<'a> {
    pub seg: &'a RoadSegment,
    pub class_name: &'static str,
    pub src_alt: f64,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub flc: f64,
    pub ground_g: f64,
    pub reflection_boost_db: f64,
    pub light: f64,
    pub medium: f64,
    pub heavy: f64,
    pub moto: f64,
    pub speed_kmh: f64,
    pub surf_corr: f64,
    pub path_profile: PathProfile,
    pub terrain: TerrainTrace,
    pub screening_atten: [f64; NUM_BANDS],
    pub obstacle_trace: ScreeningObstacleTrace,
    pub veg_atten: [f64; NUM_BANDS],
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
}

pub(crate) struct BuildPointTrace<'a> {
    pub src: &'a PointSource,
    pub source_kind: LayerKind,
    pub src_alt: f64,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub prop_dist: f64,
    pub ground_g: f64,
    pub reflection_boost_db: f64,
    pub path_profile: PathProfile,
    pub terrain: TerrainTrace,
    pub screening_atten: [f64; NUM_BANDS],
    pub obstacle_trace: ScreeningObstacleTrace,
    pub veg_atten: [f64; NUM_BANDS],
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
}

pub(crate) fn build_point_segment_trace(inputs: BuildPointTrace<'_>) -> SegmentTrace {
    let BuildPointTrace {
        src,
        source_kind,
        src_alt,
        rcv_alt,
        d_slant,
        prop_dist,
        ground_g,
        reflection_boost_db,
        path_profile,
        terrain,
        screening_atten,
        obstacle_trace,
        veg_atten,
        seg_variants,
        lw_bands,
    } = inputs;

    let (subtype_label, emission) = match source_kind {
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
                // `src.source_height_m` for buildings is the mid-facade
                // anchor (= building_height / 2, per CNOSSOS-EU §2.5.5
                // and `prepare_building_points`). The popup user sees a
                // row labeled "Height" so present the FULL building
                // height by doubling. The mid-facade source_height_m
                // remains available on `baseline.source_height_m` for
                // the ISO 9613-2 anchor display.
                EmissionTrace::Building {
                    building_type: label,
                    height_m: src.source_height_m * 2.0,
                    floors: src.floors,
                    area_m2: src.area_m2 as f64,
                },
            )
        }
    };

    let vegetation = vegetation_trace(
        veg_atten,
        &path_profile.t,
        &path_profile.forest_u8,
        path_profile.dist_m,
    );

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
        propagation: PropagationBreakdown::Cnossos(Box::new(CnossosBreakdown {
            baseline: baseline_trace(
                d_slant,
                src_alt,
                ground_g,
                0.0,
                reflection_boost_db,
                iso9613::SourceGeometry::Point,
            ),
            path_profile: path_profile_into_trace(path_profile, src_alt, rcv_alt),
            terrain,
            screening: screening_trace(screening_atten, obstacle_trace),
            vegetation,
            ground: ground_trace(ground_g),
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
            received_bands: variants_to_received_bands(&seg_variants),
        })),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        hex_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
    }
}

pub(crate) struct BuildRailTrace<'a> {
    pub seg: &'a RailSegment,
    pub src_alt: f64,
    pub rcv_alt: f64,
    pub d_slant: f64,
    pub flc: f64,
    pub ground_g: f64,
    pub reflection_boost_db: f64,
    pub q_pax: f64,
    pub q_frt: f64,
    pub speed_kmh: f64,
    pub path_profile: PathProfile,
    pub terrain: TerrainTrace,
    pub screening_atten: [f64; NUM_BANDS],
    pub obstacle_trace: ScreeningObstacleTrace,
    pub veg_atten: [f64; NUM_BANDS],
    pub seg_variants: [PropagationVariants; 3],
    pub lw_bands: [[f64; NUM_BANDS]; 3],
}

pub(crate) fn build_rail_segment_trace(inputs: BuildRailTrace<'_>) -> SegmentTrace {
    let BuildRailTrace {
        seg,
        src_alt,
        rcv_alt,
        d_slant,
        flc,
        ground_g,
        reflection_boost_db,
        q_pax,
        q_frt,
        speed_kmh,
        path_profile,
        terrain,
        screening_atten,
        obstacle_trace,
        veg_atten,
        seg_variants,
        lw_bands,
    } = inputs;

    let rail_type = rail_type_name(seg.rail_type);
    let seg_name = seg_name_from_tags(&seg.rail_ref, &seg.name, rail_type, seg.osm_id);

    let vegetation = vegetation_trace(
        veg_atten,
        &path_profile.t,
        &path_profile.forest_u8,
        path_profile.dist_m,
    );

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
            trains_passenger: q_pax,
            trains_freight: q_frt,
            trains_passenger_source: if seg.trains_passenger_source == 0 {
                "arrow"
            } else {
                "default_by_type"
            },
            trains_freight_source: if seg.trains_freight_source == 0 {
                "arrow"
            } else {
                "default_by_type"
            },
            source_id: seg.source_id,
            provenance: crate::sources::dataset_meta(seg.source_id),
            speed_kmh,
            bridge: seg.bridge,
            highspeed: seg.highspeed,
            rail_type,
            service: seg.service,
        },
        propagation: PropagationBreakdown::Cnossos(Box::new(CnossosBreakdown {
            baseline: baseline_trace(
                d_slant,
                src_alt,
                ground_g,
                flc,
                reflection_boost_db,
                iso9613::SourceGeometry::Line,
            ),
            path_profile: path_profile_into_trace(path_profile, src_alt, rcv_alt),
            terrain,
            screening: screening_trace(screening_atten, obstacle_trace),
            vegetation,
            ground: ground_trace(ground_g),
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
            received_bands: variants_to_received_bands(&seg_variants),
        })),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        hex_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
    }
}

pub(crate) fn build_road_segment_trace(inputs: BuildRoadTrace<'_>) -> SegmentTrace {
    let BuildRoadTrace {
        seg,
        class_name,
        src_alt,
        rcv_alt,
        d_slant,
        flc,
        ground_g,
        reflection_boost_db,
        light,
        medium,
        heavy,
        moto,
        speed_kmh,
        surf_corr,
        path_profile,
        terrain,
        screening_atten,
        obstacle_trace,
        veg_atten,
        seg_variants,
        lw_bands,
    } = inputs;

    let seg_name = seg_name_from_tags(&seg.road_ref, &seg.name, class_name, seg.osm_id);

    let provenance = crate::sources::provenance_of(seg.source_id);
    let traffic_source = if provenance.has_data() && seg.aadt_light > 0 {
        provenance.legacy_traffic_source_str()
    } else {
        "default_by_class"
    };
    let emission = EmissionTrace::Road {
        aadt_light: light,
        aadt_medium: medium,
        aadt_heavy: heavy,
        aadt_moto: moto,
        speed_kmh,
        surface_corr_db: surf_corr,
        surface: crate::surface_name(seg.surface_type),
        traffic_source,
        source_id: seg.source_id,
        provenance: crate::sources::dataset_meta(seg.source_id),
        road_class: class_name,
        bridge: seg.bridge,
        tunnel: seg.tunnel,
        oneway: seg.oneway,
        lanes: seg.lanes,
    };

    let vegetation = vegetation_trace(
        veg_atten,
        &path_profile.t,
        &path_profile.forest_u8,
        path_profile.dist_m,
    );

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
        propagation: PropagationBreakdown::Cnossos(Box::new(CnossosBreakdown {
            baseline: baseline_trace(
                d_slant,
                src_alt,
                ground_g,
                flc,
                reflection_boost_db,
                iso9613::SourceGeometry::Line,
            ),
            path_profile: path_profile_into_trace(path_profile, src_alt, rcv_alt),
            terrain,
            screening: screening_trace(screening_atten, obstacle_trace),
            vegetation,
            ground: ground_trace(ground_g),
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
            received_bands: variants_to_received_bands(&seg_variants),
        })),
        received_lden: variants_to_lden(&seg_variants),
        aircraft_subtype: 0,
        polyline: None,
        hex_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
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
        let obstacle = ScreeningObstacleTrace {
            kind: "none",
            ..Default::default()
        };
        assert_bands_no_scalar(&screening_trace([0.0; NUM_BANDS], obstacle));
    }

    #[test]
    fn vegetation_trace_shape() {
        assert_bands_no_scalar(&vegetation_trace([0.0; NUM_BANDS], &[], &[], 0.0));
    }

    #[test]
    fn ground_trace_shape() {
        assert_bands_no_scalar(&ground_trace(0.5));
    }

    /// The popup trace accumulator mirrors `vegetation_run_length` exactly:
    /// total depth equal on binary AND continuous inputs, and each run's
    /// `len_m` carries the density-weighted depth (physical geometry stays
    /// in t_start/t_end).
    #[test]
    fn vegetation_runs_mirror_run_length() {
        use crate::propagation::path_profile::vegetation_run_length;
        let t: Vec<f64> = (0..=10).map(|i| i as f64 / 10.0).collect();
        for vals in [
            vec![0u8, 100, 100, 0, 0, 100, 100, 100, 0, 40, 40],
            vec![0u8, 40, 40, 40, 0, 0, 100, 0, 0, 0, 0],
        ] {
            let (runs, depth) = vegetation_runs_and_depth(&t, &vals, 2000.0);
            let expected = vegetation_run_length(&t, &vals, 2000.0);
            // identical arithmetic in identical order ⇒ bit-equal, not just close
            assert!(
                depth == expected,
                "trace depth {depth} != kernel depth {expected}"
            );
            let sum: f64 = runs.iter().map(|r| r.len_m).sum();
            assert!(sum == depth);
        }
    }
}
