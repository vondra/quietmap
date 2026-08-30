//! Per-segment validity filters and ground-context helpers.
//!
//! These predicates classify each ADS-B segment into one of three buckets:
//! * **Ground** — taxi/runway/apron, near terrain or under airport context.
//! * **Valid airborne** — physically plausible flight (above terrain, sane speed).
//! * **Stale ground** — extractor missed an `on_ground` flag; runtime fallback
//!   rejects via low-AGL test.
//!
//! `SegmentTerrain` caches DEM samples for the cruise validity path. Airborne
//! popup and heatmap retain only endpoint samples for the stale-ground gate.

use crate::types::{AircraftSegment, RasterSampler};

use super::npd::{is_helicopter_profile, noise_class_of, IS_JET};

/// Runtime fallback for stale prepared ADS-B data that still contains taxi or
/// runway-roll segments. The authoritative fix is extractor-side `on_ground`
/// filtering, but until the full dataset is rebuilt we also reject segments
/// whose both endpoints stay close to local terrain.
pub const GROUND_STALE_MAX_AGL_M: f64 = 15.0;
pub const AIRPORT_GROUND_MAX_AGL_M: f64 = 60.0;
pub const GROUND_CONTEXT_NONE: u8 = 0;
pub const GROUND_CONTEXT_AIRPORT_LINE: u8 = 1;
pub const GROUND_OPS_KIND_NONE: u8 = 0;
pub const GROUND_OPS_KIND_RUNWAY_ROLL: u8 = 1;
pub const GROUND_OPS_KIND_TAXI: u8 = 2;
pub const GROUND_OPS_KIND_APRON_MOVEMENT: u8 = 3;
pub const GROUND_OPS_SOURCE_HEIGHT_M: f64 = 4.0;

