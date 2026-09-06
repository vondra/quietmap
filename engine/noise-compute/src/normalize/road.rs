//! Road source normalization: raw OSM road inputs → per-band emission-ready
//! values (`NormalizedRoad`), plus the class-default cascade, lane/access
//! scaling, and the nominal-AADT surface the popup reports.

use crate::admin::Admin;
use crate::constants::{SOURCE_HEIGHT_ROAD, SURFACE_CORR};
use crate::defaults::{
    build_traffic_default_cache, resolve_speed_default, resolve_traffic_default, Aadt,
    WORLD_DEFAULT,
};
use crate::emission::road;
use crate::sources::{provenance_of, Provenance};
use crate::types::{RoadSegment, NUM_BANDS};

use super::{bands_to_f32, has_enriched_traffic, DERESTRICTED_SPEED_KMH, SPEED_LIMIT_DERESTRICTED};

#[derive(Debug, Clone, Copy)]
pub struct RawRoadInput {
    pub road_class: u8,
    /// km/h; 0 = untagged (class default), [`SPEED_LIMIT_DERESTRICTED`]
    /// = `maxspeed=none` → [`DERESTRICTED_SPEED_KMH`].
    pub speed_limit: u8,
    /// R7 taper graded effective speed (km/h, 0 = none) — consulted only when
    /// `speed_limit` is 0; ranks above the legal/class default it refines.
    pub speed_taper: u8,
    pub surface_type: u8,
    pub oneway: bool,
    pub lanes: u8,
    pub aadt_light: i32,
    pub aadt_medium: i32,
    pub aadt_heavy: i32,
    pub aadt_moto: i32,
    /// Provenance of the AADT values — derived from the row's
    /// dataset id via `sources::provenance_of()`.
    pub provenance: Provenance,
    pub tunnel: bool,
    pub access: u8,
    pub junction: u8,
    /// Vector-footprint density for the legal speed default of untagged roads:
    /// 0 unknown, 1 rural, 2 urban.
    pub built_up: u8,
}

#[derive(Debug, Clone)]
pub struct NormalizedRoad {
    pub class_idx: usize,
    pub class_name: &'static str,
    pub max_distance_m: f64,
    pub source_height_m: f64,
    /// Speed after junction cap (≤30 km/h at roundabouts).
    pub speed_kmh: f64,
    /// Speed before junction cap — equals `speed_kmh` for non-junction segments.
    /// Callers compare the two to tell whether the roundabout cap fired.
    pub base_speed_kmh: f64,
    pub surf_corr_db: f64,
    pub light_aadt: f64,
    pub medium_aadt: f64,
    pub heavy_aadt: f64,
    pub moto_aadt: f64,
}

impl NormalizedRoad {
    pub fn time_dist(&self) -> &'static road::TimeDist {
        // motorway (0) + trunk (1) + their ramps (10, 11) share the motorway
        // day/evening/night split; everything else uses the urban split.
        match self.class_idx {
            0 | 1 | 10 | 11 => &road::TIME_DIST_MOTORWAY,
            _ => &road::TIME_DIST_URBAN,
        }
    }

    pub fn period_emission(&self, period_pct: f64, period_hours: f64) -> [f32; NUM_BANDS] {
        let flows = road::build_period_flows(
            self.light_aadt,
            self.medium_aadt,
            self.heavy_aadt,
            self.moto_aadt,
            self.speed_kmh,
            period_pct,
            period_hours,
        );
        bands_to_f32(road::line_source_emission(&flows, self.surf_corr_db))
    }

    pub fn period_emissions(&self) -> ([f32; NUM_BANDS], [f32; NUM_BANDS], [f32; NUM_BANDS]) {
        let td = self.time_dist();
        (
            self.period_emission(td.day_pct, 12.0),
            self.period_emission(td.evening_pct, 4.0),
            self.period_emission(td.night_pct, 8.0),
        )
    }
}

