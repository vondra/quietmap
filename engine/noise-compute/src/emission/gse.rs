//! GSE (ground support equipment) emission profiles.
//!
//! Three noise classes — LIGHT, MEDIUM, HEAVY — for airport ground
//! vehicles (follow-me cars, pushback tractors, fuel trucks, fire
//! trucks, runway sweepers, maintenance vans). These vehicles surface
//! in ADS-B feeds with `t=GND`; without a dedicated model the legacy
//! pipeline routed them through `WING_FALLBACK` 737-800 NPDs, which
//! over-estimates them by ~25-30 dB.
//!
//! ## Calibration source: CNOSSOS-EU road model (Annex II)
//!
//! Per-band Lw is derived from CNOSSOS CAT1 / CAT2 / CAT3 coefficients
//! evaluated at 20 km/h, CNOSSOS' lowest validated speed (below 20 km/h
//! the formula extrapolates). Class → CNOSSOS category mapping reflects
//! axle mass per the CNOSSOS classification:
//!
//! | Class  | Analog        | CNOSSOS cat | Mass    | Representative      |
//! |--------|---------------|-------------|---------|---------------------|
//! | LIGHT  | passenger car | CAT1        | <3.5 t  | follow-me, meteo    |
//! | MEDIUM | medium truck  | CAT2        | 3.5-12t | fuel cart, van      |
//! | HEAVY  | heavy truck   | CAT3        | >12 t   | pushback, ARFF, sweeper |
//!
//! CNOSSOS road noise at low speed is conservative for GSE: it captures
//! tyre-road and engine but misses hydraulic-pump / PTO / aircraft-push
//! load. AEDT 3.x GSE database measures these directly and reports
//! 5-10 dB higher levels per equipment code. Treat these values as a
//! v1 floor; future calibration commits may swap in AEDT measurements
//! per equipment type. Aircraft NPD (NUM_CLASSES in
//! `profiles_generated`) is untouched — GSE lives in its own table.
//!
//! ## Reference
//!
//! Bands are 8 octaves centred at 63 / 125 / 250 / 500 / 1k / 2k / 4k / 8k Hz.
//! Lw is sound power level at the source (not at a specific distance);
//! distance attenuation, atmospheric absorption, and A-weighting are
//! applied in the propagation stage, identical to `road.rs`.

use crate::types::NUM_BANDS;

pub const NUM_GSE_CLASSES: usize = 3;
pub const GSE_CLASS_LIGHT: u8 = 0;
pub const GSE_CLASS_MEDIUM: u8 = 1;
pub const GSE_CLASS_HEAVY: u8 = 2;

/// Per-class octave-band sound-power level Lw (dB) at low operating
/// speed (CNOSSOS validated floor, 20 km/h). Lw is per-second
/// sustained emission; per-event SEL depends on propagation
/// distance, duration, and is computed by the consumer.
pub static GSE_LW_BANDS_DB: [[f64; NUM_BANDS]; NUM_GSE_CLASSES] = [
    // LIGHT  — CNOSSOS CAT1 @ 20 km/h (~89 dB(A) Lw total)
    [98.9, 87.6, 85.4, 83.4, 84.1, 83.5, 78.9, 71.4],
    // MEDIUM — CNOSSOS CAT2 @ 20 km/h (~100 dB(A) Lw total)
    [106.9, 96.9, 96.0, 95.0, 96.7, 93.3, 86.6, 80.4],
    // HEAVY  — CNOSSOS CAT3 @ 20 km/h (~103 dB(A) Lw total)
    [108.8, 102.1, 100.2, 99.9, 99.4, 95.1, 90.4, 84.1],
];

/// Mode-S ICAO 24-bit address ranges OBSERVED to carry ground vehicles.
///
/// **Not** an authoritative ICAO sub-allocation — Czech CAA does not
/// publish per-purpose sub-blocks. The CZ aviation block is
/// `0x498000..=0x49FFFF`; within it, ADS-B data for LKPR/LKKB shows
/// `0x49F000..=0x49F1FF` carrying PLET pushback, POZAR ARFF, FOLLOW-ME,
/// meteorology, and bird-control transponders — alongside some GA. This
/// range is a Stage-0 *tiebreaker only*: typecode evidence (`t=GND`,
/// callsign prefix) must come first, with the ICAO check filling in
/// when typecode is missing but the address sits in this empirical
/// allowlist. Treating it as a standalone classifier would
/// misclassify the GA aircraft that share the range.
pub static GND_VEHICLE_ICAO_RANGES: &[(u32, u32)] = &[
    (0x49F000, 0x49F1FF), // CZ — LKPR/LKKB observed ground fleet (not exclusive)
];

pub fn icao_is_ground_vehicle(icao24: u32) -> bool {
    GND_VEHICLE_ICAO_RANGES
        .iter()
        .any(|(lo, hi)| icao24 >= *lo && icao24 <= *hi)
}