pub fn is_ground_ops_segment(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> bool {
    seg.surface_model || is_airport_ground_segment(seg, rasters)
}

pub fn resolve_ground_ops_kind(seg: &AircraftSegment) -> u8 {
    if seg.ground_ops_kind != GROUND_OPS_KIND_NONE {
        seg.ground_ops_kind
    } else if seg.ground_context != GROUND_CONTEXT_NONE || seg.on_ground || seg.surface_model {
        ground_ops_kind_fallback(seg)
    } else {
        GROUND_OPS_KIND_NONE
    }
}

/// Speed-only `ops_kind` classifier — fallback for segments without
/// an explicit Stage-2C ops_kind (no OSM aeroway match). The
/// authoritative `ops_kind` comes from `airport_traffic.arrow` rows
/// keyed off OSM aeroway type (see SPEC §5.2).
///
/// * `speed_kt ≥ 40` → `RUNWAY_ROLL`
/// * `speed_kt ≥ 8`  → `TAXI`
/// * else            → `APRON_MOVEMENT`
///
/// Helicopters never get `RUNWAY_ROLL` — a helicopter accelerating
/// through 40 kt on a helipad is rotor-thrust-driven, not the turbofan
/// T/O profile RUNWAY_ROLL is calibrated for. Cap helicopters at TAXI.
pub fn ground_ops_kind_fallback(seg: &AircraftSegment) -> u8 {
    if is_helicopter_profile(seg.profile_idx) {
        return if seg.speed_kt >= 8.0 {
            GROUND_OPS_KIND_TAXI
        } else {
            GROUND_OPS_KIND_APRON_MOVEMENT
        };
    }
    if seg.speed_kt >= 40.0 {
        GROUND_OPS_KIND_RUNWAY_ROLL
    } else if seg.speed_kt >= 8.0 {
        GROUND_OPS_KIND_TAXI
    } else {
        GROUND_OPS_KIND_APRON_MOVEMENT
    }
}

/// Per-segment terrain sample cache (start, q1, mid, q3, end).
///
/// Cruise scatter samples all five points. The airborne paths construct this
/// with only start/end populated and call only [`is_ground_stale_with_terrain`].
#[derive(Clone, Copy, Debug)]
pub struct SegmentTerrain {
    pub start_elev: f64,
    pub q1_elev: f64,
    pub mid_elev: f64,
    pub q3_elev: f64,
    pub end_elev: f64,
}

impl SegmentTerrain {
    pub fn sample(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> Self {
        let mid_lat = (seg.start_lat + seg.end_lat) * 0.5;
        let mid_lon = (seg.start_lon + seg.end_lon) * 0.5;
        let q1_lat = seg.start_lat * 0.75 + seg.end_lat * 0.25;
        let q1_lon = seg.start_lon * 0.75 + seg.end_lon * 0.25;
        let q3_lat = seg.start_lat * 0.25 + seg.end_lat * 0.75;
        let q3_lon = seg.start_lon * 0.25 + seg.end_lon * 0.75;
        SegmentTerrain {
            start_elev: rasters.elevation(seg.start_lat, seg.start_lon),
            q1_elev: rasters.elevation(q1_lat, q1_lon),
            mid_elev: rasters.elevation(mid_lat, mid_lon),
            q3_elev: rasters.elevation(q3_lat, q3_lon),
            end_elev: rasters.elevation(seg.end_lat, seg.end_lon),
        }
    }
}

/// Filter obviously-invalid airborne ADS-B segments.
/// Returns false for segments that pipeline AND popup should skip:
/// - Max altitude below terrain - 30m (underground / radar echo)
/// - Jet-like profile (not Turboprop, not LightGA/helicopter) flying < 80 kt (impossible)
/// - Jet-like profile < 150m AGL outside any airport context (radar echo / decode error)
///
/// Retained for raster-backed callers; current airborne paths do not call it.
pub fn is_valid_airborne_segment(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> bool {
    if seg.on_ground || seg.ground_context != GROUND_CONTEXT_NONE {
        return true;
    }

    let is_fixed_wing_jet = IS_JET[noise_class_of(seg.profile_idx) as usize];

    if is_fixed_wing_jet && (seg.speed_kt as f64) < 80.0 {
        return false;
    }

    let mid_lat = (seg.start_lat + seg.end_lat) * 0.5;
    let mid_lon = (seg.start_lon + seg.end_lon) * 0.5;
    let terrain_mid = rasters.elevation(mid_lat, mid_lon);
    let max_alt = (seg.start_alt_m as f64).max(seg.end_alt_m as f64);

    if max_alt < terrain_mid - 30.0 {
        return false;
    }

    // Endpoint AGL < -30 m handles subsea-level airports (Schiphol -4m, Atyrau
    // -22m, Caspian-basin sites) via DEM-relative terrain rather than global MSL.
    let (start_agl, end_agl) = segment_agl(seg, rasters);
    if start_agl < -30.0 || end_agl < -30.0 {
        return false;
    }

    let sl = seg.start_lat;
    let sn = seg.start_lon;
    let el = seg.end_lat;
    let en = seg.end_lon;
    let sa = seg.start_alt_m as f64;
    let ea = seg.end_alt_m as f64;
    for frac in [0.25_f64, 0.75] {
        let lat = sl + (el - sl) * frac;
        let lon = sn + (en - sn) * frac;
        let alt = sa + (ea - sa) * frac;
        if alt < rasters.elevation(lat, lon) - 30.0 {
            return false;
        }
    }

    if !is_fixed_wing_jet {
        return true;
    }

    // Jet < 150m AGL outside airport: radar echo or altitude decode error.
    if max_alt < terrain_mid + 150.0 {
        return false;
    }

    true
}

pub fn is_ground_stale_segment(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> bool {
    if seg.on_ground {
        return seg.ground_context == GROUND_CONTEXT_NONE;
    }
    if seg.ground_context != GROUND_CONTEXT_NONE {
        return false;
    }
    is_low_agl_segment_raw(seg, rasters)
}

pub fn is_low_agl_segment_raw(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> bool {
    let (start_agl, end_agl) = segment_agl(seg, rasters);
    start_agl <= GROUND_STALE_MAX_AGL_M && end_agl <= GROUND_STALE_MAX_AGL_M
}

/// `is_ground_stale_segment` reading elevations from a `SegmentTerrain` cache.
pub fn is_ground_stale_with_terrain(seg: &AircraftSegment, terrain: &SegmentTerrain) -> bool {
    if seg.on_ground {
        return seg.ground_context == GROUND_CONTEXT_NONE;
    }
    if seg.ground_context != GROUND_CONTEXT_NONE {
        return false;
    }
    let start_agl = seg.start_alt_m as f64 - terrain.start_elev;
    let end_agl = seg.end_alt_m as f64 - terrain.end_elev;
    start_agl <= GROUND_STALE_MAX_AGL_M && end_agl <= GROUND_STALE_MAX_AGL_M
}

/// `is_valid_airborne_segment` reading all five elevations from a
/// [`SegmentTerrain`] cache. Cruise constructs one in place via
/// [`SegmentTerrain::sample`]; current airborne paths do not call this gate.
///
/// Underground-segment check: AGL ≥ -30 m must hold at all five
/// stored sample points (start / q1 / mid / q3 / end). Aircraft `alt`
/// is linearly interpolated between endpoints (we have only the two
/// ADS-B samples bounding the sub-segment); terrain `elev` is real
/// DEM at each frac, so it is NOT linear in the sub-segment. The q1/q3
/// samples catch a narrow ridge spike at frac=0.25 or 0.75 that a
/// midpoint-only check would miss.
pub fn is_valid_airborne_with_terrain(seg: &AircraftSegment, terrain: &SegmentTerrain) -> bool {
    if seg.on_ground || seg.ground_context != GROUND_CONTEXT_NONE {
        return true;
    }
    let is_fixed_wing_jet = IS_JET[noise_class_of(seg.profile_idx) as usize];
    if is_fixed_wing_jet && (seg.speed_kt as f64) < 80.0 {
        return false;
    }
    let max_alt = (seg.start_alt_m as f64).max(seg.end_alt_m as f64);
    if max_alt < terrain.mid_elev - 30.0 {
        return false;
    }
    let sa = seg.start_alt_m as f64;
    let ea = seg.end_alt_m as f64;
    let start_agl = sa - terrain.start_elev;
    let end_agl = ea - terrain.end_elev;
    if start_agl < -30.0 || end_agl < -30.0 {
        return false;
    }
    // Mid / q1 / q3 AGL gates. Aircraft alt is linearly interpolated
    // between endpoints (no intermediate ADS-B samples); terrain elev
    // is real raster at each frac. A steep climb (start_alt=1000,
    // end_alt=2000) over a midpath peak (mid_elev=1800) has
    // mid_alt=1500, i.e. 300 m underground at the midpoint. q1/q3
    // catch the same pattern when the peak isn't centred.
    let mid_alt = (sa + ea) * 0.5;
    if mid_alt < terrain.mid_elev - 30.0 {
        return false;
    }
    let q1_alt = sa * 0.75 + ea * 0.25;
    let q3_alt = sa * 0.25 + ea * 0.75;
    if q1_alt < terrain.q1_elev - 30.0 || q3_alt < terrain.q3_elev - 30.0 {
        return false;
    }
    if !is_fixed_wing_jet {
        return true;
    }
    max_alt >= terrain.mid_elev + 150.0
}

pub fn is_airport_ground_segment(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> bool {
    if seg.ground_context == GROUND_CONTEXT_NONE {
        return false;
    }
    if seg.on_ground {
        return true;
    }
    let (start_agl, end_agl) = segment_agl(seg, rasters);
    start_agl <= AIRPORT_GROUND_MAX_AGL_M && end_agl <= AIRPORT_GROUND_MAX_AGL_M
}

fn segment_agl(seg: &AircraftSegment, rasters: &dyn RasterSampler) -> (f64, f64) {
    let start_agl = seg.start_alt_m as f64 - rasters.elevation(seg.start_lat, seg.start_lon);
    let end_agl = seg.end_alt_m as f64 - rasters.elevation(seg.end_lat, seg.end_lon);
    (start_agl, end_agl)
}

/// Meters → degrees of latitude (constant ≈ 110 540 m / deg, valid to
/// within ~0.6 % anywhere on Earth). Use for any latitude bounding box
/// where the exact geodesic isn't worth the cost.
pub fn meters_to_lat_deg(meters: f64) -> f64 {
    meters / 110_540.0
}

/// Meters → degrees of longitude at a given latitude. Includes a
/// `cos.max(0.2)` clamp that bounds the conversion factor at ~78°
/// latitude — above that the cosine collapses and the bbox would
/// over-fetch enormously. Aircraft cruise tracks live well below that
/// limit, so the clamp is a safety net rather than a routine concern.
pub fn meters_to_lon_deg(lat: f64, meters: f64) -> f64 {
    let cos_lat = lat.to_radians().cos().abs().max(0.2);
    meters / (111_320.0 * cos_lat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::AIRCRAFT_ADSB_SOURCE_ID;

    struct FlatGround;

    impl RasterSampler for FlatGround {
        fn elevation(&self, _lat: f64, _lon: f64) -> f64 {
            250.0
        }
        fn ground_g(&self, _lat: f64, _lon: f64) -> f64 {
            0.0
        }
        fn building_enclosure(&self, _lat: f64, _lon: f64) -> f64 {
            0.0
        }
    }

    /// A steep climb over a midpath mountain peak whose stored
    /// `terrain.mid_elev` sits
    /// above the linearly-interpolated `(start_alt + end_alt) / 2`
    /// must be rejected by `is_valid_airborne_with_terrain`. The
    /// `max_alt < mid_elev - 30` gate alone misses this case (end_alt
    /// pulls max_alt above mid_elev); the `mid_alt < mid_elev - 30`
    /// gate added in Opt A v15 closes the hole.
    #[test]
    fn airborne_with_terrain_rejects_steep_climb_below_midpath_peak() {
        let seg = AircraftSegment {
            flight_id: 1,
            profile_idx: 0, // jet
            is_departure: true,
            on_ground: false,
            period: 0,
            date_id: 0,
            start_lat: 47.25, // LOWI-ish geometry
            start_lon: 11.34,
            start_alt_m: 1000.0, // lowland start
            end_lat: 47.27,
            end_lon: 11.40,
            end_alt_m: 2000.0, // higher end (still below peak)
            speed_kt: 200.0,   // jet ≥ 80 kt
            segment_length_m: 5000.0,
            ground_context: GROUND_CONTEXT_NONE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            count_weight: 1.0,
            surface_model: false,
            source_id: AIRCRAFT_ADSB_SOURCE_ID,
        };
        let terrain = SegmentTerrain {
            start_elev: 700.0, // start AGL = 300 m, OK
            q1_elev: 1100.0,   // q1_alt = 1250 → AGL = 150, OK
            mid_elev: 1800.0,  // peak  → mid_alt = 1500 m → AGL = -300 m
            q3_elev: 1750.0,   // q3_alt = 1750 → AGL = 0, OK
            end_elev: 1700.0,  // end   AGL = 300 m, OK
        };
        // max_alt = 2000 ≥ mid_elev - 30 = 1770: passes the max gate.
        // mid_alt = 1500 < mid_elev - 30 = 1770: fails the mid gate.
        assert!(
            !is_valid_airborne_with_terrain(&seg, &terrain),
            "steep climb over midpath peak must be rejected (mid_alt < mid_elev - 30)"
        );
    }

    /// Q1/Q3-only spike: midpoint elevation is benign but a narrow
    /// ridge at frac=0.25 (or 0.75) crosses the climbing alt path.
    /// Without q1/q3 raster samples this case would silently pass.
    #[test]
    fn airborne_with_terrain_rejects_q1_spike() {
        let seg = AircraftSegment {
            flight_id: 1,
            profile_idx: 0,
            is_departure: true,
            on_ground: false,
            period: 0,
            date_id: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 1000.0,
            end_lat: 50.01,
            end_lon: 14.01,
            end_alt_m: 2000.0,
            speed_kt: 200.0,
            segment_length_m: 1400.0,
            ground_context: GROUND_CONTEXT_NONE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            count_weight: 1.0,
            surface_model: false,
            source_id: AIRCRAFT_ADSB_SOURCE_ID,
        };
        // q1_alt = 1000*0.75 + 2000*0.25 = 1250 m; q1_elev = 1500 →
        // q1_alt - q1_elev = -250 m. Other points safe.
        let terrain = SegmentTerrain {
            start_elev: 700.0,
            q1_elev: 1500.0,  // narrow ridge at frac=0.25
            mid_elev: 1100.0, // dips back
            q3_elev: 1500.0,
            end_elev: 1700.0,
        };
        assert!(
            !is_valid_airborne_with_terrain(&seg, &terrain),
            "narrow ridge at q1 must reject the sub-segment"
        );
    }

    #[test]
    fn test_ground_stale_segment_filter() {
        let seg = AircraftSegment {
            flight_id: 1,
            profile_idx: 7,
            is_departure: false,
            on_ground: false,
            period: 0,
            date_id: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 252.0,
            end_lat: 50.001,
            end_lon: 14.001,
            end_alt_m: 259.0,
            speed_kt: 35.0,
            segment_length_m: 300.0,
            ground_context: GROUND_CONTEXT_NONE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            count_weight: 1.0,
            surface_model: false,
            source_id: AIRCRAFT_ADSB_SOURCE_ID,
        };
        assert!(is_ground_stale_segment(&seg, &FlatGround));

        let airborne = AircraftSegment {
            start_alt_m: 320.0,
            end_alt_m: 340.0,
            ..seg
        };
        assert!(!is_ground_stale_segment(&airborne, &FlatGround));
    }

    #[test]
    fn test_airport_ground_segment_detection() {
        let off_airport = AircraftSegment {
            flight_id: 1,
            profile_idx: 7,
            is_departure: false,
            on_ground: false,
            period: 0,
            date_id: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 252.0,
            end_lat: 50.0008,
            end_lon: 14.0,
            end_alt_m: 255.0,
            speed_kt: 35.0,
            segment_length_m: 90.0,
            ground_context: GROUND_CONTEXT_NONE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            count_weight: 1.0,
            surface_model: false,
            source_id: AIRCRAFT_ADSB_SOURCE_ID,
        };
        assert!(!is_airport_ground_segment(&off_airport, &FlatGround));

        let airport_ground = AircraftSegment {
            ground_context: GROUND_CONTEXT_AIRPORT_LINE,
            ..off_airport
        };
        assert!(is_airport_ground_segment(&airport_ground, &FlatGround));
    }

    /// A helicopter accelerating through 40 kt on a helipad must be
    /// classified as TAXI, not RUNWAY_ROLL. The latter SEL is calibrated
    /// for turbofan T/O thrust which is absent on rotor aircraft.
    #[test]
    fn helicopter_never_runway_roll() {
        // EC35 (Eurocopter EC135) — first helicopter typecode in the
        // profile array. Class HELICOPTER, anchor for all 21 heli types.
        let heli_profile_idx = crate::emission::aircraft::profile_idx("EC35");

        let fast_seg = AircraftSegment {
            flight_id: 1,
            profile_idx: heli_profile_idx,
            is_departure: true,
            on_ground: true,
            period: 0,
            date_id: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: 252.0,
            end_lat: 50.001,
            end_lon: 14.001,
            end_alt_m: 252.0,
            speed_kt: 60.0,          // would trigger RUNWAY_ROLL for fixed-wing
            segment_length_m: 800.0, // ditto
            ground_context: GROUND_CONTEXT_AIRPORT_LINE,
            ground_ops_kind: GROUND_OPS_KIND_NONE,
            count_weight: 1.0,
            surface_model: false,
            source_id: AIRCRAFT_ADSB_SOURCE_ID,
        };
        assert_eq!(ground_ops_kind_fallback(&fast_seg), GROUND_OPS_KIND_TAXI);

        let slow_seg = AircraftSegment {
            speed_kt: 3.0,
            segment_length_m: 50.0,
            ..fast_seg.clone()
        };
        assert_eq!(
            ground_ops_kind_fallback(&slow_seg),
            GROUND_OPS_KIND_APRON_MOVEMENT
        );

        // Sanity: a non-helicopter (B738) at the same fast settings still
        // hits RUNWAY_ROLL. Confirms the gate is helicopter-specific.
        let jet_profile_idx = crate::emission::aircraft::profile_idx("B738");
        let jet_seg = AircraftSegment {
            profile_idx: jet_profile_idx,
            ..fast_seg
        };
        assert_eq!(
            ground_ops_kind_fallback(&jet_seg),
            GROUND_OPS_KIND_RUNWAY_ROLL
        );
    }
}