/// Normalise a raw road input into per-band emission-ready values.
///
/// `admin` drives the class-default cascade (city → country → continent →
/// world) when the row lacks enriched traffic. Callers that do not have
/// an admin context yet — or that legitimately want the world arm — pass
/// [`Admin::UNKNOWN`]; the cascade collapses to `WORLD_DEFAULT` in that
/// case, matching the pre-Phase-0.5 behaviour bit-for-bit.
///
/// Tight loops should prefer [`normalize_road_with_cache`] together with
/// [`build_traffic_default_cache`] so the city → country → continent →
/// world cascade only runs 13 times per (admin, batch) instead of once
/// per `source_id == 0` segment. This wrapper builds a fresh cache on
/// every call — cheap (13 tuples) but wasteful when the same `admin`
/// is reused.
pub fn normalize_road(input: RawRoadInput, admin: Admin) -> Option<NormalizedRoad> {
    let cache = build_traffic_default_cache(admin);
    normalize_road_with_cache(input, admin, &cache)
}

/// Cache-aware variant of [`normalize_road`]. The 13-entry slice is
/// produced once per (admin, batch) by [`build_traffic_default_cache`],
/// after which class-default lookup is a single array index instead of
/// up to four hash-map / binary-search hops through the cascade.
pub fn normalize_road_with_cache(
    input: RawRoadInput,
    admin: Admin,
    defaults_cache: &[Aadt; WORLD_DEFAULT.len()],
) -> Option<NormalizedRoad> {
    // access=no / motor_vehicle=no drops a segment ONLY when no MEASUREMENT
    // contradicts the tag: a national census counting thousands of vehicles on
    // an access=no primary (Neratovice "Nádražní", owner case 2026-07-10) is
    // proof the flattened OSM tag hides an exception (bus=yes, destination
    // modifiers, temporary closures) — measured reality outranks tag
    // interpretation, mirroring access_factor's measured pass-through.
    // Deliberately is_measured, not has_data: a heuristic estimate (service
    // tree, taper) is a guess and must not resurrect a legally closed road.
    // The aadt_light > 0 guard is load-bearing: a measured-tier stamp with
    // zero light traffic would otherwise fall through to CLASS DEFAULTS
    // downstream (has_enriched_traffic is false) — full mainline noise on an
    // empty closed road. `is_measured` currently describes the provenience
    // tier, not the row's counted-vs-derived method: mixed high-priority
    // datasets may therefore pass this exception. Splitting that distinction
    // belongs in the registry/writer contract, not in this engine gate.
    // Tunnels stay dropped unconditionally (the emission is underground).
    if input.tunnel
        || (matches!(input.access, 2 | 4)
            && !(input.provenance.is_measured() && input.aadt_light > 0))
    {
        return None;
    }

    let class_idx = (input.road_class as usize).min(ROAD_CLASS_NAMES.len() - 1);
    let class_name = ROAD_CLASS_NAMES[class_idx];
    let oneway_factor = if input.oneway { 0.5 } else { 1.0 };
    let access_factor = access_factor(input.access, input.provenance, input.road_class);

    let (light_aadt, medium_aadt, heavy_aadt, moto_aadt) =
        if has_enriched_traffic(input.provenance, input.aadt_light) {
            (
                input.aadt_light as f64 * oneway_factor * access_factor,
                input.aadt_medium as f64 * oneway_factor * access_factor,
                input.aadt_heavy as f64 * oneway_factor * access_factor,
                input.aadt_moto as f64 * oneway_factor * access_factor,
            )
        } else {
            let defaults =
                defaults_cache[(input.road_class as usize).min(defaults_cache.len() - 1)];
            let factor =
                oneway_factor * access_factor * lane_ratio(class_idx, input.lanes, input.oneway);
            (
                defaults.0 * factor,
                defaults.1 * factor,
                defaults.2 * factor,
                defaults.3 * factor,
            )
        };

    let base_speed_kmh = if input.speed_limit == SPEED_LIMIT_DERESTRICTED {
        DERESTRICTED_SPEED_KMH
    } else if input.speed_limit > 0 {
        input.speed_limit as f64
    } else if input.speed_taper > 0 {
        // R7 taper: a graded effective speed at a junction-free step — a
        // refinement of the default the row would otherwise get, so it ranks
        // between the OSM tag (real law) and the legal/class default.
        input.speed_taper as f64
    } else {
        // Untagged: the country's legal implicit limit, selected by vector-footprint
        // density, beats the global class default. A tagged/untagged boundary
        // mid-road otherwise painted a ±5–6 dB colour seam (Wetherby, task #15).
        // Unknown country or density uses the class default.
        resolve_speed_default(input.road_class, admin, input.built_up)
            .unwrap_or_else(|| default_road_speed(class_idx))
    };
    let speed_kmh = if input.junction == 1 {
        base_speed_kmh.min(30.0)
    } else {
        base_speed_kmh
    };
    let surf_corr_db = SURFACE_CORR
        .get(input.surface_type as usize)
        .copied()
        .unwrap_or(0.0);

    Some(NormalizedRoad {
        class_idx,
        class_name,
        max_distance_m: ROAD_MAX_DIST[class_idx],
        source_height_m: SOURCE_HEIGHT_ROAD,
        speed_kmh,
        base_speed_kmh,
        surf_corr_db,
        light_aadt,
        medium_aadt,
        heavy_aadt,
        moto_aadt,
    })
}

