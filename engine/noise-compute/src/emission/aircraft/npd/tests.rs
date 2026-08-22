use super::*;

#[test]
fn test_npd_at_table_node() {
    // Interpolating exactly at a table distance must return that
    // table's value (no rounding drift through log-linear). Anchor on
    // the B738 approach SEL @ 1000 ft (= NPD_DIST_FT[3]).
    let p = &PROFILES[profile_idx("B738") as usize];
    let sel = interpolate_sel(p, 1000.0, false);
    assert!(
        (sel - p.approach_sel[3]).abs() < 0.01,
        "B738 @ 1000ft: expected {}, got {sel}",
        p.approach_sel[3]
    );
}

#[test]
fn test_npd_interpolation() {
    let p = &PROFILES[profile_idx("B738") as usize];
    let sel = interpolate_sel(p, 1500.0, false);
    let lo = p.approach_sel[3].min(p.approach_sel[4]);
    let hi = p.approach_sel[3].max(p.approach_sel[4]);
    assert!(
        sel >= lo - 0.01 && sel <= hi + 0.01,
        "B738 @ 1500ft = {sel} outside [{lo},{hi}]"
    );
}

#[test]
fn test_npd_extrapolation_below() {
    let p = &PROFILES[profile_idx("B738") as usize];
    let sel = interpolate_sel(p, 100.0, false);
    let near = p.approach_sel[0];
    assert!(
        sel > near,
        "Should extrapolate above approach_sel[0]={near}, got {sel}"
    );
}

#[test]
fn test_npd_extrapolation_above() {
    // 50 000 ft = 15 240 m slant, beyond the NPD table.
    let sel = interpolate_sel(&PROFILES[0], 50000.0, false);
    assert!(
        sel < 45.0 && sel > 30.0,
        "Physics extrapolation @ 15 km should be 30–45 dB, got {sel}"
    );
}

#[test]
fn test_npd_continuity_at_tail() {
    for profile in &PROFILES {
        for is_dep in [false, true] {
            let tail_sel = if is_dep {
                profile.departure_sel[9]
            } else {
                profile.approach_sel[9]
            };
            let computed = interpolate_sel(profile, 25000.0, is_dep);
            assert!(
                (computed - tail_sel).abs() < 0.05,
                "{} {} @ 25 000 ft: expected {tail_sel}, got {computed}",
                profile.name,
                if is_dep { "dep" } else { "app" }
            );
        }
    }
}

#[test]
fn test_alpha_eff_back_out() {
    for profile in &PROFILES {
        for is_dep in [false, true] {
            let alpha = profile.alpha_eff(is_dep);
            assert!(
                (-0.0005..=0.0030).contains(&alpha),
                "{} {} α_eff = {alpha:.6} dB/m, expected -0.0005..0.0030",
                profile.name,
                if is_dep { "dep" } else { "app" }
            );
        }
    }
    let b738_app = PROFILES[profile_idx("B738") as usize].alpha_eff(false);
    assert!(
        (0.0..0.003).contains(&b738_app),
        "B738 approach α_eff = {b738_app:.5}"
    );
}

#[test]
fn test_estimate_reach_lightga_shorter_than_jets() {
    let ga = PROFILES[profile_idx("C172") as usize].estimate_reach_m(40.0, false);
    let b738 = PROFILES[profile_idx("B738") as usize].estimate_reach_m(40.0, false);
    assert!(
        ga < b738,
        "GA reach ({ga:.0}) should be shorter than B738 ({b738:.0})"
    );
}

#[test]
fn test_estimate_reach_jets_physics() {
    for tc in ["B738", "A320", "A321", "B772", "A359"] {
        let p = &PROFILES[profile_idx(tc) as usize];
        let reach = p.estimate_reach_m(40.0, false);
        assert!(
            reach > 4_000.0 && reach <= AIRCRAFT_NPD_REACH_CAP_M,
            "{tc} approach reach at 40 dB should be 4–16 km, got {reach:.0}"
        );
    }
}

#[test]
fn test_estimate_reach_departure_ge_approach() {
    for profile in &PROFILES {
        let dep = profile.estimate_reach_m(40.0, true);
        let app = profile.estimate_reach_m(40.0, false);
        assert!(
            dep >= app - 1.0,
            "{}: departure reach ({dep:.0}) should >= approach ({app:.0})",
            profile.name
        );
    }
}

