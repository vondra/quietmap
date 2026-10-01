//! The generated designator mapping: dev4's routing cases for unmapped, light, sailplane, heavy
//! and rotorcraft designators.

use super::profiles_generated::*;

/// Real unmapped ADS-B designators (the top of dev4's 24-day scan, and of four world days of the
/// year run) reach their nearest ANP aircraft; business jets by weight (Learjets the Learjet 35,
/// Phenoms the Citation III, Challenger 300s and Falcon 900/2000s the Challenger 601).
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
        ("CL30", "CL60"),
        ("CL35", "CL60"),
        ("E55P", "C56X"),
        ("E545", "C56X"),
        ("E110", "DH8D"),
        ("E120", "DH8D"),
        ("GLF4", "CRJ9"),
        ("F900", "CL60"),
        ("F2TH", "CL60"),
        ("H125", "EC35"),
        ("H145", "EC35"),
        ("RV6", "C172"),
        ("PA46", "C172"),
        ("LJ45", "LJ60"),
        ("B712", "CRJ9"),
        ("B461", "CRJ9"),
        ("B463", "CRJ9"),
        ("RJ85", "CRJ9"),
        // r051: the most flown designators dev4 left on the fallback.
        ("T206", "C172"),
        ("PIAT", "C172"),
        ("TEX2", "DH8D"),
        ("SW4", "DH8D"),
        ("DHC6", "L410"),
        ("F100", "F70"),
        ("B762", "B763"),
        ("CRJX", "CRJ9"),
        ("GA6C", "GLF6"),
        ("HDJT", "C56X"),
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

/// Every designator reads its own ANP aircraft, or the proxy its mapping names: dev4's 15
/// classes had flown the 737-700 on the fallback curve, the A300-600 and the A330-200 as 737-800s,
/// the E175 as an A320, the 767-300 as a 787-8 and the ATR 72 as a Dash 8-300.
#[test]
fn designators_read_their_own_anp_aircraft() {
    let class_name = |designator| CLASS_NAMES[usize::from(noise_class_of(profile_idx(designator)))];
    for (designator, aircraft) in [
        ("B738", "737800"),
        ("B737", "737700"),
        ("A306", "A300-622R"),
        ("A332", "A330-301"),
        ("A359", "A350-941"),
        ("E75L", "EMB175"),
        ("E190", "EMB190"),
        ("B763", "7673ER"),
        ("B789", "7879"),
        ("B77W", "7773ER"),
        ("A388", "A380-841"),
        ("AT76", "ATR72"),
        ("AT43", "ATR72"),
        ("DH8D", "DHC830"),
        ("DH8C", "DHC830"),
        ("SF34", "SF340"),
        ("PC12", "DHC830"),
        ("C172", "CNA172"),
        ("PA44", "PA30"),
        ("DA42", "CNA172"),
        ("BCS3", "A320-270N"),
        ("GLF6", "CRJ9-ER"),
    ] {
        assert_eq!(class_name(designator), aircraft, "{designator}");
    }
    assert_eq!(class_name("ZZZZ"), "WING_FALLBACK");
    assert_eq!(NUM_CLASSES, 55);
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
    assert_eq!(profile_idx("ASTR"), profile_idx("C56X"));
}