pub fn normalize_road_segment(seg: &RoadSegment, admin: Admin) -> Option<NormalizedRoad> {
    normalize_road(
        RawRoadInput {
            road_class: seg.road_class,
            speed_limit: seg.speed_limit,
            speed_taper: seg.speed_taper,
            surface_type: seg.surface_type,
            oneway: seg.oneway,
            lanes: seg.lanes,
            aadt_light: seg.aadt_light,
            aadt_medium: seg.aadt_medium,
            aadt_heavy: seg.aadt_heavy,
            aadt_moto: seg.aadt_moto,
            provenance: provenance_of(seg.source_id),
            tunnel: seg.tunnel,
            access: seg.access,
            junction: seg.junction,
            built_up: seg.built_up,
        },
        admin,
    )
}

/// Lane-based AADT scaling ratio for un-enriched roads.
///
/// Source: CZ ŘSD Celostátní sčítání dopravy 2020. Median totals over
/// 3 197 deduplicated census sections (one section ≈ many OSM ways)
/// bucketed by `(class × lanes × oneway)`; each arm is
/// `bucket_median / 2-lane-baseline_median`. Producer: `pipeline/
/// calibrate-lane-ratios.ts` — re-run against current enriched arrows
/// to regenerate the table. Only buckets with N ≥ 30 sections emit an
/// arm, so trunk and tertiary stay at 1.0 (sample too thin). `min()`
/// clamps saturate higher lane counts at the highest calibrated
/// bucket. Enriched rows (Provenance::*Measured) bypass this whole
/// path — they carry the observed AADT directly.
pub fn lane_ratio(class_idx: usize, lanes: u8, oneway: bool) -> f64 {
    if lanes <= 2 || class_idx >= 5 {
        return 1.0;
    }
    match (class_idx, oneway) {
        (0, true) => match lanes.min(3) {
            3 => 1.42,
            _ => 1.0,
        },
        (2, false) => match lanes.min(4) {
            3 => 1.37,
            4 => 2.13,
            _ => 1.0,
        },
        (3, false) => match lanes.min(3) {
            3 => 1.83,
            _ => 1.0,
        },
        _ => 1.0,
    }
}

/// AADT multiplier per OSM access code. Measured provenance
/// (national / continental / global) passes through unchanged — the
/// observation already reflects the restriction.
///
/// Codes 7/8 (agricultural/forestry) are not literature-backed — they're
/// conservative heuristics reflecting that such tracks mostly carry single-digit
/// daily vehicle counts. Permissive sits at 0.9 because real-world permissive is
/// bimodal (fully open service roads vs. gated holiday-resort roads).
///
/// 94.7 % of class-8 (track) segments in the world OSM extract have
/// `access=0` (untagged). Empirically, an untagged gravel track carries
/// about the same load as an explicitly agricultural one, so we fold
/// `(class=8, access=0)` into the agricultural arm (0.1×) instead of
/// leaving it at the full class-8 default. This drops effective traffic
/// on untagged tracks to ~0.5/day — matches the reality at e.g. Kytín
/// "alej loupežníka Babinského".
fn access_factor(access: u8, provenance: Provenance, road_class: u8) -> f64 {
    if provenance.is_measured() {
        return 1.0;
    }
    // A.5: untagged (access=0) class-8 track → implicit agricultural.
    if road_class == 8 && access == 0 {
        return 0.1;
    }
    match access {
        1 => 0.1,  // private
        3 => 0.5,  // destination (local traffic only)
        5 => 0.9,  // permissive (bimodal — see docstring)
        6 => 0.3,  // customers (shop/parking approach; short-trip traffic only)
        7 => 0.1,  // agricultural (tractor on field track; heuristic)
        8 => 0.08, // forestry (even fewer than agricultural; heuristic)
        _ => 1.0,  // 0=yes/untagged; codes 2/4 already dropped in normalize_road
    }
}

