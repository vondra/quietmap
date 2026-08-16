//! Fail-closed analyser for the pre-registered CPU H0 V3 field matrix.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use noise_compute::propagation::h0_v3::{
    score_h0_v3, score_h0_v3_periods, select_h0_theta, H0V3DeltaSummary, H0V3Observation,
    H0V3PeriodDeltaSummary, H0V3Theta, H0_V3_CASES, H0_V3_JUDGE_MACHINE_HOUR_CEILING,
    H0_V3_RAW_CANDIDATES_PER_PAIR_MAX, H_JUDGE_MAX, JUDGE_LOGICAL_HINT_BYTES_MAX,
};
use serde::Serialize;

use noise_gpu::h0_v3_field::{
    read_h0_v3_observations, sha256_file_hex, H0V3FieldArm, H0V3FieldIdentity,
};

const ARM_NAMES: [(&str, H0V3FieldArm); 7] = [
    ("stock", H0V3FieldArm::Stock),
    ("h0-5", H0V3FieldArm::H0(H0V3Theta::Degrees5)),
    ("h0-4", H0V3FieldArm::H0(H0V3Theta::Degrees4)),
    ("h0-3", H0V3FieldArm::H0(H0V3Theta::Degrees3)),
    ("h0-2", H0V3FieldArm::H0(H0V3Theta::Degrees2)),
    ("judge-coarse", H0V3FieldArm::JudgeCoarse),
    ("judge-fine", H0V3FieldArm::JudgeFine),
];

const H0_ARM_NAMES: [&str; 4] = ["h0-5", "h0-4", "h0-3", "h0-2"];

#[derive(Serialize)]
struct CaseAnalysis<'a> {
    case: &'a str,
    judge_halving: H0V3DeltaSummary,
    judge_halving_pass: bool,
    h0: BTreeMap<u8, H0V3DeltaSummary>,
    stock_model_delta: BTreeMap<u8, H0V3PeriodDeltaSummary>,
}

#[derive(Serialize)]
struct AnalysisReceipt<'a> {
    status: &'static str,
    verdict: &'static str,
    design_v3_sha256: &'static str,
    plan_v9_sha256: &'static str,
    cases: Vec<CaseAnalysis<'a>>,
    aggregate_judge_halving: H0V3DeltaSummary,
    aggregate_h0: BTreeMap<u8, H0V3DeltaSummary>,
    aggregate_stock_model_delta: BTreeMap<u8, H0V3PeriodDeltaSummary>,
    selected_stock_model_delta: Option<H0V3PeriodDeltaSummary>,
    selected_h0_theta_degrees: Option<u8>,
    stock_model_delta_authority: &'static str,
}

#[derive(serde::Deserialize)]
struct JudgeBudgetReceipt {
    status: String,
    aggregate_conservative_projected_machine_hours: f64,
    aggregate_kappa_projected_machine_hours: f64,
    aggregate_machine_hour_ceiling: f64,
    cases: Vec<BudgetCaseReceipt>,
}

#[derive(serde::Deserialize)]
struct BudgetCaseReceipt {
    case: String,
    hostname: String,
    cpu_model: String,
    rayon_num_threads: usize,
    h0_three_degree_binary_sha256: String,
    h0_three_degree_field_sha256: String,
}

