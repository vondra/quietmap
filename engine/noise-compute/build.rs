//! Fail-closed build binding for the optional H0 production selection.

#[path = "src/h0_production_selection.rs"]
#[allow(dead_code)]
mod h0_production_selection;
#[path = "src/h0_production_selection_parser.rs"]
mod h0_production_selection_parser;

use h0_production_selection::{H0ProductionSelection, H0_PRODUCTION_SELECTION_RECORD_PATH};
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/h0_production_selection.rs");
    println!("cargo:rerun-if-changed=src/h0_production_selection_parser.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_H0_PRODUCTION_SELECTION");

    if env::var_os("CARGO_FEATURE_H0_PRODUCTION_SELECTION").is_none() {
        return;
    }
    // A missing rerun-if-changed path is permanently dirty in Cargo. Only a
    // consuming feature watches the exact record; the feature env transition
    // itself invalidates a stock target when selection is enabled later.
    println!("cargo:rerun-if-changed={H0_PRODUCTION_SELECTION_RECORD_PATH}");

    let source = fs::read_to_string(H0_PRODUCTION_SELECTION_RECORD_PATH).unwrap_or_else(|error| {
        panic!(
            "feature `h0-production-selection` requires reviewed `{H0_PRODUCTION_SELECTION_RECORD_PATH}` after terminal H0_QUADRATURE_ACCEPTED: {error}"
        )
    });
    let selection = H0ProductionSelection::parse_and_verify(&source)
        .unwrap_or_else(|error| panic!("invalid H0 production selection record: {error}"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("Cargo provides OUT_DIR"));
    fs::write(
        out.join("h0-production-selection-receipt.txt"),
        selection.render_build_receipt(),
    )
    .expect("write H0 production selection receipt");
}