/// Nominal (pre-factor) AADT for a segment — the arrow's raw number if
/// traffic is enriched (any provenance except `None`, `aadt_light > 0`),
/// otherwise the class default resolved through the admin cascade. This
/// is the "road total, both directions" number the popup surfaces,
/// independent of per-OSM-way oneway halving and per-segment access /
/// lane-ratio factors. `admin` should be the receiver-square admin already
/// computed by the caller; `Admin::UNKNOWN` falls through to WORLD which
/// silently under-reports for places like Bangkok / São Paulo where the
/// city or country tier is meaningfully higher.
pub fn nominal_road_aadt(
    road_class: u8,
    provenance: Provenance,
    aadt_light: i32,
    aadt_medium: i32,
    aadt_heavy: i32,
    aadt_moto: i32,
    admin: Admin,
) -> (f64, f64, f64, f64) {
    if has_enriched_traffic(provenance, aadt_light) {
        (
            aadt_light as f64,
            aadt_medium as f64,
            aadt_heavy as f64,
            aadt_moto as f64,
        )
    } else {
        resolve_traffic_default(road_class, admin)
    }
}

fn default_road_speed(class_idx: usize) -> f64 {
    match class_idx {
        0 => 100.0,
        1 => 70.0,
        2 => 50.0,
        3 => 50.0,
        4 => 50.0,
        5 => 30.0,
        6 => 20.0,
        7 => 20.0,  // service
        8 => 20.0,  // track
        9 => 50.0,  // unclassified (rural typical)
        10 => 60.0, // motorway_link
        11 => 50.0, // trunk_link
        _ => 50.0,  // 12 primary_link (+ defensive fallback)
    }
}

const ROAD_CLASS_NAMES: [&str; 13] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "residential",
    "living_street",
    "service",
    "track",
    "unclassified",
    "motorway_link",
    "trunk_link",
    "primary_link",
];