/// Callsign prefix → GSE class lookup.
///
/// Prefixes are LKPR/LKKB-specific (Czech operator). Unknown callsigns
/// fall through to `GSE_CLASS_MEDIUM` — a conservative midpoint when
/// the vehicle role is ambiguous (most fleets are dominated by tractors
/// + tugs, which are MEDIUM-class).
static GSE_CALLSIGN_MAP: &[(&str, u8)] = &[
    ("POZAR", GSE_CLASS_HEAVY), // ARFF / fire truck
    ("PLET", GSE_CLASS_HEAVY),  // pushback tractor — 30-50 t narrow-body
    // class places it in CNOSSOS CAT3 by axle
    // mass, not CAT2 (3/4 /gg reviewers).
    ("FOLLOW", GSE_CLASS_LIGHT),  // follow-me car
    ("UDRZBA", GSE_CLASS_MEDIUM), // maintenance — defensible mid default
    ("METEO", GSE_CLASS_LIGHT),   // meteorology
    ("PTACNIK", GSE_CLASS_LIGHT), // bird control (vehicle only — pyro impulses unmodelled)
    ("EMIL", GSE_CLASS_LIGHT),    // ramp coordination
];

pub fn classify_gse_callsign(callsign: &str) -> u8 {
    let cs = callsign.trim().to_ascii_uppercase();
    for (prefix, class) in GSE_CALLSIGN_MAP {
        if cs.starts_with(prefix) {
            return *class;
        }
    }
    GSE_CLASS_MEDIUM
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn classify_pozar_heavy() {
        assert_eq!(classify_gse_callsign("POZAR4"), GSE_CLASS_HEAVY);
        assert_eq!(classify_gse_callsign("pozar9"), GSE_CLASS_HEAVY);
        assert_eq!(classify_gse_callsign("  POZAR21  "), GSE_CLASS_HEAVY);
    }

    #[test]
    fn classify_plet_heavy() {
        assert_eq!(classify_gse_callsign("PLET1"), GSE_CLASS_HEAVY);
        assert_eq!(classify_gse_callsign("PLET 2"), GSE_CLASS_HEAVY);
    }

    #[test]
    fn classify_follow_light() {
        assert_eq!(classify_gse_callsign("FOLLOW3"), GSE_CLASS_LIGHT);
        assert_eq!(classify_gse_callsign("FOLLOWME"), GSE_CLASS_LIGHT);
    }

    #[test]
    fn classify_unknown_defaults_medium() {
        assert_eq!(classify_gse_callsign("UNKNOWN42"), GSE_CLASS_MEDIUM);
        assert_eq!(classify_gse_callsign(""), GSE_CLASS_MEDIUM);
        assert_eq!(classify_gse_callsign("   "), GSE_CLASS_MEDIUM);
    }

    #[test]
    fn icao_cz_range_recognised() {
        assert!(icao_is_ground_vehicle(0x49F000));
        assert!(icao_is_ground_vehicle(0x49F100));
        assert!(icao_is_ground_vehicle(0x49F1FF));
    }

    #[test]
    fn icao_outside_range_rejected() {
        assert!(!icao_is_ground_vehicle(0x49EFFF), "just below lo edge");
        assert!(!icao_is_ground_vehicle(0x49F200), "just above hi edge");
        assert!(!icao_is_ground_vehicle(0x498F84));
        assert!(!icao_is_ground_vehicle(0x000000));
        assert!(!icao_is_ground_vehicle(0xFFFFFF));
    }

    #[test]
    fn classes_strictly_ordered_loudness() {
        let light = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_LIGHT as usize]);
        let medium = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_MEDIUM as usize]);
        let heavy = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_HEAVY as usize]);
        assert!(light < medium, "LIGHT={light} not < MEDIUM={medium}");
        assert!(medium < heavy, "MEDIUM={medium} not < HEAVY={heavy}");
    }

    #[test]
    fn cnossos_totals_in_expected_band() {
        // Bounds ±2 dB around expected CNOSSOS @ 20 km/h totals
        // (89.3 / 100.2 / 103.3 dB(A) per hand-recalc) — catches a
        // ≥3 dB drift in the band table or a swapped class.
        let light = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_LIGHT as usize]);
        let medium = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_MEDIUM as usize]);
        let heavy = a_weighted_total(&GSE_LW_BANDS_DB[GSE_CLASS_HEAVY as usize]);
        assert!(
            (87.3..91.3).contains(&light),
            "LIGHT={light} outside 87.3-91.3 dB(A)"
        );
        assert!(
            (98.2..102.2).contains(&medium),
            "MEDIUM={medium} outside 98.2-102.2 dB(A)"
        );
        assert!(
            (101.3..105.3).contains(&heavy),
            "HEAVY={heavy} outside 101.3-105.3 dB(A)"
        );
    }
}
