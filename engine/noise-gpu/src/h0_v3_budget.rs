//! Fail-closed judge wall projection from geometry census and measured H0-3 work.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use noise_compute::propagation::h0_v3::{
    H0_V3_CASES, H0_V3_JUDGE_MACHINE_HOUR_CEILING, H0_V3_RAW_CANDIDATES_PER_PAIR_MAX,
};
use noise_gpu::h0_v3_field::sha256_file_hex;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct CensusReceipt {
    status: String,
    case: String,
    h0_three_degree_nodes: u64,
    h0_three_degree_admitted_nodes: u64,
    coarse_judge_nodes: u64,
    fine_judge_nodes: u64,
    evaluated_pairs: u64,
    region_source_rows: usize,
    barrier_rows: usize,
    obstacle_indexes: usize,
    raw_candidate_replay_capacity: usize,
    hostname: String,
    cpu_model: String,
    rayon_num_threads: usize,
    binary_sha256: String,
    surface_budget_eta: String,
}

#[derive(Deserialize)]
struct H0Receipt {
    status: String,
    case: String,
    arm: String,
    evaluated_nodes: u64,
    admitted_nodes: u64,
    evaluated_pairs: u64,
    region_source_rows: usize,
    barrier_rows: usize,
    obstacle_indexes: usize,
    raw_candidate_replay_capacity: usize,
    hostname: String,
    cpu_model: String,
    rayon_num_threads: usize,
    binary_sha256: String,
    field_sha256: String,
    surface_budget_eta: String,
    process_wall_seconds: f64,
}

#[derive(Serialize)]
struct CaseProjection {
    case: String,
    measured_h0_three_degree_wall_seconds: f64,
    h0_three_degree_nodes: u64,
    h0_three_degree_admitted_nodes: u64,
    coarse_judge_nodes: u64,
    fine_judge_nodes: u64,
    conservative_projected_machine_hours: f64,
    kappa_projected_machine_hours: f64,
    hostname: String,
    cpu_model: String,
    rayon_num_threads: usize,
    preflight_binary_sha256: String,
    h0_three_degree_binary_sha256: String,
    h0_three_degree_field_sha256: String,
}

#[derive(Serialize)]
struct BudgetReceipt {
    status: &'static str,
    projection_method: &'static str,
    kappa_projection_method: &'static str,
    aggregate_conservative_projected_machine_hours: f64,
    aggregate_kappa_projected_machine_hours: f64,
    aggregate_machine_hour_ceiling: f64,
    cases: Vec<CaseProjection>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(
        File::open(path).with_context(|| format!("missing {}", path.display()))?,
    )
    .with_context(|| format!("invalid {}", path.display()))
}

