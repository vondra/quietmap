//! Geometry-only preflight for the bounded-large CPU judge.

#[path = "h0_v3_case.rs"]
mod case;

use std::fs;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use noise_compute::propagation::h0_v3::{
    H0_V3_RAW_CANDIDATES_PER_PAIR_MAX, H_JUDGE_MAX, JUDGE_LOGICAL_HINT_BYTES_MAX,
};
use serde::Serialize;
use tile_painter::h0_v3_sampler::h0_v3_sampled_receivers;
use tile_painter::h0_v3_tile_reference::census_h0_v3_judge_tile;

use case::{map_tile_error, parse_arguments, run_identity, with_loaded_case, write_json};

#[derive(Serialize)]
struct CensusReceipt<'a> {
    status: &'static str,
    case: &'a str,
    case_index: usize,
    zoom: u8,
    tile_x: u32,
    tile_y: u32,
    layer: &'static str,
    region_source_rows: usize,
    barrier_rows: usize,
    obstacle_indexes: usize,
    evaluated_pairs: u64,
    raw_candidate_visits: u64,
    maximum_raw_candidates_per_pair: u64,
    h0_three_degree_nodes: u64,
    h0_three_degree_admitted_nodes: u64,
    coarse_judge_nodes: u64,
    fine_judge_nodes: u64,
    realised_judge_node_ratio: f64,
    maximum_distinct_hint_records: usize,
    maximum_unique_u_hints: usize,
    maximum_logical_hint_storage_bytes: usize,
    judge_hint_capacity: usize,
    raw_candidate_replay_capacity: usize,
    hostname: &'a str,
    cpu_model: &'a str,
    rayon_num_threads: usize,
    binary_sha256: &'a str,
    surface_budget_eta: &'static str,
    process_wall_seconds: f64,
}

fn main() -> Result<()> {
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
        ensure!(!loaded.obstacle_set.indexes.is_empty());
        let receivers = h0_v3_sampled_receivers(loaded.arguments.case_index as u32);
        let census = census_h0_v3_judge_tile(
            loaded.tile,
            loaded.rows,
            loaded.arguments.layer.line_layer(),
            &receivers,
            &|line, receiver_latitude, receiver_longitude| {
                loaded.collect_candidates(line, receiver_latitude, receiver_longitude)
            },
        )
        .map_err(map_tile_error)?;
        ensure!(census.evaluated_pair_count > 0);
        ensure!(census.h0_three_degree_node_count > 0);
        ensure!(census.maximum_distinct_hint_records <= H_JUDGE_MAX);
        ensure!(census.maximum_unique_u_hints <= H_JUDGE_MAX);
        ensure!(census.maximum_logical_hint_storage_bytes <= JUDGE_LOGICAL_HINT_BYTES_MAX);
        ensure!(census.maximum_raw_candidates_per_pair <= H0_V3_RAW_CANDIDATES_PER_PAIR_MAX as u64);
        write_json(
            &loaded.arguments.output.join("census.json"),
            &CensusReceipt {
                status: "PASS",
                case: &loaded.arguments.case_name,
                case_index: loaded.arguments.case_index,
                zoom: loaded.arguments.zoom,
                tile_x: loaded.arguments.tile_x,
                tile_y: loaded.arguments.tile_y,
                layer: loaded.arguments.layer.name(),
                region_source_rows: loaded.rows.len(),
                barrier_rows: loaded.barriers.len(),
                obstacle_indexes: loaded.flat.n_indexes,
                evaluated_pairs: census.evaluated_pair_count,
                raw_candidate_visits: census.raw_candidate_visit_count,
                maximum_raw_candidates_per_pair: census.maximum_raw_candidates_per_pair,
                h0_three_degree_nodes: census.h0_three_degree_node_count,
                h0_three_degree_admitted_nodes: census.h0_three_degree_admitted_node_count,
                coarse_judge_nodes: census.coarse_judge_node_count,
                fine_judge_nodes: census.fine_judge_node_count,
                realised_judge_node_ratio: census.realised_judge_node_ratio(),
                maximum_distinct_hint_records: census.maximum_distinct_hint_records,
                maximum_unique_u_hints: census.maximum_unique_u_hints,
                maximum_logical_hint_storage_bytes: census.maximum_logical_hint_storage_bytes,
                judge_hint_capacity: H_JUDGE_MAX,
                raw_candidate_replay_capacity: H0_V3_RAW_CANDIDATES_PER_PAIR_MAX,
                hostname: &identity.hostname,
                cpu_model: &identity.cpu_model,
                rayon_num_threads: identity.rayon_num_threads,
                binary_sha256: &identity.binary_sha256,
                surface_budget_eta: "0",
                process_wall_seconds: started.elapsed().as_secs_f64(),
            },
        )
    })
}
