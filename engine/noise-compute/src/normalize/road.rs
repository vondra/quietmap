//! Road source normalization: prepared road traffic → per-band emission-ready
//! values (`NormalizedRoad`), plus the legal speed cascade and the surface
//! correction. Traffic is FINAL at read time — `roads-finalize` resolved
//! observations, priors and allocation before publication — so this module
//! consumes the prepared counts verbatim and never substitutes, scales or
//! re-gates them (SPEC §"Prepared road direction and traffic"). The
//! class-default cascade and the access/lane/oneway estimate factors remain
//! available here (`resolve_traffic_default`, `access_factor`, `lane_ratio`)
//! for that producer to call at build time.

use crate::constants::{SOURCE_HEIGHT_ROAD, SURFACE_CORR};
use crate::defaults::{resolve_speed_default, WORLD_DEFAULT};
use crate::emission::road;
use crate::sources::Provenance;
use crate::square_country_city::SquareCountryCity;
use crate::types::{RoadSegment, NUM_BANDS};

use super::{bands_to_f32, DERESTRICTED_SPEED_KMH, SPEED_LIMIT_DERESTRICTED};

/// Bit in [`RoadTraffic::estimated`] marking the light category as estimated.
pub const ROAD_ESTIMATED_LIGHT: u8 = 1;
/// Bit in [`RoadTraffic::estimated`] marking the medium category as estimated.
pub const ROAD_ESTIMATED_MEDIUM: u8 = 2;
/// Bit in [`RoadTraffic::estimated`] marking the heavy category as estimated.
pub const ROAD_ESTIMATED_HEAVY: u8 = 4;
/// Bit in [`RoadTraffic::estimated`] marking the motorcycle category as estimated.
pub const ROAD_ESTIMATED_MOTO: u8 = 8;

/// Observed per-vehicle-class share of the 24 h volume across the three legal
/// periods (day 07–19, evening 19–23, night 23–07 local) — the measured
/// counterpart of the class-default [`road::TimeDist`] splits. A class with no
/// observations stays `None` and keeps the class default; classes with counts
/// carry `[day, evening, night]` fractions summing to 1. One canonical
/// validation lives here; every producer, reader and consumer defers to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadTimeProfile {
    pub light: Option<[f64; 3]>,
    pub medium: Option<[f64; 3]>,
    pub heavy: Option<[f64; 3]>,
    pub moto: Option<[f64; 3]>,
}

impl RoadTimeProfile {
    /// Shares must be finite, non-negative, and each observed class triple
    /// sums to 1 (1e-9 tolerance for float aggregation rounding).
    pub fn validate(&self) -> Result<(), String> {
        for (name, class) in
            [("light", &self.light), ("medium", &self.medium), ("heavy", &self.heavy), ("moto", &self.moto)]
        {
            let Some(shares) = class else { continue };
            if shares.iter().any(|v| !v.is_finite() || *v < 0.0)
                || (shares[0] + shares[1] + shares[2] - 1.0).abs() > 1e-9
            {
                return Err(format!("invalid {name} period shares {:?}", shares));
            }
        }
        Ok(())
    }

    pub fn class_shares(&self, class: usize) -> Option<[f64; 3]> {
        match class {
            0 => self.light,
            1 => self.medium,
            2 => self.heavy,
            3 => self.moto,
            _ => None,
        }
    }
}

/// Prepared road traffic: per-category effective vehicles/day plus the
/// per-category estimated bitmask written by `roads-finalize`. Bit set means
/// that category's value is an estimate or prior rather than an observed
/// count; the value is consumed either way.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoadTraffic {
    pub light: f64,
    pub medium: f64,
    pub heavy: f64,
    pub moto: f64,
    pub estimated: u8,
    /// Observed per-class period profile; `None` = no observed profile, the
    /// class-default split applies (absence is genuinely unknown, never a
    /// variant of the defaults).
    pub time_profile: Option<RoadTimeProfile>,
}

impl RoadTraffic {
    pub fn total(&self) -> f64 {
        self.light + self.medium + self.heavy + self.moto
    }

