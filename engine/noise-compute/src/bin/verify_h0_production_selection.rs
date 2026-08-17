//! Verify the checked-in H0 selection against compiled Rust constants.

use noise_compute::h0_production_selection::{
    H0ProductionSelection, H0ThetaCapPair, H0_PRODUCTION_SELECTION_RECORD_PATH, H0_THETA_CAP_PAIRS,
};
use noise_compute::h0_production_selection_record as compiled;
use serde::Serialize;
use std::{fs, path::Path};

#[derive(Serialize)]
struct ThetaCapJson {
    theta_degrees: u8,
    theta_radians_bits: String,
    node_cap: usize,
}

#[derive(Serialize)]
struct SelectionJson {
    status: &'static str,
    schema: u32,
    epoch: u64,
    mechanism: String,
    allowed_pairs: Vec<ThetaCapJson>,
    selected: ThetaCapJson,
    h_max: usize,
    v3_verdict_sha256: String,
    v3_root_manifest_sha256: String,
    v3_analyzer_sha256: String,
    streaming_design_sha256: String,
    model_v2_plan_sha256: String,
    role_integration_design_sha256: String,
}

fn pair_json(pair: H0ThetaCapPair) -> ThetaCapJson {
    ThetaCapJson {
        theta_degrees: pair.theta_degrees,
        theta_radians_bits: format!("0x{:016x}", pair.theta_radians_bits),
        node_cap: pair.node_cap,
    }
}

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let record_path = manifest_dir.join(H0_PRODUCTION_SELECTION_RECORD_PATH);
    let source = fs::read_to_string(&record_path)
        .unwrap_or_else(|error| panic!("{} is not readable: {error}", record_path.display()));
    let selected = H0ProductionSelection::parse_and_verify(&source)
        .unwrap_or_else(|error| panic!("invalid selection record: {error}"));

    assert_eq!(selected.schema, compiled::H0_PRODUCTION_SELECTION_SCHEMA);
    assert_eq!(selected.epoch, compiled::H0_PRODUCTION_SELECTION_EPOCH);
    assert_eq!(selected.mechanism, compiled::H0_PRODUCTION_MECHANISM);
    assert_eq!(
        selected.theta_degrees,
        compiled::H0_PRODUCTION_THETA_DEGREES
    );
    assert_eq!(
        selected.theta_radians_bits,
        compiled::H0_PRODUCTION_THETA_RADIANS_BITS
    );
    assert_eq!(selected.h_max, compiled::H0_PRODUCTION_H_MAX);
    assert_eq!(selected.node_cap, compiled::H0_PRODUCTION_NODE_CAP);
    assert_eq!(
        selected.v3_verdict_sha256,
        compiled::H0_PRODUCTION_V3_VERDICT_SHA256
    );
    assert_eq!(
        selected.v3_root_manifest_sha256,
        compiled::H0_PRODUCTION_V3_ROOT_MANIFEST_SHA256
    );
    assert_eq!(
        selected.v3_analyzer_sha256,
        compiled::H0_PRODUCTION_V3_ANALYZER_SHA256
    );
    assert_eq!(
        selected.streaming_design_sha256,
        compiled::H0_PRODUCTION_STREAMING_DESIGN_SHA256
    );
    assert_eq!(
        selected.model_v2_plan_sha256,
        compiled::H0_PRODUCTION_MODEL_V2_PLAN_SHA256
    );
    assert_eq!(
        selected.role_integration_design_sha256,
        compiled::H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256
    );

    let output = SelectionJson {
        status: "H0_PRODUCTION_SELECTION_VERIFIED",
        schema: selected.schema,
        epoch: selected.epoch,
        mechanism: selected.mechanism.clone(),
        allowed_pairs: H0_THETA_CAP_PAIRS.into_iter().map(pair_json).collect(),
        selected: pair_json(H0ThetaCapPair {
            theta_degrees: selected.theta_degrees,
            theta_radians_bits: selected.theta_radians_bits,
            node_cap: selected.node_cap,
        }),
        h_max: selected.h_max,
        v3_verdict_sha256: selected.v3_verdict_sha256,
        v3_root_manifest_sha256: selected.v3_root_manifest_sha256,
        v3_analyzer_sha256: selected.v3_analyzer_sha256,
        streaming_design_sha256: selected.streaming_design_sha256,
        model_v2_plan_sha256: selected.model_v2_plan_sha256,
        role_integration_design_sha256: selected.role_integration_design_sha256,
    };
    println!(
        "{}",
        serde_json::to_string(&output).expect("selection receipt serializes")
    );
}