#[test]
fn test_estimate_reach_higher_threshold_shorter() {
    let reach_40 = PROFILES[6].estimate_reach_m(40.0, false);
    let reach_50 = PROFILES[6].estimate_reach_m(50.0, false);
    assert!(
        reach_50 < reach_40,
        "Higher threshold should give shorter reach: 50dB={reach_50:.0} vs 40dB={reach_40:.0}"
    );
}

/// `similarity_fallback` invoked via `profile_idx` for unmapped typecodes
/// must route real ADSB typecodes (sampled from the 24-day scan, top
/// unmapped by traffic) onto the closest anchor profile.
#[test]
fn test_similarity_fallback_top_unmapped() {
    let cases: &[(&str, &str)] = &[
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
    ];
    for (typecode, expected_anchor) in cases {
        let got = profile_idx(typecode);
        let expected = profile_idx(expected_anchor);
        assert_eq!(
            got, expected,
            "{} → idx {} but expected anchor {} (idx {})",
            typecode, got, expected_anchor, expected
        );
        assert_ne!(
            got, FALLBACK_PROFILE_IDX,
            "{} should not fall through to FALLBACK_PROFILE_IDX",
            typecode
        );
    }
}

#[test]
fn test_similarity_fallback_unknown_still_fallback() {
    for typecode in ["XXXX", "ZZZ", "9999", "TUM", "EXOT"] {
        assert_eq!(profile_idx(typecode), FALLBACK_PROFILE_IDX, "{}", typecode);
    }
}

/// C10a (audit 2026-06 airborne A1): GA piston singles, light piston
/// twins, powered ultralights and touring motor gliders route to the
/// C172 profile instead of the FALLBACK energy-mean (+10..25 dB hot
/// for light types). Every code verified against ICAO 8643.
#[test]
fn test_similarity_ga_and_ultralight_to_c172() {
    let c172 = profile_idx("C172");
    for tc in [
        // GA piston singles + light twins
        "DR40", "DR22", "HR20", "P208", "TWEN", "G115", "AA5", "M20T", "TB20", "TOBA", "TAMP",
        "A210", "RALL", "AC11", "AS02", "P06T", "P68",
        // ultralights / LSA (Rotax class)
        "WT9", "C42", "ULAC", "SIRA", "ECHO", "ASTO", "FDCT", "VL3", "PIVI", "BREZ", "EV97", "EVSS",
        "NG5", "BR23", "CRUZ", "SLG2", "SD4", "AAT3", "SHRK", "ALTO", "SAVG", "EUPA",
        // ICAO 8643 special designators that cruise engine-on
        // (GYRO is strict-mapped to HELICOPTER — pinned in the heli test)
        "PARA", "SHIP", // touring motor gliders (engine-on cruise)
        "DIMO", "SF25", "G109", "AS16",
    ] {
        assert_eq!(profile_idx(tc), c172, "{tc} should route to C172");
        assert!(
            !is_negligible_noise_typecode(tc),
            "{tc} is powered — not negligible"
        );
    }
}

/// Sailplanes + balloons are dropped at Stage 0/1 via
/// `is_negligible_noise_typecode`.
/// Blank typecode is NOT a glider — it keeps the deliberate Apr-29
/// FALLBACK energy-mean semantics for truly-unknown types. The
/// similarity arm additionally keeps glider codes off the GL*→CRJ9 /
/// helicopter prefixes if any caller ever evaluates them.
#[test]
fn test_gliders_negligible_blank_stays_fallback() {
    for tc in [
        "GLID", "VENT", "DISC", "DUOD", "NIMB", "JANU", "AS14", "AS20", "AS21", "AS22", "AS24",
        "AS25", "AS26", "AS28", "AS29", "AS30", "AS31", "DG40", "DG50", "DG60", "DG80", "DG1T",
        "LS8", "LS9", "LS10", "G103", "PK20", "BALL",
    ] {
        assert!(
            is_negligible_noise_typecode(tc),
            "{tc} must be negligible-noise"
        );
        assert_eq!(
            profile_idx(tc),
            profile_idx("C172"),
            "{tc} defense-in-depth arm must hit the quiet profile, not GL*/AS* prefixes"
        );
    }
    for tc in [
        "", " ", "TWR", "GND", "C172", "B738", "AS2T", "AS32", "AS3B", "AS50",
    ] {
        assert!(
            !is_negligible_noise_typecode(tc),
            "{tc:?} must NOT be negligible"
        );
    }
    assert_eq!(
        profile_idx(""),
        FALLBACK_PROFILE_IDX,
        "blank stays on the energy-mean"
    );
}

