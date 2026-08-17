//! Mutation tests for the fail-closed H0 production selection authority.

use noise_compute::compute::element::{
    derived_node_cap as engine_derived_node_cap, LINE_MAX_LENGTH_M,
};
use noise_compute::h0_production_selection::{
    derived_node_cap, theta_cap_pair, H0ProductionSelection,
    H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256, H0_STREAMING_DESIGN_SHA256, H0_THETA_CAP_PAIRS,
    MODEL_V2_PLAN_SHA256,
};

const VERDICT_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ROOT_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const ANALYZER_SHA: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn selection(theta_degrees: u8) -> H0ProductionSelection {
    let pair = theta_cap_pair(theta_degrees).unwrap();
    H0ProductionSelection {
        schema: 1,
        epoch: 7,
        mechanism: "H0StreamingReduction".to_owned(),
        theta_degrees,
        theta_radians_bits: pair.theta_radians_bits,
        h_max: 0,
        node_cap: pair.node_cap,
        v3_verdict_sha256: VERDICT_SHA.to_owned(),
        v3_root_manifest_sha256: ROOT_SHA.to_owned(),
        v3_analyzer_sha256: ANALYZER_SHA.to_owned(),
        streaming_design_sha256: H0_STREAMING_DESIGN_SHA256.to_owned(),
        model_v2_plan_sha256: MODEL_V2_PLAN_SHA256.to_owned(),
        role_integration_design_sha256: H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256.to_owned(),
    }
}

fn replace_once(source: &str, old: &str, new: &str) -> String {
    assert_eq!(
        source.matches(old).count(),
        1,
        "mutation target must be unique"
    );
    source.replacen(old, new, 1)
}

fn record_source(selection: &H0ProductionSelection) -> String {
    format!(
        "/// Reviewed numerical authority for the selected production H0 line model.\n\
         pub const H0_PRODUCTION_SELECTION_SCHEMA: u32 = {};\n\
         pub const H0_PRODUCTION_SELECTION_EPOCH: u64 = {};\n\
         pub const H0_PRODUCTION_MECHANISM: &str = \"{}\";\n\
         pub const H0_PRODUCTION_THETA_DEGREES: u8 = {};\n\
         pub const H0_PRODUCTION_THETA_RADIANS_BITS: u64 = 0x{:016x};\n\
         pub const H0_PRODUCTION_H_MAX: usize = {};\n\
         pub const H0_PRODUCTION_NODE_CAP: usize = {};\n\
         pub const H0_PRODUCTION_V3_VERDICT_SHA256: &str = \"{}\";\n\
         pub const H0_PRODUCTION_V3_ROOT_MANIFEST_SHA256: &str = \"{}\";\n\
         pub const H0_PRODUCTION_V3_ANALYZER_SHA256: &str = \"{}\";\n\
         pub const H0_PRODUCTION_STREAMING_DESIGN_SHA256: &str = \"{}\";\n\
         pub const H0_PRODUCTION_MODEL_V2_PLAN_SHA256: &str = \"{}\";\n\
         pub const H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256: &str = \"{}\";\n",
        selection.schema,
        selection.epoch,
        selection.mechanism,
        selection.theta_degrees,
        selection.theta_radians_bits,
        selection.h_max,
        selection.node_cap,
        selection.v3_verdict_sha256,
        selection.v3_root_manifest_sha256,
        selection.v3_analyzer_sha256,
        selection.streaming_design_sha256,
        selection.model_v2_plan_sha256,
        selection.role_integration_design_sha256,
    )
}

#[test]
fn all_reviewed_theta_bits_and_theorem_caps_are_independently_pinned() {
    assert_eq!(
        H0_THETA_CAP_PAIRS.map(|pair| (pair.theta_degrees, pair.theta_radians_bits, pair.node_cap)),
        [
            (5, 0x3fb6_5718_4ae7_4487, 40),
            (4, 0x3fb1_df46_a252_9d39, 50),
            (3, 0x3faa_cee9_f37b_ebd5, 66),
            (2, 0x3fa1_df46_a252_9d39, 99),
        ]
    );
    for pair in H0_THETA_CAP_PAIRS {
        assert_eq!(
            derived_node_cap(f64::from_bits(pair.theta_radians_bits)),
            pair.node_cap
        );
        assert_eq!(
            engine_derived_node_cap(
                f64::from_bits(pair.theta_radians_bits),
                0,
                LINE_MAX_LENGTH_M,
            ),
            pair.node_cap,
            "selection verifier and live element theorem must share their constants",
        );
        let selected = selection(pair.theta_degrees);
        let record = record_source(&selected);
        assert_eq!(
            H0ProductionSelection::parse_and_verify(&record).unwrap(),
            selected
        );
    }
}