    /// A prepared total of exactly zero is a TRUE zero: the segment is
    /// silent, and no runtime default may resurrect it.
    pub fn is_silent(&self) -> bool {
        self.total() == 0.0
    }
}

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
    /// Prepared effective traffic; consumed verbatim.
    pub traffic: RoadTraffic,
    pub tunnel: bool,
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
    /// Observed period profile carried from the prepared row; `None` keeps
    /// the class-default [`Self::time_dist`] split.
    pub traffic_profile: Option<RoadTimeProfile>,
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

    /// Per-class traffic shares for each period (outer index day/evening/night,
    /// inner light/medium/heavy/moto): the observed profile where a class has
    /// one, the class-default split otherwise. `build_period_flows` consumes
    /// exactly these — there is no second day/night split anywhere.
    pub fn period_pcts(&self) -> [[f64; 4]; 3] {
        let default = self.time_dist();
        let defaults = [default.day_pct, default.evening_pct, default.night_pct];
        let profile = self.traffic_profile;
        let class = |c: usize| {
            let shares = profile.and_then(|p| p.class_shares(c));
            [0, 1, 2].map(|p| shares.map_or(defaults[p], |s| s[p]))
        };
        let [light, medium, heavy, moto] = [class(0), class(1), class(2), class(3)];
        let columns = [light, medium, heavy, moto];
        std::array::from_fn(|p| std::array::from_fn(|c| columns[c][p]))
    }

    pub fn period_emission(&self, period_pcts: [f64; 4], period_hours: f64) -> [f32; NUM_BANDS] {
        let flows = road::build_period_flows(
            self.light_aadt,
            self.medium_aadt,
            self.heavy_aadt,
            self.moto_aadt,
            self.speed_kmh,
            period_pcts,
            period_hours,
        );
        bands_to_f32(road::line_source_emission(&flows, self.surf_corr_db))
    }

    pub fn period_emissions(&self) -> ([f32; NUM_BANDS], [f32; NUM_BANDS], [f32; NUM_BANDS]) {
        let [day, evening, night] = self.period_pcts();
        (
            self.period_emission(day, 12.0),
            self.period_emission(evening, 4.0),
            self.period_emission(night, 8.0),
        )
    }
}

/// Normalise a prepared road input into per-band emission-ready values.
///
/// `square_country_city` drives only the SPEED cascade (country legal
/// implicit limit for untagged rows). Traffic arrives final from
/// `roads-finalize`: a true total zero is silent (never a class default),
/// tunnels never emit, and no tag or provenance scales an observation here.
pub fn normalize_road(
    input: RawRoadInput,
    square_country_city: SquareCountryCity,
) -> Option<NormalizedRoad> {
    if input.tunnel || input.traffic.is_silent() {
        return None;
    }

    let class_idx = road_class_idx(input.road_class);
    let class_name = ROAD_CLASS_NAMES[class_idx];

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
        resolve_speed_default(input.road_class, square_country_city, input.built_up)
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
        max_distance_m: road_max_distance_m(input.road_class),
        source_height_m: SOURCE_HEIGHT_ROAD,
        speed_kmh,
        base_speed_kmh,
        surf_corr_db,
        light_aadt: input.traffic.light,
        medium_aadt: input.traffic.medium,
        heavy_aadt: input.traffic.heavy,
        moto_aadt: input.traffic.moto,
        traffic_profile: input.traffic.time_profile,
    })
}

pub fn normalize_road_segment(
    seg: &RoadSegment,
    square_country_city: SquareCountryCity,
) -> Option<NormalizedRoad> {
    normalize_road(
        RawRoadInput {
            road_class: seg.road_class,
            speed_limit: seg.speed_limit,
            speed_taper: seg.speed_taper,
            surface_type: seg.surface_type,
            traffic: seg.traffic,
            tunnel: seg.tunnel,
            junction: seg.junction,
            built_up: seg.built_up,
        },
        square_country_city,
    )
}

/// Lane-based AADT scaling ratio for un-enriched roads. PRODUCER input
/// (`roads-finalize` allocation) — runtime consumes prepared counts and never
/// calls this.
///
/// Source: CZ ŘSD Celostátní sčítání dopravy 2020. Median totals over
/// 3 197 deduplicated census sections (one section ≈ many OSM ways)
/// bucketed by `(class × lanes × oneway)`; each arm is
/// `bucket_median / 2-lane-baseline_median`. Producer: `pipeline/
/// calibrate-lane-ratios.ts` — re-run against current enriched arrows
/// to regenerate the table. Only buckets with N ≥ 30 sections emit an
/// arm, so trunk and tertiary stay at 1.0 (sample too thin). `min()`
/// clamps saturate higher lane counts at the highest calibrated
/// bucket.
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

