//! Fixed-arm CPU field renderer shared by the seven H0 V3 executables.

#[path = "h0_v3_case.rs"]
mod case;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use noise_compute::propagation::h0_v3::{
    H0V3Theta, H0_V3_RAW_CANDIDATES_PER_PAIR_MAX, H_JUDGE_MAX, JUDGE_LOGICAL_HINT_BYTES_MAX,
};
use serde::Serialize;
use tile_painter::h0_pair_reference::H0V3PairArm;
use tile_painter::h0_v3_tile_reference::{evaluate_h0_v3_stock_tile, evaluate_h0_v3_tile};

use case::{map_tile_error, parse_arguments, run_identity, with_loaded_case, write_json};
use noise_gpu::h0_v3_field::{sha256_file_hex, write_h0_v3_field, H0V3FieldArm, H0V3FieldIdentity};

fn arm_name(arm: H0V3PairArm) -> &'static str {
    match arm {
        H0V3PairArm::Production(H0V3Theta::Degrees5) => "h0-5",
        H0V3PairArm::Production(H0V3Theta::Degrees4) => "h0-4",
        H0V3PairArm::Production(H0V3Theta::Degrees3) => "h0-3",
        H0V3PairArm::Production(H0V3Theta::Degrees2) => "h0-2",
        H0V3PairArm::JudgeCoarse => "judge-coarse",
        H0V3PairArm::JudgeFine => "judge-fine",
    }
}

#[derive(Serialize)]
struct SweepReceipt<'a> {
    status: &'static str,
    case: &'a str,
    case_index: usize,
    arm: &'static str,
    zoom: u8,
    tile_x: u32,
    tile_y: u32,
    layer: &'static str,
    region_source_rows: usize,
    barrier_rows: usize,
    obstacle_indexes: usize,
    evaluated_pairs: u64,
    evaluated_nodes: u64,
    admitted_nodes: u64,
    raw_candidate_visits: u64,
    maximum_raw_candidates_per_pair: u64,
    maximum_distinct_hint_records: usize,
    maximum_unique_u_hints: usize,
    maximum_logical_hint_storage_bytes: usize,
    judge_hint_capacity: usize,
    raw_candidate_replay_capacity: usize,
    hostname: &'a str,
    cpu_model: &'a str,
    rayon_num_threads: usize,
    binary_sha256: &'a str,
    field_sha256: &'a str,
    surface_budget_eta: &'static str,
    process_wall_seconds: f64,
}

/// Render the current exact CPU skyline baseline. This arm has a separate
/// executable because it reports the complete stock-model fork and never
/// selects H0. The direct predicate fixture, not this field, owns P2b cause.
pub fn run_stock() -> Result<()> {
    // Every fixed-arm binary compiles this shared module. Keep the sibling
    // entry point type-checked even though this executable cannot call it.
    let _h0_entry_point: fn(H0V3PairArm) -> Result<()> = run;
    let arguments = parse_arguments()?;
    let identity = run_identity()?;
    fs::create_dir(&arguments.output).with_context(|| {
        format!(
            "output must be a fresh directory: {}",
            arguments.output.display()
        )
    })?;
    let started = Instant::now();
    with_loaded_case(&arguments, |loaded| {
        let field = evaluate_h0_v3_stock_tile(
            loaded.tile,
            loaded.rows,
            loaded.barriers,
            Some(loaded.obstacle_set),
        );
        ensure!(field.period_power_f32.len() == 512 * 512);
        let field_path = loaded.arguments.output.join("field.bin");
        write_h0_v3_field(
            &field_path,
            H0V3FieldIdentity {
                case_index: loaded.arguments.case_index as u32,
                arm: H0V3FieldArm::Stock,
            },
            &field,
        )
        .map_err(|error| anyhow::anyhow!("field write failed: {error:?}"))?;
        let field_sha256 = sha256_file_hex(&field_path)
            .map_err(|error| anyhow::anyhow!("field hash failed: {error:?}"))?;
        write_json(
            &loaded.arguments.output.join("receipt.json"),
            &SweepReceipt {
                status: "PASS",
                case: &loaded.arguments.case_name,
                case_index: loaded.arguments.case_index,
                arm: "stock",
                zoom: loaded.arguments.zoom,
                tile_x: loaded.arguments.tile_x,
                tile_y: loaded.arguments.tile_y,
                layer: loaded.arguments.layer.name(),
                region_source_rows: loaded.rows.len(),
                barrier_rows: loaded.barriers.len(),
                obstacle_indexes: loaded.flat.n_indexes,
                evaluated_pairs: field.evaluated_pair_count,
                evaluated_nodes: 0,
                admitted_nodes: 0,
                raw_candidate_visits: 0,
                maximum_raw_candidates_per_pair: 0,
                maximum_distinct_hint_records: 0,
                maximum_unique_u_hints: 0,
                maximum_logical_hint_storage_bytes: 0,
                judge_hint_capacity: H_JUDGE_MAX,
                raw_candidate_replay_capacity: H0_V3_RAW_CANDIDATES_PER_PAIR_MAX,
                hostname: &identity.hostname,
                cpu_model: &identity.cpu_model,
                rayon_num_threads: identity.rayon_num_threads,
                binary_sha256: &identity.binary_sha256,
                field_sha256: &field_sha256,
                surface_budget_eta: "0",
                process_wall_seconds: started.elapsed().as_secs_f64(),
            },
        )
    })
}