#[derive(serde::Deserialize)]
struct ArmReceipt {
    status: String,
    case: String,
    case_index: usize,
    arm: String,
    region_source_rows: usize,
    barrier_rows: usize,
    obstacle_indexes: usize,
    evaluated_pairs: u64,
    evaluated_nodes: u64,
    admitted_nodes: u64,
    maximum_raw_candidates_per_pair: u64,
    maximum_distinct_hint_records: u64,
    maximum_unique_u_hints: u64,
    maximum_logical_hint_storage_bytes: u64,
    judge_hint_capacity: usize,
    raw_candidate_replay_capacity: usize,
    hostname: String,
    cpu_model: String,
    rayon_num_threads: usize,
    binary_sha256: String,
    field_sha256: String,
    surface_budget_eta: String,
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_and_verify_arm_receipt(
    path: &Path,
    field_path: &Path,
    case: &str,
    case_index: usize,
    arm: &str,
) -> Result<ArmReceipt> {
    let receipt: ArmReceipt = serde_json::from_reader(
        File::open(path).with_context(|| format!("missing {}", path.display()))?,
    )?;
    ensure!(receipt.status == "PASS");
    ensure!(receipt.case == case && receipt.case_index == case_index);
    ensure!(receipt.arm == arm);
    ensure!(receipt.evaluated_pairs > 0);
    ensure!(receipt.raw_candidate_replay_capacity == H0_V3_RAW_CANDIDATES_PER_PAIR_MAX);
    ensure!(receipt.maximum_raw_candidates_per_pair <= H0_V3_RAW_CANDIDATES_PER_PAIR_MAX as u64);
    ensure!(receipt.surface_budget_eta == "0");
    ensure!(receipt.rayon_num_threads > 0);
    ensure!(is_sha256(&receipt.binary_sha256) && is_sha256(&receipt.field_sha256));
    let actual_field_sha = sha256_file_hex(field_path)
        .map_err(|error| anyhow::anyhow!("cannot hash {}: {error:?}", field_path.display()))?;
    ensure!(actual_field_sha == receipt.field_sha256);
    if arm.starts_with("judge-") {
        ensure!(receipt.judge_hint_capacity == H_JUDGE_MAX);
        ensure!(receipt.maximum_distinct_hint_records <= H_JUDGE_MAX as u64);
        ensure!(receipt.maximum_unique_u_hints <= H_JUDGE_MAX as u64);
        ensure!(receipt.maximum_logical_hint_storage_bytes <= JUDGE_LOGICAL_HINT_BYTES_MAX as u64);
        ensure!(receipt.evaluated_nodes > 0);
        ensure!(receipt.evaluated_nodes == receipt.admitted_nodes);
    }
    Ok(receipt)
}

fn load_case(
    root: &Path,
    case_index: usize,
    budget_case: &BudgetCaseReceipt,
) -> Result<BTreeMap<&'static str, Vec<H0V3Observation>>> {
    let case = H0_V3_CASES[case_index];
    ensure!(budget_case.case == case.name);
    let mut fields = BTreeMap::new();
    let mut geometry_identity = None;
    for (arm_name, arm) in ARM_NAMES {
        let arm_root = root.join(case.name).join(arm_name);
        let field_path = arm_root.join("field.bin");
        let receipt = read_and_verify_arm_receipt(
            &arm_root.join("receipt.json"),
            &field_path,
            case.name,
            case_index,
            arm_name,
        )?;
        ensure!(receipt.hostname == budget_case.hostname);
        ensure!(receipt.cpu_model == budget_case.cpu_model);
        ensure!(receipt.rayon_num_threads == budget_case.rayon_num_threads);
        let arm_geometry = (
            receipt.region_source_rows,
            receipt.barrier_rows,
            receipt.obstacle_indexes,
            receipt.evaluated_pairs,
        );
        if let Some(expected) = geometry_identity {
            ensure!(
                arm_geometry == expected,
                "mixed input geometry in {}",
                case.name
            );
        } else {
            geometry_identity = Some(arm_geometry);
        }
        if arm_name == "h0-3" {
            ensure!(receipt.binary_sha256 == budget_case.h0_three_degree_binary_sha256);
            ensure!(receipt.field_sha256 == budget_case.h0_three_degree_field_sha256);
        }
        let observations = read_h0_v3_observations(
            &field_path,
            H0V3FieldIdentity {
                case_index: case_index as u32,
                arm,
            },
        )
        .map_err(|error| anyhow::anyhow!("invalid {}/{} field: {error:?}", case.name, arm_name))?;
        fields.insert(arm_name, observations);
    }
    Ok(fields)
}