/// AADT multiplier per OSM access code. PRODUCER input (`roads-finalize`
/// allocation) — runtime consumes prepared counts and never calls this.
/// Measured provenance (national / continental / global) passes through
/// unchanged — the observation already reflects the restriction.
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
pub fn access_factor(access: u8, provenance: Provenance, road_class: u8) -> f64 {
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
        _ => 1.0,  // 0=yes/untagged; codes 2/4 are zeroed by the producer
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

/// The raw class clamped onto the 13 class tables (names, reaches, world
/// defaults), which therefore must stay the same length.
const _: () = assert!(ROAD_CLASS_NAMES.len() == WORLD_DEFAULT.len());
fn road_class_idx(road_class: u8) -> usize {
    (road_class as usize).min(ROAD_CLASS_NAMES.len() - 1)
}

/// How far a road of this raw class is audible — the row's `max_distance_m`
/// after normalization, so a reader can reject a far row before the
/// normalize cascade and keep exactly the rows the cascade would (dev1 ba6bd59e).
pub fn road_max_distance_m(road_class: u8) -> f64 {
    ROAD_MAX_DIST[road_class_idx(road_class)]
}

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

    /// Audible prepared traffic for the speed-path tests (any positive counts).
    const AUDIBLE: RoadTraffic = RoadTraffic {
        light: 1000.0,
        medium: 100.0,
        heavy: 100.0,
        moto: 50.0,
        estimated: 0,
        time_profile: None,
    };

    fn prepared(traffic: RoadTraffic) -> RawRoadInput {
        RawRoadInput {
            road_class: 2,
            speed_limit: 50,
            speed_taper: 0,
            surface_type: 0,
            traffic,
            tunnel: false,
            junction: 0,
            built_up: 0,
        }
    }

    /// A published directional 10 000 stays 10 000 into emission — the
    /// producer already allocated scope, so runtime never applies a second
    /// oneway half-share (and no longer has the tag to consult at all).
    #[test]
    fn prepared_directional_count_is_not_rescaled() {
        let traffic = RoadTraffic {
            light: 10_000.0,
            ..Default::default()
        };
        let road = normalize_road(prepared(traffic), SquareCountryCity::UNKNOWN).unwrap();
        assert_eq!(road.light_aadt, 10_000.0);
        assert_eq!(
            normalize_road_segment(
                &crate::types::RoadSegment {
                    oneway: true,
                    traffic,
                    ..secondary_segment()
                },
                SquareCountryCity::UNKNOWN
            )
            .unwrap()
            .light_aadt,
            10_000.0
        );
    }

    /// Heavy-only prepared traffic emits without inventing light vehicles; a
    /// true total zero stays silent (never resurrected by class defaults);
    /// tunnels still never emit.
    #[test]
    fn heavy_only_emits_true_zero_stays_silent_and_tunnels_drop() {
        let heavy_only = RoadTraffic {
            heavy: 500.0,
            ..Default::default()
        };
        let road = normalize_road(prepared(heavy_only), SquareCountryCity::UNKNOWN)
            .expect("prepared heavy-only traffic emits");
        assert_eq!(
            (road.light_aadt, road.medium_aadt, road.heavy_aadt, road.moto_aadt),
            (0.0, 0.0, 500.0, 0.0)
        );
        assert!(
            normalize_road(prepared(RoadTraffic::default()), SquareCountryCity::UNKNOWN).is_none(),
            "a true total zero is silent"
        );
        assert!(
            normalize_road(
                RawRoadInput {
                    tunnel: true,
                    ..prepared(heavy_only)
                },
                SquareCountryCity::UNKNOWN
            )
            .is_none(),
            "tunnels stay dropped"
        );
    }

    /// Prepared priors stamped source_id=0 are valid traffic: runtime holds no
    /// provenance gate, so a prior consumes exactly like an observation. The
    /// estimated bitmask rides along untouched for the popup.
    #[test]
    fn prepared_priors_consume_like_observations() {
        let prior = RoadTraffic {
            light: 2640.0,
            medium: 120.0,
            heavy: 180.0,
            moto: 60.0,
            estimated: ROAD_ESTIMATED_LIGHT
                | ROAD_ESTIMATED_MEDIUM
                | ROAD_ESTIMATED_HEAVY
                | ROAD_ESTIMATED_MOTO,
            time_profile: None,
    };
        let road = normalize_road(prepared(prior), SquareCountryCity::UNKNOWN).unwrap();
        assert_eq!(road.light_aadt, 2640.0);
        assert_eq!(road.heavy_aadt, 180.0);
    }

    /// `maxspeed=none` sentinel (255) resolves to the derestricted model
    /// speed, not a literal 255 km/h emission input.
    #[test]
    fn derestricted_sentinel_resolves_to_130() {
        let road = normalize_road(
            RawRoadInput {
                road_class: 0,
                speed_limit: SPEED_LIMIT_DERESTRICTED,
                ..prepared(AUDIBLE)
            },
            SquareCountryCity::UNKNOWN,
        )
        .unwrap();
        assert_eq!(road.base_speed_kmh, DERESTRICTED_SPEED_KMH);
        assert_eq!(road.speed_kmh, DERESTRICTED_SPEED_KMH);
    }

    #[test]
    fn built_up_selects_cz_legal_speed_below_explicit_tags_and_taper() {
        let cz = SquareCountryCity {
            country_iso: *b"CZ",
            continent: crate::square_country_city::Continent::Europe,
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
                ..prepared(AUDIBLE)
            };
            assert_eq!(normalize_road(input, cz).unwrap().speed_kmh, expected);
        }
    }

    #[test]
    fn ramp_defaults_are_15_percent_of_mainline() {
        // Producer-side default table invariant (resolve_traffic_default
        // stays authoritative for roads-finalize).
        let (l0, m0, h0, x0) =
            crate::defaults::resolve_traffic_default(0, SquareCountryCity::UNKNOWN);
        let (l10, m10, h10, x10) =
            crate::defaults::resolve_traffic_default(10, SquareCountryCity::UNKNOWN);
        assert!((l10 - l0 * 0.15).abs() < 1e-6);
        assert!((m10 - m0 * 0.15).abs() < 1e-6);
        assert!((h10 - h0 * 0.15).abs() < 1e-6);
        assert!((x10 - x0 * 0.15).abs() < 1e-6);

        let (l1, m1, h1, _) = crate::defaults::resolve_traffic_default(1, SquareCountryCity::UNKNOWN);
        let (l11, m11, h11, _) =
            crate::defaults::resolve_traffic_default(11, SquareCountryCity::UNKNOWN);
        assert!((l11 - l1 * 0.15).abs() < 1e-6);
        assert!((m11 - m1 * 0.15).abs() < 1e-6);
        assert!((h11 - h1 * 0.15).abs() < 1e-6);

        let (l2, m2, h2, _) = crate::defaults::resolve_traffic_default(2, SquareCountryCity::UNKNOWN);
        let (l12, m12, h12, _) =
            crate::defaults::resolve_traffic_default(12, SquareCountryCity::UNKNOWN);
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
            traffic_profile: None,
        };
        let motorway = make(0).time_dist();
        assert!(std::ptr::eq(make(10).time_dist(), motorway));
        assert!(std::ptr::eq(make(11).time_dist(), motorway));
        // primary_link (12) falls under urban split (closer to urban flow).
        assert!(!std::ptr::eq(make(12).time_dist(), motorway));
    }

    /// Shared fixture for the segment-level flow: one prepared secondary.
    fn secondary_segment() -> crate::types::RoadSegment {
        crate::types::RoadSegment {
            osm_id: 1,
            square_country_city: None,
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
            traffic: RoadTraffic {
                light: 2640.0,
                medium: 120.0,
                heavy: 180.0,
                moto: 60.0,
                estimated: 15,
                time_profile: None,
            },
            source_id: 0,
            name: String::new(),
            road_ref: String::new(),
            bridge: false,
            tunnel: false,
            junction: 0,
            built_up: 0,
            dist_m: 200.0,
            cp_lat: 50.0,
            cp_lon: 14.0015,
            fraction: 0.5,
        }
    }
}
