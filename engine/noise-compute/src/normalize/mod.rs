//! Shared source normalization helpers used by popup and pipeline.
//!
//! Per-domain submodules turn raw OSM rows into emission-ready values:
//! [`road`] (line sources), [`rail`] (line sources), [`points`]
//! (building / industrial / leisure AREA point sources). The small
//! cross-domain helpers below ([`has_enriched_traffic`], [`bands_to_f32`],
//! [`resolve_area_m2`]) stay here so each domain shares one definition.

use crate::sources::Provenance;
use crate::types::NUM_BANDS;

mod points;
mod rail;
mod road;

pub use points::{
    prepare_building_points, prepare_industrial_points, prepare_leisure_points, PreparedPoint,
    RawBuildingInput, RawIndustrialInput, RawLeisureInput,
};
pub use rail::{normalize_rail, normalize_rail_segment, NormalizedRail, RawRailInput};
pub use road::{
    lane_ratio, nominal_road_aadt, normalize_road, normalize_road_segment,
    normalize_road_with_cache, NormalizedRoad, RawRoadInput,
};

/// `speed_limit` sentinel for OSM `maxspeed=none` (derestricted, e.g.
/// German Autobahn). Written by osm-extract (`spill.rs`), which clamps
/// real limits to 254 so they can never collide. Re-exported by
/// `osm-extract::classify` — this is the single definition.
pub const SPEED_LIMIT_DERESTRICTED: u8 = 255;

/// Effective emission speed for derestricted roads: BASt 2025 measured
/// a 124.1 km/h mean car speed on derestricted Autobahn sections, and
/// CNOSSOS-EU road emission is only valid up to its 130 km/h clamp
/// (see `road.rs`) — so model at the cap.
pub const DERESTRICTED_SPEED_KMH: f64 = 130.0;

/// Does this segment carry enriched traffic (any data from an enricher)
/// rather than a class default? Shared by `normalize_road` and
/// `nominal_road_aadt` so the "raw vs default" decision can't drift.
#[inline]
fn has_enriched_traffic(provenance: Provenance, aadt_light: i32) -> bool {
    provenance.has_data() && aadt_light > 0
}

fn bands_to_f32(bands: [f64; NUM_BANDS]) -> [f32; NUM_BANDS] {
    std::array::from_fn(|i| bands[i] as f32)
}

/// Resolve footprint area_m2 from the three sources the prep paths
/// consult, in priority order: caller-provided positive value (from
/// the arrow column) → snapped z30 ring shoelace
/// (`grid::poly::ring_area_m2`, when a ring is available) →
/// `default_m2` (per-source-type small/large fallback: 100 m² for
/// buildings, 10 000 m² for industrial sites). Apply the shared footprint floor
/// to stored values too, so existing sub-metre rows need no re-extract.
fn resolve_area_m2(provided: Option<f64>, polygon_grid: &[(i32, i32)], default_m2: f64) -> f64 {
    provided
        .filter(|a| *a > 0.0)
        .or_else(|| grid::poly::ring_area_m2(polygon_grid))
        .unwrap_or(default_m2)
        .max(grid::poly::MIN_FOOTPRINT_AREA_M2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared_area_sources(
        area_m2: Option<f64>,
        polygon_grid: &[(i32, i32)],
    ) -> [Vec<PreparedPoint>; 3] {
        [
            prepare_building_points(RawBuildingInput {
                centroid_lat: 0.0,
                centroid_lon: 0.0,
                height_m: 9.0,
                floors: 3,
                building_type: 0,
                area_m2,
                polygon_grid,
            }),
            prepare_industrial_points(RawIndustrialInput {
                centroid_lat: 0.0,
                centroid_lon: 0.0,
                source_type: 0,
                site_subtype: 0,
                hub_height_m: None,
                rated_power_kw: None,
                nace_4digit: None,
                area_m2,
                polygon_grid,
            }),
            prepare_leisure_points(RawLeisureInput {
                centroid_lat: 0.0,
                centroid_lon: 0.0,
                sport: crate::emission::leisure::PADEL,
                area_m2,
                polygon_grid,
            }),
        ]
    }

    #[test]
    fn stored_and_geometry_areas_preserve_the_one_square_metre_emission_floor() {
        let origin = 1 << 29;
        let degenerate = [(origin, origin), (origin + 1, origin), (origin + 2, origin)];
        // 26 × 13 z30 cells at the equator: a valid footprint below 1 m².
        let tiny = [
            (origin, origin),
            (origin + 26, origin),
            (origin + 26, origin + 13),
            (origin, origin + 13),
        ];
        for (case, provided, ring, expected_area) in [
            ("degenerate geometry", None, &degenerate[..], Some(1.0)),
            ("stored sub-metre area", Some(0.5), &[][..], Some(1.0)),
            (
                "stored sub-metre area with geometry",
                Some(0.5),
                &tiny[..],
                Some(1.0),
            ),
            ("geometry fallback", None, &tiny[..], Some(1.0)),
            (
                "zero falls back to geometry",
                Some(0.0),
                &tiny[..],
                Some(1.0),
            ),
            (
                "provided area retains priority",
                Some(25.0),
                &tiny[..],
                Some(25.0),
            ),
            ("ordinary stored area", Some(300.0), &[][..], Some(300.0)),
            ("missing area keeps source default", None, &[][..], None),
            (
                "zero without geometry keeps source default",
                Some(0.0),
                &[][..],
                None,
            ),
        ] {
            let actual = prepared_area_sources(provided, ring);
            let reference = prepared_area_sources(expected_area, &[]);
            for (kind, points) in actual.iter().enumerate() {
                let area = expected_area.unwrap_or([100.0, 10_000.0, 200.0][kind]);
                assert_eq!(points.len(), 1, "{case}, source {kind}");
                let point = &points[0];
                assert!(
                    point.lw_day.iter().all(|band| band.is_finite()),
                    "{case}, source {kind}: {point:?}"
                );
                assert_eq!(point.area_m2, area as f32, "{case}, source {kind}");
                assert_eq!(
                    point.lw_day, reference[kind][0].lw_day,
                    "{case}, source {kind}"
                );
                assert_eq!(
                    point.lw_evening, reference[kind][0].lw_evening,
                    "{case}, source {kind}"
                );
                assert_eq!(
                    point.lw_night, reference[kind][0].lw_night,
                    "{case}, source {kind}"
                );
                assert_eq!(
                    point.exclusion_radius_m, reference[kind][0].exclusion_radius_m,
                    "{case}, source {kind}"
                );
                assert_eq!(
                    point.max_radius_m, reference[kind][0].max_radius_m,
                    "{case}, source {kind}"
                );
            }
        }
        // The producer and geometry fallback share this exact area helper.
        assert_eq!(grid::poly::ring_area_m2(&tiny), Some(1.0));
        assert_eq!(grid::poly::ring_area_m2(&degenerate), Some(1.0));
        assert_eq!(grid::poly::ring_area_m2(&[]), None);
    }
}