/// C10b (audit 2026-06): the pinned WING_B748 class catches the
/// rare-but-loud heavy family that traffic-ranked Voronoi never gives
/// a representative — they sat 3–8 dB above their nearest passenger
/// anchor (B744 was scored as a B789, B741/B742/IL76 even as an A321
/// narrowbody). Membership is still pure L∞ nearest-anchor: quieter
/// wide-bodies must NOT get pulled in.
#[test]
fn test_pinned_heavy_class_membership() {
    let heavy = noise_class_of(profile_idx("B748"));
    assert_eq!(CLASS_NAMES[heavy as usize], "WING_B748");
    for tc in ["B744", "B77W", "MD11", "B741", "B742", "IL76"] {
        assert_eq!(
            noise_class_of(profile_idx(tc)),
            heavy,
            "{tc} belongs to the heavy class"
        );
    }
    for tc in ["B77L", "B772", "B789", "A388", "A346", "B763"] {
        assert_ne!(
            noise_class_of(profile_idx(tc)),
            heavy,
            "{tc} must stay on its quieter anchor"
        );
    }
    assert_eq!(NUM_CLASSES, 15, "14 algorithmic + 1 pinned heavy");
}

/// Hybrid GA-window membership pins.
/// IN: the PROP_C172 piston/UL family and every rotorcraft. OUT:
/// airline turboprops (PROP_DH8D — incl. the PC12 fallback
/// residual), jets, bizjet classes, and the blank-typecode
/// FALLBACK.
#[test]
fn test_ga_sampled_window_membership() {
    for tc in [
        "C172", "DR40", "WT9", "R44", "R22", "GYRO", "UHEL", "EC35", "H125",
    ] {
        assert!(
            is_ga_sampled_profile(profile_idx(tc)),
            "{tc} must be 365-day GA-window sampled"
        );
    }
    for tc in [
        "AT72", "AT76", "DH8D", "SF34", "L410", // airline turboprops
        "B738", "A320", "B748", // jets
        "CRJ9", "C56X", "GLF4", "CL30", // regional-jet/bizjet classes
        "PC12", "C208", // GA turbine singles → DH8D residual
    ] {
        assert!(
            !is_ga_sampled_profile(profile_idx(tc)),
            "{tc} must stay on the 12-day airline window"
        );
    }
    assert!(
        !is_ga_sampled_profile(FALLBACK_PROFILE_IDX),
        "blank/unknown typecodes stay 12-day (blank ≠ GA)"
    );
    // Class-level predicate agrees with the name-keyed definition and
    // rejects out-of-range indices instead of panicking.
    for (idx, name) in CLASS_NAMES.iter().enumerate() {
        assert_eq!(
            is_ga_sampled_class(idx as u8),
            matches!(*name, "PROP_C172" | "HELICOPTER"),
            "class {idx} ({name})"
        );
    }
    assert!(!is_ga_sampled_class(NUM_CLASSES as u8));
    assert!(!is_ga_sampled_class(u8::MAX));
}