/// Run the one arm fixed by the calling binary. The arm is not accepted on the
/// command line, so each accuracy point has a distinct executable and build
/// manifest as required by the frozen V3 contract.
pub fn run(arm: H0V3PairArm) -> Result<()> {
    // Symmetric with `run_stock`: distinct executables, one checked module.
    let _stock_entry_point: fn() -> Result<()> = run_stock;
    let arguments = parse_arguments()?;
    let identity = run_identity()?;
    fs::create_dir(&arguments.output).with_context(|| {
        format!(
            "output must be a fresh directory: {}",
            arguments.output.display()
        )
    })?;
    let started = Instant::now();
    with_loaded_case(&arguments, |loaded| {
        let raw_candidate_visits = AtomicU64::new(0);
        let maximum_raw_candidates_per_pair = AtomicU64::new(0);
        let field = evaluate_h0_v3_tile(
            loaded.tile,
            loaded.rows,
            loaded.barriers,
            Some(loaded.obstacle_set),
            loaded.arguments.layer.line_layer(),
            arm,
            &|line, receiver_latitude, receiver_longitude| {
                let candidates =
                    loaded.collect_candidates(line, receiver_latitude, receiver_longitude)?;
                let count = candidates.len() as u64;
                raw_candidate_visits.fetch_add(count, Ordering::Relaxed);
                maximum_raw_candidates_per_pair.fetch_max(count, Ordering::Relaxed);
                Ok::<Vec<_>, anyhow::Error>(candidates)
            },
        )
        .map_err(map_tile_error)?;
        ensure!(field.period_band_power.len() == 512 * 512);
        ensure!(field.evaluated_pair_count > 0);
        if matches!(arm, H0V3PairArm::JudgeCoarse | H0V3PairArm::JudgeFine) {
            ensure!(field.maximum_distinct_hint_records <= H_JUDGE_MAX);
            ensure!(field.maximum_unique_u_hints <= H_JUDGE_MAX);
            ensure!(field.maximum_logical_hint_storage_bytes <= JUDGE_LOGICAL_HINT_BYTES_MAX);
            ensure!(field.evaluated_node_count > 0);
            ensure!(field.evaluated_node_count == field.admitted_node_count);
        }
        ensure!(
            maximum_raw_candidates_per_pair.load(Ordering::Relaxed)
                <= H0_V3_RAW_CANDIDATES_PER_PAIR_MAX as u64
        );
        let field_path = loaded.arguments.output.join("field.bin");
        write_h0_v3_field(
            &field_path,
            H0V3FieldIdentity {
                case_index: loaded.arguments.case_index as u32,
                arm: arm.into(),
            },
            &field,
        )
        .map_err(|error| anyhow::anyhow!("field write failed: {error:?}"))?;
        let field_sha256 = sha256_file_hex(&field_path)
            .map_err(|error| anyhow::anyhow!("field hash failed: {error:?}"))?;
        write_json(
            &loaded.arguments.output.join("receipt.json"),
            &SweepReceipt {
                status: "PASS",
                case: &loaded.arguments.case_name,
                case_index: loaded.arguments.case_index,
                arm: arm_name(arm),
                zoom: loaded.arguments.zoom,
                tile_x: loaded.arguments.tile_x,
                tile_y: loaded.arguments.tile_y,
                layer: loaded.arguments.layer.name(),
                region_source_rows: loaded.rows.len(),
                barrier_rows: loaded.barriers.len(),
                obstacle_indexes: loaded.flat.n_indexes,
                evaluated_pairs: field.evaluated_pair_count,
                evaluated_nodes: field.evaluated_node_count,
                admitted_nodes: field.admitted_node_count,
                raw_candidate_visits: raw_candidate_visits.load(Ordering::Relaxed),
                maximum_raw_candidates_per_pair: maximum_raw_candidates_per_pair
                    .load(Ordering::Relaxed),
                maximum_distinct_hint_records: field.maximum_distinct_hint_records,
                maximum_unique_u_hints: field.maximum_unique_u_hints,
                maximum_logical_hint_storage_bytes: field.maximum_logical_hint_storage_bytes,
                judge_hint_capacity: H_JUDGE_MAX,
                raw_candidate_replay_capacity: H0_V3_RAW_CANDIDATES_PER_PAIR_MAX,
                hostname: &identity.hostname,
                cpu_model: &identity.cpu_model,
                rayon_num_threads: identity.rayon_num_threads,
                binary_sha256: &identity.binary_sha256,
                field_sha256: &field_sha256,
                surface_budget_eta: "0",
                process_wall_seconds: started.elapsed().as_secs_f64(),
            },
        )
    })
}
