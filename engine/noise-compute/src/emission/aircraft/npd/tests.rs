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

/// Doc 29 Eq. 4-3 headline check: B738 at 3,000 ft AFE after cutback
/// (MaxClimb Fn/δ 18,093 lb, rows 16,000/19,000 lb, w 0.6977) reads
/// 93.77 dB SEL at 1,000 ft — not the 99.3 dB max-thrust row.
#[test]
fn test_b738_cutback_interpolation_reads_93_77_db() {
    use crate::emission::aircraft::thrust::{bracket_power, thrust_model_for_class};
    let class = noise_class_of(profile_idx("B738")) as usize;
    let model = thrust_model_for_class(class);
    let (row, w) = bracket_power(&model.dep_power, model.dep_rows, 18_093.0);
    assert_eq!((row, (w * 10_000.0).round() as u32), (2, 6977));
    let sel = NpdLuts::shared().lookup(class, true, row, w, 1000.0_f64.log10());
    assert!(
        (sel - 93.77).abs() < 0.06,
        "B738 cutback SEL@1000ft = {sel}"
    );
    let max_row =
        NpdLuts::shared().lookup(class, true, model.dep_rows - 1, 0.0, 1000.0_f64.log10());
    assert!(
        (max_row - 99.3).abs() < 0.06,
        "B738 max-row SEL@1000ft = {max_row}"
    );
}

/// Cross-check between the two generators: every thrust class's edge power
/// rows must reproduce the ANP merge already checked into PROFILES (max
/// departure row = `departure_sel`, min approach row = `approach_sel`).
/// A wrong ACFT_ID/NPD_ID in the thrust generator fails here at dB scale;
/// the 0.1 dB tolerance is LUT bin sag only (worst at the 25,000 ft edge).
#[test]
fn test_power_row_edges_reproduce_anchor_curves() {
    use crate::emission::aircraft::thrust::thrust_model_for_class;
    let luts = NpdLuts::shared();
    for class in 0..NUM_CLASSES {
        let model = thrust_model_for_class(class);
        if !model.has_thrust {
            continue;
        }
        let anchor = &PROFILES[CLASS_REP_PROFILE_IDX[class] as usize];
        for (node, curve) in NPD_DIST_FT.iter().zip(anchor.departure_sel.iter()) {
            let actual = luts.lookup(class, true, model.dep_rows - 1, 0.0, node.log10());
            assert!(
                (actual - curve).abs() < 0.1,
                "{} dep edge @ {node} ft: {actual} vs {curve}",
                model.class_name
            );
        }
        for (node, curve) in NPD_DIST_FT.iter().zip(anchor.approach_sel.iter()) {
            let actual = luts.lookup(class, false, 0, 0.0, node.log10());
            assert!(
                (actual - curve).abs() < 0.1,
                "{} app edge @ {node} ft: {actual} vs {curve}",
                model.class_name
            );
        }
    }
}

/// Pinned classes (fallback, piston, turboprop, helicopter) read their
/// anchor curve at row 0: bit-identical to the pre-thrust LUT at bin
/// centers (frac = 0 takes the exact bin value).
#[test]
fn test_pinned_rows_match_anchor_lut_bit_for_bit() {
    use crate::emission::aircraft::thrust::thrust_model_for_class;
    let luts = NpdLuts::shared();
    for class in 0..NUM_CLASSES {
        if thrust_model_for_class(class).has_thrust {
            continue;
        }
        let anchor = &PROFILES[CLASS_REP_PROFILE_IDX[class] as usize];
        for departure in [false, true] {
            for bin in [0, 1, 64, 127, 128] {
                let log_d = NPD_LUT_LOG_MIN + bin as f64 * NPD_LUT_STEP;
                assert_eq!(
                    luts.lookup(class, departure, 0, 0.0, log_d),
                    interpolate_sel_logd(anchor, log_d, departure),
                    "class {class} dep={departure} bin {bin}"
                );
            }
        }
    }
}

/// Reach envelopes the loudest power row per operation: departure reach is
/// unchanged (max row = today's curve), approach reach grows (min row →
/// loudest approach row) so interpolated approach segments never gate out.
#[test]
fn test_reach_envelopes_loudest_power_row() {
    use crate::emission::aircraft::thrust::thrust_model_for_class;
    for class in 0..NUM_CLASSES {
        let anchor = &PROFILES[CLASS_REP_PROFILE_IDX[class] as usize];
        let model = thrust_model_for_class(class);
        let old_dep = anchor.estimate_reach_m(AIRCRAFT_NPD_REACH_THRESHOLD_DB, true);
        let old_app = anchor.estimate_reach_m(AIRCRAFT_NPD_REACH_THRESHOLD_DB, false);
        let new_dep = REACH_SQ_TABLE[class][1].sqrt();
        let new_app = REACH_SQ_TABLE[class][0].sqrt();
        if model.has_thrust {
            assert_eq!(new_dep, old_dep, "{} dep reach moved", model.class_name);
            assert!(new_app >= old_app, "{} app reach shrank", model.class_name);
        } else {
            assert_eq!(new_dep, old_dep, "{} dep reach moved", model.class_name);
            assert_eq!(new_app, old_app, "{} app reach moved", model.class_name);
        }
    }
}