const ROAD_MAX_DIST: [f64; 13] = crate::constants::ROAD_MAX_RADIUS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_factor_rules() {
        use Provenance::{Heuristic, NationalMeasured, NationalProxy, None as Prov_None};
        // Non-track classes (5 residential) — default behaviour.
        assert_eq!(access_factor(0, Prov_None, 5), 1.0); // yes + default
        assert_eq!(access_factor(1, Prov_None, 5), 0.1); // private + default
        assert_eq!(access_factor(3, Prov_None, 5), 0.5); // destination + default
        assert_eq!(access_factor(5, Prov_None, 5), 0.9); // permissive + default
        assert_eq!(access_factor(6, Prov_None, 5), 0.3); // customers + default
        assert_eq!(access_factor(7, Prov_None, 5), 0.1); // agricultural + default
        assert_eq!(access_factor(8, Prov_None, 5), 0.08); // forestry + default
        assert_eq!(access_factor(3, Heuristic, 5), 0.5); // destination + service-tree heuristic
        assert_eq!(access_factor(3, NationalMeasured, 5), 1.0); // destination + measured → pass-through
        assert_eq!(access_factor(1, NationalMeasured, 5), 1.0); // private + measured → pass-through
        assert_eq!(access_factor(7, NationalMeasured, 5), 1.0); // agricultural + measured → pass-through
                                                                // A proxy is an ESTIMATE, not a measurement — it must still be down-scaled on
                                                                // restricted roads (gg 2026-06-14). Public roads are unaffected (factor 1.0).
        assert_eq!(access_factor(1, NationalProxy, 5), 0.1); // private + proxy → down-scaled
        assert_eq!(access_factor(7, NationalProxy, 5), 0.1); // agricultural + proxy → down-scaled
        assert_eq!(access_factor(0, NationalProxy, 5), 1.0); // public + proxy → unchanged
                                                             // A.5: untagged class-8 track gets implicit agricultural factor.
        assert_eq!(access_factor(0, Prov_None, 8), 0.1); // untagged track → implicit ag
        assert_eq!(access_factor(0, Prov_None, 5), 1.0); // untagged residential → unchanged
        assert_eq!(access_factor(0, NationalMeasured, 8), 1.0); // measured track → pass-through
    }

    /// access=no drops a defaults-only segment, but a MEASURED count proves
    /// traffic exists (Neratovice case) — the segment must emit; heuristic
    /// stamps must not resurrect it.
    #[test]
    fn access_no_yields_to_measured_traffic_only() {
        let base = RawRoadInput {
            road_class: 2,
            speed_limit: 50,
            speed_taper: 0,
            surface_type: 0,
            oneway: false,
            lanes: 0,
            aadt_light: 0,
            aadt_medium: 0,
            aadt_heavy: 0,
            aadt_moto: 0,
            provenance: Provenance::None,
            tunnel: false,
            access: 2,
            junction: 0,
            built_up: 0,
        };
        assert!(
            normalize_road(base, Admin::UNKNOWN).is_none(),
            "unmeasured access=no drops"
        );

        let measured = RawRoadInput {
            aadt_light: 8824,
            aadt_medium: 516,
            aadt_heavy: 1497,
            provenance: Provenance::NationalMeasured,
            ..base
        };
        let road = normalize_road(measured, Admin::UNKNOWN).expect("measured access=no emits");
        assert!(
            (road.light_aadt - 8824.0).abs() < 1e-9,
            "census AADT passes through"
        );

        let heuristic = RawRoadInput {
            aadt_light: 500,
            provenance: Provenance::Heuristic,
            ..base
        };
        assert!(
            normalize_road(heuristic, Admin::UNKNOWN).is_none(),
            "a heuristic guess must not resurrect a closed road"
        );
        let proxy = RawRoadInput {
            aadt_light: 500,
            provenance: Provenance::NationalProxy,
            ..base
        };
        assert!(
            normalize_road(proxy, Admin::UNKNOWN).is_none(),
            "a national proxy estimate must not resurrect a closed road"
        );

        // The aadt_light > 0 conjunct is load-bearing: measured-tier stamp with
        // zero light traffic (heavy-only industrial gate) must stay dropped —
        // it would otherwise fall through to full class defaults downstream.
        let heavy_only = RawRoadInput {
            aadt_light: 0,
            aadt_heavy: 500,
            provenance: Provenance::NationalMeasured,
            ..base
        };
        assert!(
            normalize_road(heavy_only, Admin::UNKNOWN).is_none(),
            "measured heavy-only with zero light traffic stays dropped (fail closed)"
        );

        // motor_vehicle=no (code 4) follows the same gate as access=no (2).
        let mvno = RawRoadInput {
            access: 4,
            ..measured
        };
        assert!(
            normalize_road(mvno, Admin::UNKNOWN).is_some(),
            "code 4 + measured emits"
        );

        let tunnel = RawRoadInput {
            tunnel: true,
            ..measured
        };
        assert!(
            normalize_road(tunnel, Admin::UNKNOWN).is_none(),
            "tunnels stay dropped"
        );
    }

    #[test]
    fn road_defaults_match_tertiary_case() {
        let road = normalize_road(
            RawRoadInput {
                road_class: 4,
                speed_limit: 90,
                speed_taper: 0,
                surface_type: 0,
                oneway: false,
                lanes: 0,
                aadt_light: 0,
                aadt_medium: 0,
                aadt_heavy: 0,
                aadt_moto: 0,
                provenance: Provenance::None,
                tunnel: false,
                access: 0,
                junction: 0,
                built_up: 0,
            },
            Admin::UNKNOWN,
        )
        .unwrap();
        assert_eq!(road.class_name, "tertiary");
        assert!((road.light_aadt - 720.0).abs() < 1e-9);
        assert!((road.medium_aadt - 26.0).abs() < 1e-9);
        assert!((road.heavy_aadt - 38.0).abs() < 1e-9);
        assert!((road.moto_aadt - 16.0).abs() < 1e-9);
        assert!((road.speed_kmh - 90.0).abs() < 1e-9);
    }

    /// `maxspeed=none` sentinel (255) resolves to the derestricted model
    /// speed, not a literal 255 km/h emission input.
    #[test]
    fn derestricted_sentinel_resolves_to_130() {
        let road = normalize_road(
            RawRoadInput {
                road_class: 0,
                speed_limit: SPEED_LIMIT_DERESTRICTED,
                speed_taper: 0,
                surface_type: 0,
                oneway: false,
                lanes: 0,
                aadt_light: 0,
                aadt_medium: 0,
                aadt_heavy: 0,
                aadt_moto: 0,
                provenance: Provenance::None,
                tunnel: false,
                access: 0,
                junction: 0,
                built_up: 0,
            },
            Admin::UNKNOWN,
        )
        .unwrap();
        assert_eq!(road.base_speed_kmh, DERESTRICTED_SPEED_KMH);
        assert_eq!(road.speed_kmh, DERESTRICTED_SPEED_KMH);
    }

    #[test]
    fn built_up_selects_cz_legal_speed_below_explicit_tags_and_taper() {
        let cz = Admin {
            country_iso: *b"CZ",
            continent: crate::admin::Continent::Europe,
            city_id: 0,
        };
        for (road_class, built_up, speed_limit, speed_taper, expected) in [
            (3, 1, 0, 0, 90.0),
            (3, 2, 0, 0, 50.0),
            (3, 0, 0, 0, 50.0),
            (3, 2, 70, 40, 70.0),
            (3, 1, 0, 40, 40.0),
            (5, 2, 0, 0, 30.0),
            (7, 1, 0, 0, 20.0),
        ] {
            let input = RawRoadInput {
                road_class,
                built_up,
                speed_limit,
                speed_taper,
                surface_type: 0,
                oneway: false,
                lanes: 0,
                aadt_light: 0,
                aadt_medium: 0,
                aadt_heavy: 0,
                aadt_moto: 0,
                provenance: Provenance::None,
                tunnel: false,
                access: 0,
                junction: 0,
            };
            assert_eq!(normalize_road(input, cz).unwrap().speed_kmh, expected);
        }
    }

    #[test]
    fn ramp_defaults_are_15_percent_of_mainline() {
        // motorway_link (10) = 15 % of motorway (0)
        let (l0, m0, h0, x0) = resolve_traffic_default(0, Admin::UNKNOWN);
        let (l10, m10, h10, x10) = resolve_traffic_default(10, Admin::UNKNOWN);
        assert!((l10 - l0 * 0.15).abs() < 1e-6);
        assert!((m10 - m0 * 0.15).abs() < 1e-6);
        assert!((h10 - h0 * 0.15).abs() < 1e-6);
        assert!((x10 - x0 * 0.15).abs() < 1e-6);

        // trunk_link (11) = 15 % of trunk (1)
        let (l1, m1, h1, _) = resolve_traffic_default(1, Admin::UNKNOWN);
        let (l11, m11, h11, _) = resolve_traffic_default(11, Admin::UNKNOWN);
        assert!((l11 - l1 * 0.15).abs() < 1e-6);
        assert!((m11 - m1 * 0.15).abs() < 1e-6);
        assert!((h11 - h1 * 0.15).abs() < 1e-6);

        // primary_link (12) = 15 % of primary (2)
        let (l2, m2, h2, _) = resolve_traffic_default(2, Admin::UNKNOWN);
        let (l12, m12, h12, _) = resolve_traffic_default(12, Admin::UNKNOWN);
        assert!((l12 - l2 * 0.15).abs() < 1e-6);
        assert!((m12 - m2 * 0.15).abs() < 1e-6);
        assert!((h12 - h2 * 0.15).abs() < 1e-6);
    }

    #[test]
    fn ramp_speed_lower_than_mainline() {
        assert!(default_road_speed(10) < default_road_speed(0)); // motorway_link < motorway
        assert!(default_road_speed(11) < default_road_speed(1)); // trunk_link <= trunk
        assert!(default_road_speed(12) == default_road_speed(2)); // primary_link same as primary
    }

    #[test]
    fn ramp_time_dist_matches_motorway() {
        // Classes 10/11 must use TIME_DIST_MOTORWAY (65/20/15), not TIME_DIST_URBAN.
        let make = |class_idx: usize| NormalizedRoad {
            class_idx,
            class_name: "",
            max_distance_m: 0.0,
            source_height_m: 0.0,
            speed_kmh: 50.0,
            base_speed_kmh: 50.0,
            surf_corr_db: 0.0,
            light_aadt: 0.0,
            medium_aadt: 0.0,
            heavy_aadt: 0.0,
            moto_aadt: 0.0,
        };
        let motorway = make(0).time_dist();
        assert!(std::ptr::eq(make(10).time_dist(), motorway));
        assert!(std::ptr::eq(make(11).time_dist(), motorway));
        // primary_link (12) falls under urban split (closer to urban flow).
        assert!(!std::ptr::eq(make(12).time_dist(), motorway));
    }
}