fn projected_machine_hours(
    h0_wall_seconds: f64,
    h0_admitted_nodes: u64,
    coarse_judge_nodes: u64,
    fine_judge_nodes: u64,
) -> Result<f64> {
    ensure!(h0_wall_seconds.is_finite() && h0_wall_seconds > 0.0);
    ensure!(h0_admitted_nodes > 0);
    let judge_nodes = coarse_judge_nodes
        .checked_add(fine_judge_nodes)
        .context("judge node total overflow")?;
    let projected = h0_wall_seconds * judge_nodes as f64 / h0_admitted_nodes as f64 / 3600.0;
    ensure!(projected.is_finite());
    Ok(projected)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn main() -> Result<()> {
    let values: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        values.len() == 2,
        "usage: h0-v3-budget FIELD_ROOT OUTPUT_DIR"
    );
    let field_root = PathBuf::from(&values[0]);
    let output = PathBuf::from(&values[1]);
    ensure!(field_root.is_dir(), "missing FIELD_ROOT");
    fs::create_dir(&output)
        .with_context(|| format!("output must be a fresh directory: {}", output.display()))?;

    let mut cases = Vec::new();
    let mut aggregate_conservative_projected_machine_hours = 0.0;
    let mut aggregate_kappa_projected_machine_hours = 0.0;
    for case in H0_V3_CASES {
        let case_root = field_root.join(case.name);
        let census: CensusReceipt = read_json(&case_root.join("preflight/census.json"))?;
        let h0: H0Receipt = read_json(&case_root.join("h0-3/receipt.json"))?;
        ensure!(census.status == "PASS" && census.case == case.name);
        ensure!(h0.status == "PASS" && h0.case == case.name && h0.arm == "h0-3");
        ensure!(census.surface_budget_eta == "0" && h0.surface_budget_eta == "0");
        ensure!(
            census.raw_candidate_replay_capacity == H0_V3_RAW_CANDIDATES_PER_PAIR_MAX
                && h0.raw_candidate_replay_capacity == H0_V3_RAW_CANDIDATES_PER_PAIR_MAX
        );
        ensure!(census.evaluated_pairs == h0.evaluated_pairs && h0.evaluated_pairs > 0);
        ensure!(census.region_source_rows == h0.region_source_rows);
        ensure!(census.barrier_rows == h0.barrier_rows);
        ensure!(census.obstacle_indexes == h0.obstacle_indexes);
        ensure!(census.hostname == h0.hostname);
        ensure!(census.cpu_model == h0.cpu_model);
        ensure!(census.rayon_num_threads == h0.rayon_num_threads);
        ensure!(h0.rayon_num_threads > 0);
        ensure!(is_sha256(&census.binary_sha256) && is_sha256(&h0.binary_sha256));
        ensure!(is_sha256(&h0.field_sha256));
        let measured_field_sha = sha256_file_hex(&case_root.join("h0-3/field.bin"))
            .map_err(|error| anyhow::anyhow!("cannot hash H0-3 field: {error:?}"))?;
        ensure!(measured_field_sha == h0.field_sha256);
        ensure!(census.h0_three_degree_nodes == h0.evaluated_nodes);
        ensure!(census.h0_three_degree_admitted_nodes == h0.admitted_nodes);
        let conservative_projected = projected_machine_hours(
            h0.process_wall_seconds,
            h0.admitted_nodes,
            census.coarse_judge_nodes,
            census.fine_judge_nodes,
        )?;
        let kappa_projected = projected_machine_hours(
            h0.process_wall_seconds,
            h0.evaluated_nodes,
            census.coarse_judge_nodes,
            census.fine_judge_nodes,
        )?;
        aggregate_conservative_projected_machine_hours += conservative_projected;
        aggregate_kappa_projected_machine_hours += kappa_projected;
        cases.push(CaseProjection {
            case: case.name.to_owned(),
            measured_h0_three_degree_wall_seconds: h0.process_wall_seconds,
            h0_three_degree_nodes: h0.evaluated_nodes,
            h0_three_degree_admitted_nodes: h0.admitted_nodes,
            coarse_judge_nodes: census.coarse_judge_nodes,
            fine_judge_nodes: census.fine_judge_nodes,
            conservative_projected_machine_hours: conservative_projected,
            kappa_projected_machine_hours: kappa_projected,
            hostname: h0.hostname,
            cpu_model: h0.cpu_model,
            rayon_num_threads: h0.rayon_num_threads,
            preflight_binary_sha256: census.binary_sha256,
            h0_three_degree_binary_sha256: h0.binary_sha256,
            h0_three_degree_field_sha256: h0.field_sha256,
        });
    }
    let passes = aggregate_conservative_projected_machine_hours <= H0_V3_JUDGE_MACHINE_HOUR_CEILING;
    let receipt = BudgetReceipt {
        status: if passes { "PASS" } else { "REJECTED" },
        projection_method:
            "SUM(H0_3_WALL_SECONDS*(COARSE_NODES+FINE_NODES)/H0_3_ADMITTED_NODES)/3600",
        kappa_projection_method:
            "SUM(H0_3_WALL_SECONDS*(COARSE_NODES+FINE_NODES)/H0_3_EVALUATED_NODES)/3600",
        aggregate_conservative_projected_machine_hours,
        aggregate_kappa_projected_machine_hours,
        aggregate_machine_hour_ceiling: H0_V3_JUDGE_MACHINE_HOUR_CEILING,
        cases,
    };
    let mut json = BufWriter::new(File::create(output.join("judge-budget.json"))?);
    serde_json::to_writer_pretty(&mut json, &receipt)?;
    json.write_all(b"\n")?;
    json.flush()?;
    let mut verdict = BufWriter::new(File::create(output.join("verdict.txt"))?);
    writeln!(verdict, "H0_V3_JUDGE_BUDGET={}", receipt.status)?;
    writeln!(
        verdict,
        "CONSERVATIVE_PROJECTED_MACHINE_HOURS={aggregate_conservative_projected_machine_hours:.9}"
    )?;
    writeln!(
        verdict,
        "KAPPA_PROJECTED_MACHINE_HOURS={aggregate_kappa_projected_machine_hours:.9}"
    )?;
    writeln!(
        verdict,
        "MACHINE_HOUR_CEILING={H0_V3_JUDGE_MACHINE_HOUR_CEILING:.9}"
    )?;
    verdict.flush()?;
    ensure!(
        passes,
        "projected judge cost exceeds the pre-registered ceiling"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_prices_every_judge_node_as_a_measured_full_walk() {
        assert_eq!(projected_machine_hours(3600.0, 10, 10, 20).unwrap(), 3.0);
        assert!(projected_machine_hours(1.0, 0, 1, 1).is_err());
        assert!(projected_machine_hours(f64::NAN, 1, 1, 1).is_err());
        assert!(projected_machine_hours(1.0, 1, u64::MAX, 1).is_err());
    }
}