fn main() -> Result<()> {
    let values: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        values.len() == 2,
        "usage: h0-v3-analyze FIELD_ROOT OUTPUT_DIR"
    );
    let field_root = PathBuf::from(&values[0]);
    let output = PathBuf::from(&values[1]);
    ensure!(field_root.is_dir(), "missing FIELD_ROOT");
    fs::create_dir(&output)
        .with_context(|| format!("output must be a fresh directory: {}", output.display()))?;
    let budget: JudgeBudgetReceipt = serde_json::from_reader(
        File::open(field_root.join("judge-budget/judge-budget.json"))
            .context("missing judge-budget authority")?,
    )?;
    ensure!(budget.status == "PASS");
    ensure!(
        budget.aggregate_machine_hour_ceiling.to_bits()
            == H0_V3_JUDGE_MACHINE_HOUR_CEILING.to_bits()
    );
    ensure!(
        budget
            .aggregate_conservative_projected_machine_hours
            .is_finite()
            && budget.aggregate_conservative_projected_machine_hours
                <= budget.aggregate_machine_hour_ceiling
    );
    ensure!(budget.aggregate_kappa_projected_machine_hours.is_finite());
    ensure!(budget.aggregate_kappa_projected_machine_hours >= 0.0);
    ensure!(budget.cases.len() == H0_V3_CASES.len());

    let mut case_analyses = Vec::new();
    let mut aggregate: BTreeMap<&'static str, Vec<H0V3Observation>> = ARM_NAMES
        .into_iter()
        .map(|(name, _)| (name, Vec::new()))
        .collect();
    let mut every_case_judge_passes = true;
    let mut every_case_theta_passes = [true; 4];
    for (case_index, case) in H0_V3_CASES.iter().enumerate() {
        let budget_case = &budget.cases[case_index];
        let fields = load_case(&field_root, case_index, budget_case)?;
        let judge_halving = score_h0_v3(&fields["judge-fine"], &fields["judge-coarse"])
            .map_err(|error| anyhow::anyhow!("judge score failed: {error:?}"))?;
        let judge_halving_pass = judge_halving.judge_halving_passes();
        every_case_judge_passes &= judge_halving_pass;
        let mut h0 = BTreeMap::new();
        let mut stock_model_delta = BTreeMap::new();
        for (index, theta) in H0V3Theta::COARSE_TO_FINE.into_iter().enumerate() {
            let arm_name = H0_ARM_NAMES[index];
            let summary = score_h0_v3(&fields["judge-fine"], &fields[arm_name])
                .map_err(|error| anyhow::anyhow!("H0 score failed: {error:?}"))?;
            every_case_theta_passes[index] &= summary.h0_v3_passes();
            h0.insert(theta.degrees(), summary);
            stock_model_delta.insert(
                theta.degrees(),
                score_h0_v3_periods(&fields["stock"], &fields[arm_name])
                    .map_err(|error| anyhow::anyhow!("stock-model score failed: {error:?}"))?,
            );
        }
        for (name, observations) in fields {
            aggregate
                .get_mut(name)
                .expect("all pre-registered arms were initialised")
                .extend(observations);
        }
        case_analyses.push(CaseAnalysis {
            case: case.name,
            judge_halving,
            judge_halving_pass,
            h0,
            stock_model_delta,
        });
    }
    let aggregate_judge_halving = score_h0_v3(&aggregate["judge-fine"], &aggregate["judge-coarse"])
        .map_err(|error| anyhow::anyhow!("aggregate judge score failed: {error:?}"))?;
    ensure!(
        aggregate_judge_halving.judge_halving_passes() == every_case_judge_passes,
        "aggregate and per-case judge verdicts disagree"
    );
    let mut aggregate_h0 = BTreeMap::new();
    let mut aggregate_stock_model_delta = BTreeMap::new();
    let mut selection_inputs = Vec::new();
    for (index, theta) in H0V3Theta::COARSE_TO_FINE.into_iter().enumerate() {
        let arm_name = H0_ARM_NAMES[index];
        let summary = score_h0_v3(&aggregate["judge-fine"], &aggregate[arm_name])
            .map_err(|error| anyhow::anyhow!("aggregate H0 score failed: {error:?}"))?;
        ensure!(
            summary.h0_v3_passes() == every_case_theta_passes[index],
            "aggregate and per-case H0 verdicts disagree"
        );
        selection_inputs.push((theta, summary.clone()));
        aggregate_h0.insert(theta.degrees(), summary);
        aggregate_stock_model_delta.insert(
            theta.degrees(),
            score_h0_v3_periods(&aggregate["stock"], &aggregate[arm_name]).map_err(|error| {
                anyhow::anyhow!("aggregate stock-model score failed: {error:?}")
            })?,
        );
    }
    let selected = if every_case_judge_passes {
        select_h0_theta(&selection_inputs)
            .map_err(|error| anyhow::anyhow!("H0 selection failed: {error:?}"))?
    } else {
        None
    };
    let verdict = if !every_case_judge_passes {
        "JUDGE_CONVERGENCE_FAILED"
    } else if selected.is_some() {
        "H0_QUADRATURE_ACCEPTED"
    } else {
        "NEEDS_H32"
    };
    let selected_stock_model_delta =
        selected.map(|theta| aggregate_stock_model_delta[&theta.degrees()].clone());
    let receipt = AnalysisReceipt {
        status: "PASS",
        verdict,
        design_v3_sha256: "382ee570373553c7007ef4a477ffc6951d407daccff0d8725f0582d0e857605b",
        plan_v9_sha256: "65ce0074a3b9bfc8a62293a093a3ebfc1b5216d07c7f14c3fae4e68ecdc09dba",
        cases: case_analyses,
        aggregate_judge_halving,
        aggregate_h0,
        aggregate_stock_model_delta,
        selected_stock_model_delta,
        selected_h0_theta_degrees: selected.map(H0V3Theta::degrees),
        stock_model_delta_authority:
            "CURRENT_EXACT_CPU_SKYLINE_VS_EACH_H0_ARM; REPORTED_NOT_GATED; DIRECT_P2B_FIXTURE_OWNS_CAUSE",
    };
    let mut receipt_output = BufWriter::new(File::create(output.join("analysis.json"))?);
    serde_json::to_writer_pretty(&mut receipt_output, &receipt)?;
    receipt_output.write_all(b"\n")?;
    receipt_output.flush()?;
    let mut verdict_output = BufWriter::new(File::create(output.join("verdict.txt"))?);
    writeln!(verdict_output, "H0_V3_ANALYSIS=PASS")?;
    writeln!(
        verdict_output,
        "JUDGE_HALVING_PASS={every_case_judge_passes}"
    )?;
    writeln!(verdict_output, "VERDICT={verdict}")?;
    writeln!(
        verdict_output,
        "SELECTED_H0_THETA_DEGREES={}",
        selected.map_or_else(|| "-".to_owned(), |theta| theta.degrees().to_string())
    )?;
    verdict_output.flush()?;
    Ok(())
}
