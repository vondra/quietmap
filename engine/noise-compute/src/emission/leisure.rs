//! Leisure-area (sports / play / open-air hospitality) noise emission. These
//! OSM features carry NO `building=*` (a padel court / playground / café terrace
//! is not a building), so they spill to their own `leisure.arrow` and
//! [`crate::normalize::prepare_leisure_points`] discretises them through the SAME
//! area grid + area-law as buildings — UNIFIED: `lw = settlement::area_lw(..)`
//! over the polygon area (a bigger court / terrace is louder). The ONLY thing
//! that differs from a building is PHYSICAL: the source sits on the open ground
//! (~1.5 m, voices/rackets, not roof plant) with no floors. Per-sport spectrum +
//! day/evening/night pattern below.
//!
//! Per-sport calibration sources are cited inline. All `lw` are radiated dB(A)
//! (`a_weighted_total(bands) == lw`) and bake the duty cycle / seasonality into a single annualized level
//! because the engine has no month axis.
//! PROP-MEAS = no clean measured value found; a conservative placeholder ships
//! flagged and is never presented as measured.

use crate::types::NUM_BANDS;

/// Leisure `sport`/kind class ids written by `osm-extract::spill` into
/// `leisure.arrow`. Stable once shipped (own v1 per-file contract): the arrow
/// stores the raw u8. 0 is the generic-pitch default so an untyped
/// `leisure=pitch` still emits.
pub const PITCH: u8 = 0;
pub const PADEL: u8 = 1;
pub const TENNIS: u8 = 2;
pub const BASKETBALL: u8 = 3;
pub const PLAYGROUND: u8 = 4;
pub const POOL: u8 = 5;
/// Open-air hospitality seating (`amenity=biergarten`, `outdoor_seating=yes`,
/// `leisure=outdoor_seating`) — patron voices, area-scaled like everything else.
pub const OUTDOOR_SEATING: u8 = 6;
/// Stadium / large sports ground — pitch + crowd + PA; rare match-day events,
/// kept at pitch level because rare match days must not be over-weighted.
pub const STADIUM: u8 = 7;

/// Map an OSM `sport=*` value (lower-cased) to a leisure class id. `leisure=*`
/// kind is the fallback when `sport` is absent (resolved in `spill.rs`).
pub fn sport_class(sport: &str) -> Option<u8> {
    Some(match sport {
        "padel" => PADEL,
        "tennis" => TENNIS,
        "basketball" | "netball" | "handball" => BASKETBALL,
        // Ball-sport pitches all share the football/pitch anchor.
        "soccer" | "football" | "american_football" | "rugby" | "rugby_union" | "rugby_league"
        | "field_hockey" | "hockey" | "baseball" | "cricket" | "multi" => PITCH,
        "swimming" => POOL,
        _ => return None,
    })
}

/// Per-leisure-area emission profile — the SAME area-law as a building
/// (`settlement::area_lw`): a low fixed floor plus a per-m² term over the
/// polygon area. Source height is fixed (~1.5 m) in the prep path; no floors.
pub struct LeisureProfile {
    /// Minimum plant floor (dB) — the shared `settlement::area_lw` form. Leisure
    /// has no real fixed plant, so this is a low common floor; the per-m² term
    /// carries the level.
    pub lw_fixed: f64,
    /// Per-m² of leisure AREA (dB/m²) — the SAME area-law as buildings
    /// (`settlement::area_lw`). Replaces the old capacity scaling: the OSM polygon
    /// area IS the source size (a 2-court padel / a 300-seat garden is bigger ⇒
    /// louder), so the whole emission model is now unified on AREA, not capacity.
    pub lw_per_m2: f64,
    /// Typical facility footprint (m²) the anchor below assumes — also the
    /// fallback area for a leisure NODE with no polygon (a court ~200, a café
    /// terrace ~50). `area_lw(lw_fixed, lw_per_m2, ref_area_m2)` == the cited anchor.
    pub ref_area_m2: f64,
    /// Relative dB per 8-band [63..8k]; A-sum-normalized to the radiated lw.
    pub spectrum: [f64; NUM_BANDS],
    pub evening_offset: f64,
    pub night_offset: f64,
}