#[test]
fn canonical_record_rejects_structural_mutations() {
    let record = record_source(&selection(3));
    let mut missing = record.lines().collect::<Vec<_>>();
    missing.remove(8);
    let missing = format!("{}\n", missing.join("\n"));
    assert!(H0ProductionSelection::parse_and_verify(&missing).is_err());

    let mut reordered = record.lines().collect::<Vec<_>>();
    reordered.swap(8, 9);
    let reordered = format!("{}\n", reordered.join("\n"));
    assert!(H0ProductionSelection::parse_and_verify(&reordered).is_err());

    let duplicate = format!(
        "{}{}\n",
        record,
        record.lines().next().expect("record has module doc")
    );
    assert!(H0ProductionSelection::parse_and_verify(&duplicate).is_err());
    assert!(H0ProductionSelection::parse_and_verify(record.trim_end()).is_err());
    assert!(H0ProductionSelection::parse_and_verify(&record.replace('\n', "\r\n")).is_err());
}

#[test]
fn canonical_record_rejects_unselected_or_inconsistent_values() {
    let record = record_source(&selection(3));
    for mutated in [
        replace_once(
            &record,
            "H0_PRODUCTION_SELECTION_SCHEMA: u32 = 1",
            "H0_PRODUCTION_SELECTION_SCHEMA: u32 = 2",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_SELECTION_EPOCH: u64 = 7",
            "H0_PRODUCTION_SELECTION_EPOCH: u64 = 0",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_MECHANISM: &str = \"H0StreamingReduction\"",
            "H0_PRODUCTION_MECHANISM: &str = \"RetainedIntervals\"",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_THETA_DEGREES: u8 = 3",
            "H0_PRODUCTION_THETA_DEGREES: u8 = 6",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_THETA_RADIANS_BITS: u64 = 0x3faacee9f37bebd5",
            "H0_PRODUCTION_THETA_RADIANS_BITS: u64 = 0x3faacee9f37bebd4",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_H_MAX: usize = 0",
            "H0_PRODUCTION_H_MAX: usize = 32",
        ),
        replace_once(
            &record,
            "H0_PRODUCTION_NODE_CAP: usize = 66",
            "H0_PRODUCTION_NODE_CAP: usize = 65",
        ),
    ] {
        assert!(H0ProductionSelection::parse_and_verify(&mutated).is_err());
    }
}

#[test]
fn canonical_record_rejects_unsealed_hash_authority() {
    let record = record_source(&selection(3));
    for mutated in [
        replace_once(&record, VERDICT_SHA, &"0".repeat(64)),
        replace_once(&record, ROOT_SHA, &"A".repeat(64)),
        replace_once(&record, ANALYZER_SHA, "abcd"),
        replace_once(
            &record,
            H0_STREAMING_DESIGN_SHA256,
            "4444444444444444444444444444444444444444444444444444444444444444",
        ),
        replace_once(
            &record,
            MODEL_V2_PLAN_SHA256,
            "5555555555555555555555555555555555555555555555555555555555555555",
        ),
        replace_once(
            &record,
            H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256,
            "6666666666666666666666666666666666666666666666666666666666666666",
        ),
    ] {
        assert!(H0ProductionSelection::parse_and_verify(&mutated).is_err());
    }
}

#[test]
fn canonical_record_rejects_noncanonical_rust_literals() {
    let record = record_source(&selection(3));
    for mutated in [
        replace_once(
            &record,
            "H0_PRODUCTION_SELECTION_EPOCH: u64 = 7",
            "H0_PRODUCTION_SELECTION_EPOCH: u64 = 07",
        ),
        replace_once(&record, "0x3faacee9f37bebd5", "0x3FAACEE9F37BEBD5"),
        replace_once(&record, "0x3faacee9f37bebd5", "0x3faa_cee9_f37b_ebd5"),
        replace_once(
            &record,
            "pub const H0_PRODUCTION_H_MAX",
            "const H0_PRODUCTION_H_MAX",
        ),
    ] {
        assert!(H0ProductionSelection::parse_and_verify(&mutated).is_err());
    }
}