/// Weight-LUT round-trip from the `sample_days_by_class` metadata
/// vector. This synthetic case uses 365 GA days and 12 airline days;
/// live extracts carry their available day count in metadata.
#[test]
fn class_weights_round_trip_from_metadata() {
    // Build a non-uniform hybrid vector: GA classes → 365, else → 12.
    let vec: String = (0..NUM_CLASSES)
        .map(|c| {
            if is_ga_sampled_class(c as u8) {
                "365"
            } else {
                "12"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let w = ClassWeights::parse(Some(&vec), 12).unwrap();
    for c in 0..NUM_CLASSES as u8 {
        let expected = if is_ga_sampled_class(c) {
            12.0 / 365.0
        } else {
            1.0
        };
        assert!(
            (w.get(c) - expected).abs() < 1e-12,
            "class {c} ({}) weight {} != {expected}",
            CLASS_NAMES[c as usize],
            w.get(c)
        );
    }
    // GA classes (C172 piston + helicopter) must carry the 12/365 factor.
    assert!((w.get(noise_class_of(profile_idx("C172"))) - 12.0 / 365.0).abs() < 1e-12);
    assert!((w.get(noise_class_of(profile_idx("R44"))) - 12.0 / 365.0).abs() < 1e-12);
    // Jets / airline turboprops stay at 1.0.
    assert_eq!(w.get(noise_class_of(profile_idx("B738"))), 1.0);
    assert_eq!(w.get(noise_class_of(profile_idx("AT72"))), 1.0);
    // Out-of-range class clamps (no panic).
    assert_eq!(w.get(u8::MAX), w.get(NUM_CLASSES as u8 - 1));
}

/// Single-window (non-hybrid) extract stamps the uniform vector → all
/// weights 1.0 (byte-identical to pre-hybrid energy accounting).
#[test]
fn class_weights_uniform_vector_is_identity() {
    let vec = std::iter::repeat_n("14", NUM_CLASSES)
        .collect::<Vec<_>>()
        .join(",");
    let w = ClassWeights::parse(Some(&vec), 14).unwrap();
    for c in 0..NUM_CLASSES as u8 {
        assert_eq!(w.get(c), 1.0, "uniform vector must weight every class 1.0");
    }
    assert_eq!(ClassWeights::uniform().as_array(), &[1.0; NUM_CLASSES]);
}

/// FAIL-LOUD contract (owner directive 2026-06-12): missing metadata,
/// wrong length, non-integer, and zero-days all error with an
/// operator-actionable message — never a silent uniform fallback.
#[test]
fn class_weights_missing_or_malformed_metadata_errors() {
    // Missing → loud, names the key + the re-run remedy.
    let err = ClassWeights::parse(None, 12).unwrap_err();
    assert!(err.contains(SAMPLE_DAYS_BY_CLASS_KEY), "{err}");
    assert!(err.contains("re-run"), "{err}");
    // Wrong length.
    assert!(ClassWeights::parse(Some("12,12,12"), 12).is_err());
    // Non-integer entry.
    let too = std::iter::repeat_n("x", NUM_CLASSES)
        .collect::<Vec<_>>()
        .join(",");
    assert!(ClassWeights::parse(Some(&too), 12).is_err());
    // Zero day-count = invalid divisor.
    let zero = {
        let mut v = vec!["12"; NUM_CLASSES];
        v[0] = "0";
        v.join(",")
    };
    assert!(ClassWeights::parse(Some(&zero), 12).is_err());
    // GA classes carrying DIFFERENT sample-day counts is malformed —
    // the count split exposes one ga_n_days divisor.
    let mut diverge = vec!["12"; NUM_CLASSES];
    let mut first_ga = true;
    for (i, slot) in diverge.iter_mut().enumerate() {
        if is_ga_sampled_class(i as u8) {
            *slot = if first_ga { "365" } else { "200" };
            first_ga = false;
        }
    }
    let err = ClassWeights::parse(Some(&diverge.join(",")), 12).unwrap_err();
    assert!(err.contains("different sample-day"), "{err}");
}

/// AS* prefix audit (ICAO 8643, 2026-06-11): the only AS-prefixed
/// helicopters are AS32/AS3B (similarity → EC35) and AS50/AS55/AS65
/// (strict-mapped); all must stay in the HELICOPTER class, as must the
/// UHEL ultralight-helicopter special designator. The IAI Astra bizjet
/// (ASTR) lands on CRJ9, not on a phantom helicopter.
#[test]
fn test_as_prefix_helicopters_still_ec35() {
    let heli_class = noise_class_of(profile_idx("EC35"));
    assert_eq!(CLASS_NAMES[heli_class as usize], "HELICOPTER");
    for tc in ["AS50", "AS55", "AS65", "AS32", "AS3B", "UHEL", "GYRO"] {
        assert_eq!(
            noise_class_of(profile_idx(tc)),
            heli_class,
            "{tc} must stay rotorcraft"
        );
    }
    assert_eq!(profile_idx("AS32"), profile_idx("EC35"));
    assert_eq!(profile_idx("AS3B"), profile_idx("EC35"));
    assert_eq!(
        profile_idx("ASTR"),
        profile_idx("CRJ9"),
        "IAI Astra is a bizjet"
    );
}