/// Emission profile by leisure class id (see the `pub const`s).
///
/// ANNUALIZATION (the honest weak spot — END/CNOSSOS does NOT model sport; no
/// standard prescribes a yearly Lden for a court). Each `lw_per_m2` below is an
/// ACTIVE peak-use sound power (measured, cited per arm) MINUS a transparent
/// duty cut to a year-average Lden:
///   annual Lden = active Lw  −  seasonal  −  daily-duty
///     seasonal  : outdoor play ~6 months/yr → −3 dB (pool ~4 mo → −5; play ~−2)
///     daily-duty: a court is in active use ~6 of 24 h (day+evening) → −6 dB
///                 (stadium: match days only ~25/yr → −12)
/// Net ≈ −9 dB for outdoor sport. Assumptions are LISTED on /about (nothing
/// invented). Indoor halls are indistinguishable in OSM → treated as outdoor
/// (a stated limitation). PROP-MEAS = no clean measured Lw; a flagged estimate.
///
/// Active anchors (pre-annualization): padel 90 (racket "pock" on glass,
/// padelcreations + Higgins); tennis 84 (LFmax 58.4/strike, TU München);
/// football pitch 88 (58 LAeq,1h @10 m, Sport England AGP); basketball pitch−6
/// (UBC/BKL); playground PROP-MEAS; pool PROP-MEAS; outdoor seating 71 dB(A)/
/// guest (Lärmfibel Biergärten).
pub fn leisure_profile(sport: u8) -> LeisureProfile {
    // All leisure shares a low fixed floor; `lw_per_m2` over the polygon area
    // carries the level (shared `settlement::area_lw`). Each anchor in the
    // comment = area_lw(FLOOR, lw_per_m2, ref_area_m2) = the year-average Lden.
    const FLOOR: f64 = 40.0;
    match sport {
        PADEL => LeisureProfile {
            // active 90 (racket "pock" on glass; padelcreations + Higgins) − 9
            // annual (−3 season −6 duty) → year Lden 81 @ ~200 m². The 2024–26
            // complaint class; stays the loudest sport. HF-weighted, impulsive.
            lw_fixed: FLOOR,
            lw_per_m2: 58.0,
            ref_area_m2: 200.0,
            spectrum: [-6.0, -4.0, -2.0, -1.0, 0.0, 1.0, 2.0, 1.0],
            evening_offset: 0.0, // plays 07–23; evening is peak
            night_offset: -15.0,
        },
        TENNIS => LeisureProfile {
            // active 84 (LFmax 58.4/strike, TU München) − 9 annual (−3 season
            // −6 duty) → year Lden 74 @ ~260 m². Indoor halls look the same in
            // OSM → treated as outdoor (a stated /about limitation).
            lw_fixed: FLOOR,
            lw_per_m2: 50.0,
            ref_area_m2: 260.0,
            spectrum: [-5.0, -4.0, -2.0, -1.0, 0.0, 1.0, 1.0, 0.0],
            evening_offset: -3.0,
            night_offset: -20.0,
        },
        BASKETBALL => LeisureProfile {
            // active tennis−6 (ball bounce on hard court) − 9 annual → year Lden
            // 68 @ ~420 m².
            lw_fixed: FLOOR,
            lw_per_m2: 42.0,
            ref_area_m2: 420.0,
            spectrum: [-4.0, -3.0, -1.0, 0.0, 0.0, 0.0, 0.0, -1.0],
            evening_offset: -3.0,
            night_offset: -20.0,
        },
        PLAYGROUND => LeisureProfile {
            // child play (PROP-MEAS — no clean per-child Lw) − 8 annual (−2
            // weather −6 duty; used more of the year than a court) → year Lden
            // 71 @ ~200 m². Day-heavy.
            lw_fixed: FLOOR,
            lw_per_m2: 48.0,
            ref_area_m2: 200.0,
            spectrum: [-3.0, -1.0, 1.0, 2.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -25.0,
        },
        POOL => LeisureProfile {
            // outdoor lido splash/voice (PROP-MEAS) − 9 annual (−5 summer-only
            // May–Sep −4 duty) → year Lden 76 @ ~400 m².
            lw_fixed: FLOOR,
            lw_per_m2: 50.0,
            ref_area_m2: 400.0,
            spectrum: [-3.0, -2.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -25.0,
        },
        OUTDOOR_SEATING => LeisureProfile {
            // beer garden / café terrace patron voices (Lärmfibel/VDI 3770 71
            // dB(A)/guest raised speech). Annualized HARD (~−16): warm season only +
            // meal-time hours (not all day) + conversational (not continuous). And
            // these are mostly bare `outdoor_seating=yes` NODES with no size (~84 %
            // of them), so a node assumes a SMALL ~12 m² terrace (a few tables) → ~66
            // dB — not a 50-seat beer garden. A mapped 50 m² terrace → ~72, 200 m² →
            // ~78. (Was 65 / 50 m² → a flat 82 that lit the whole map as loud dots.)
            lw_fixed: FLOOR,
            lw_per_m2: 55.0,
            ref_area_m2: 12.0,
            spectrum: [-2.0, -1.0, 1.0, 2.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: 0.0,
            night_offset: -15.0,
        },
        STADIUM => LeisureProfile {
            // pitch + crowd/PA, match days ONLY (~25/yr) − 12 duty → year Lden 78
            // @ ~7000 m². Do NOT over-weight (rare events, big footprint).
            lw_fixed: FLOOR,
            lw_per_m2: 40.0,
            ref_area_m2: 7000.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0],
            evening_offset: -3.0,
            night_offset: -12.0,
        },
        // PITCH (0) + any unknown → generic ball-sport pitch. active 88 (58
        // LAeq,1h @10 m, Sport England AGP) − 9 annual → year Lden 78 @ ~7000 m²
        // (a typical ~1100 m² pitch lands ~70).
        _ => LeisureProfile {
            lw_fixed: FLOOR,
            lw_per_m2: 40.0,
            ref_area_m2: 7000.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0],
            evening_offset: -3.0,
            night_offset: -10.0, // floodlit pitches run to ~22:00
        },
    }
}

/// Emission bands for a leisure area (day period), normalized so
/// `a_weighted_total(bands) == lw` (same contract as buildings).
pub fn leisure_emission_bands(profile: &LeisureProfile, lw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(lw, &profile.spectrum)
}

/// Leisure Lw — the shared [`crate::emission::settlement::area_lw`] over the
/// leisure polygon area (a bigger court / terrace is louder). Unified with
/// buildings; the polygon area replaces the old per-facility capacity scaling.
pub fn leisure_lw(profile: &LeisureProfile, area_m2: f64) -> f64 {
    crate::emission::settlement::area_lw(profile.lw_fixed, profile.lw_per_m2, area_m2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn radiated_dba_equals_lw_for_all_leisure_classes() {
        for s in [
            PITCH,
            PADEL,
            TENNIS,
            BASKETBALL,
            PLAYGROUND,
            POOL,
            OUTDOOR_SEATING,
            STADIUM,
        ] {
            let p = leisure_profile(s);
            let lw = leisure_lw(&p, 500.0);
            let aw = a_weighted_total(&leisure_emission_bands(&p, lw));
            assert!(
                (aw - lw).abs() < 1e-6,
                "sport {s}: radiated {aw:.6} != lw {lw:.6}"
            );
        }
    }

    /// The plan's loudness ordering must hold (each at its reference court):
    /// padel ≫ tennis ≫ basketball — padel is the 2024–26 complaint class.
    #[test]
    fn padel_louder_than_tennis_louder_than_basketball() {
        let anchor = |s: u8| {
            let p = leisure_profile(s);
            leisure_lw(&p, p.ref_area_m2)
        };
        assert!(anchor(PADEL) > anchor(TENNIS), "padel must beat tennis");
        assert!(
            anchor(TENNIS) > anchor(BASKETBALL),
            "tennis must beat basketball"
        );
    }

    /// Unified area scaling (replaces the old capacity build-up): a leisure
    /// source scales with its polygon AREA, +3 dB per doubling, and the anchor
    /// holds at the profile's reference footprint.
    #[test]
    fn leisure_scales_with_area() {
        let p = leisure_profile(OUTDOOR_SEATING); // node default ~12 m² ≈ 66 dB
        let small = leisure_lw(&p, p.ref_area_m2);
        let big = leisure_lw(&p, p.ref_area_m2 * 2.0);
        assert!(
            (small - 65.8).abs() < 0.2,
            "node-default terrace: {small:.1}"
        );
        assert!((big - small - 3.0103).abs() < 0.05, "doubling area = +3 dB");
    }

    #[test]
    fn sport_class_maps_known_values() {
        assert_eq!(sport_class("padel"), Some(PADEL));
        assert_eq!(sport_class("tennis"), Some(TENNIS));
        assert_eq!(sport_class("soccer"), Some(PITCH));
        assert_eq!(sport_class("basketball"), Some(BASKETBALL));
        assert_eq!(sport_class("swimming"), Some(POOL));
        assert_eq!(sport_class("chess"), None);
    }
}
