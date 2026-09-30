//! The generated designator mapping: dev4's routing cases for unmapped, light, sailplane, heavy
//! and rotorcraft designators.

use super::profiles_generated::*;

/// Real unmapped ADS-B designators (the top of dev4's 24-day scan) reach their nearest anchor.
#[test]
fn unmapped_designators_reach_their_nearest_anchor() {
    for (designator, anchor) in [
        ("PC12", "DH8D"),
        ("C208", "DH8D"),
        ("C20T", "DH8D"),
        ("C441", "DH8D"),
        ("BE20", "DH8D"),
        ("BE35", "C172"),
        ("BE58", "C172"),
        ("BE76", "C172"),
        ("C130", "DH8D"),
        ("C150", "C172"),
        ("C180", "C172"),
        ("C185", "C172"),
        ("S22T", "C172"),
        ("CL30", "CRJ9"),
        ("CL35", "CRJ9"),
        ("E55P", "CRJ9"),
        ("E545", "CRJ9"),
        ("E110", "DH8D"),
        ("E120", "DH8D"),
        ("GLF4", "CRJ9"),
        ("F900", "CRJ9"),
        ("F2TH", "CRJ9"),
        ("H125", "EC35"),
        ("H145", "EC35"),
        ("RV6", "C172"),
        ("PA46", "C172"),
        ("LJ45", "CRJ9"),
        ("B712", "CRJ9"),
        ("B461", "CRJ9"),
        ("B463", "CRJ9"),
        ("RJ85", "CRJ9"),
    ] {
        assert_eq!(profile_idx(designator), profile_idx(anchor), "{designator}");
        assert_ne!(
            profile_idx(designator),
            FALLBACK_PROFILE_IDX,
            "{designator}"
        );
    }
    for designator in ["XXXX", "ZZZ", "9999", "TUM", "EXOT", ""] {
        assert_eq!(
            profile_idx(designator),
            FALLBACK_PROFILE_IDX,
            "{designator:?}"
        );
    }
}

/// Piston singles and twins, ultralights and touring motor gliders fly on the C172 profile, not
/// the jet-like fallback (+10-25 dB for light types).
#[test]
fn light_aircraft_fly_as_the_c172() {
    let c172 = profile_idx("C172");
    for designator in [
        "DR40", "DR22", "HR20", "P208", "TWEN", "G115", "AA5", "M20T", "TB20", "TOBA", "TAMP",
        "A210", "RALL", "AC11", "AS02", "P06T", "P68", "WT9", "C42", "ULAC", "SIRA", "ECHO",
        "ASTO", "FDCT", "VL3", "PIVI", "BREZ", "EV97", "EVSS", "NG5", "BR23", "CRUZ", "SLG2",
        "SD4", "AAT3", "SHRK", "ALTO", "SAVG", "EUPA", "PARA", "SHIP", "DIMO", "SF25", "G109",
        "AS16",
    ] {
        assert_eq!(profile_idx(designator), c172, "{designator}");
        assert!(
            !is_negligible_noise_typecode(designator),
            "{designator} is powered"
        );
    }
}

/// Sailplanes and balloons are dropped at Stage 0/1; a blank designator is not one of them and
/// keeps the fallback.
#[test]
fn sailplanes_and_balloons_are_negligible_and_blank_is_not() {
    for designator in [
        "GLID", "VENT", "DISC", "DUOD", "NIMB", "JANU", "AS14", "AS20", "AS29", "AS30", "AS31",
        "DG40", "DG1T", "LS8", "LS10", "G103", "PK20", "BALL",
    ] {
        assert!(is_negligible_noise_typecode(designator), "{designator}");
        assert_eq!(profile_idx(designator), profile_idx("C172"), "{designator}");
    }
    for designator in [
        "", " ", "TWR", "GND", "C172", "B738", "AS2T", "AS32", "AS50",
    ] {
        assert!(!is_negligible_noise_typecode(designator), "{designator:?}");
    }
    assert!(is_non_aircraft_typecode("TWR") && is_non_aircraft_typecode(" GND "));
}

/// The pinned heavy class catches the loud heavy family and nothing quieter.
#[test]
fn the_heavy_class_holds_the_loud_heavies_only() {
    let heavy = noise_class_of(profile_idx("B748"));
    assert_eq!(CLASS_NAMES[usize::from(heavy)], "WING_B748");
    for designator in ["B744", "B77W", "MD11", "B741", "B742", "IL76"] {
        assert_eq!(
            noise_class_of(profile_idx(designator)),
            heavy,
            "{designator}"
        );
    }
    for designator in ["B77L", "B772", "B789", "A388", "A346", "B763"] {
        assert_ne!(
            noise_class_of(profile_idx(designator)),
            heavy,
            "{designator}"
        );
    }
    assert_eq!(NUM_CLASSES, 15);
}

/// Only the AS-prefixed rotorcraft designators are helicopters; the IAI Astra is a jet.
#[test]
fn as_prefixed_rotorcraft_stay_helicopters() {
    let helicopter = noise_class_of(profile_idx("EC35"));
    assert_eq!(CLASS_NAMES[usize::from(helicopter)], "HELICOPTER");
    for designator in ["AS50", "AS55", "AS65", "AS32", "AS3B", "UHEL", "GYRO"] {
        assert_eq!(
            noise_class_of(profile_idx(designator)),
            helicopter,
            "{designator}"
        );
    }
    assert_eq!(profile_idx("ASTR"), profile_idx("CRJ9"));
}
