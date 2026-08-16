//! CPU-only C/N/Q census over a frozen model-v2 H0 pair corpus.

use std::collections::BTreeMap;
use std::io::{self, BufRead};

use noise_compute::compute::element::{LineLayer, LinePiece, H0_NODE_CAP};
use noise_compute::propagation::h0_streaming_reduction::{reduce_h0, H0Candidate, H0Reduction};
use noise_compute::propagation::streaming_reduction::{
    candidate_wedge_owns, range_ordered, MetricVector, SourceId64, WedgeDecision,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateRecord {
    source_id_bits: u64,
    endpoint0_m: [f64; 2],
    endpoint1_m: [f64; 2],
    height_m: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRecord {
    pair_id: String,
    layer: String,
    start_m: [f64; 2],
    end_m: [f64; 2],
    emission_per_m: f64,
    candidates: Vec<CandidateRecord>,
}

fn line_layer(value: &str) -> Result<LineLayer, String> {
    match value {
        "road" => Ok(LineLayer::Road),
        "rail" => Ok(LineLayer::Rail),
        _ => Err(format!("unknown layer {value:?}; expected road or rail")),
    }
}

fn metric(value: [f64; 2]) -> MetricVector {
    MetricVector::new(value[0], value[1])
}

fn explain_zero_mask(reduction: &H0Reduction, candidates: &[H0Candidate]) -> &'static str {
    if candidates.is_empty() {
        return "EMPTY_CANDIDATE_STREAM";
    }
    let mut covered = 0_u64;
    let mut range_eligible = 0_u64;
    let mut guarded = 0_u64;
    for candidate in candidates {
        for node in reduction.nodes() {
            match candidate_wedge_owns(
                candidate.endpoint0_m(),
                candidate.endpoint1_m(),
                node.receiver_vector_m,
                candidate.near_f32(),
            ) {
                WedgeDecision::DoesNotOwn => {}
                WedgeDecision::Owns => {
                    covered += 1;
                    if range_ordered(node.node_distance_m, candidate.near_f32())
                        .expect("the landed reducer already accepted this finite geometry")
                    {
                        range_eligible += 1;
                    }
                }
                WedgeDecision::NearGuardedDegenerate => guarded += 1,
                WedgeDecision::HardFault => {
                    unreachable!("the landed reducer would have failed this candidate")
                }
            }
        }
    }
    if range_eligible != 0 {
        "INVARIANT_FAILURE_ELIGIBLE_NODE_WAS_NOT_ADMITTED"
    } else if covered != 0 {
        "RANGE_REJECTED"
    } else if guarded != 0 {
        "ONLY_NEAR_GUARDED_DEGENERATES"
    } else {
        "NO_WEDGE_OWNED_A_NODE"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut distribution: BTreeMap<(u64, usize, usize), u64> = BTreeMap::new();
    let mut pair_count = 0_u64;
    let mut zero_mask_count = 0_u64;
    let mut total_raw_multiplicity = 0_u64;
    println!("pair_id\traw_multiplicity\tC_p\tN_p\tQ_p\tE_p\tA_p\tguarded\tzero_reason");
    for (line_index, line) in io::stdin().lock().lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: PairRecord = serde_json::from_str(&line)
            .map_err(|error| format!("JSONL line {}: {error}", line_index + 1))?;
        let layer = line_layer(&record.layer).map_err(io::Error::other)?;
        let candidates: Vec<_> = record
            .candidates
            .iter()
            .map(|candidate| {
                H0Candidate::from_metric_segment(
                    SourceId64::from_bits(candidate.source_id_bits),
                    metric(candidate.endpoint0_m),
                    metric(candidate.endpoint1_m),
                    candidate.height_m,
                )
            })
            .collect::<Result<_, _>>()
            .map_err(|error| io::Error::other(format!("candidate geometry: {error:?}")))?;
        let raw_multiplicity = candidates.len() as u64;
        let reduction = reduce_h0(
            LinePiece {
                start_m: record.start_m,
                end_m: record.end_m,
                emission_per_m: record.emission_per_m,
            },
            layer,
            candidates.iter().copied(),
        )
        .map_err(|error| io::Error::other(format!("H0 reduction: {error:?}")))?;
        if reduction.nodes().len() > H0_NODE_CAP {
            return Err(format!("{} exceeded H0 node theorem", record.pair_id).into());
        }
        if reduction.candidate_visit_count() != raw_multiplicity {
            return Err(format!("{} candidate visit count drift", record.pair_id).into());
        }
        let zero_reason = if reduction.any_node_is_admitted() {
            "-"
        } else {
            zero_mask_count += 1;
            explain_zero_mask(&reduction, &candidates)
        };
        if zero_reason.starts_with("INVARIANT_FAILURE") {
            return Err(format!("{}: {zero_reason}", record.pair_id).into());
        }
        let c = reduction.candidate_visit_count();
        let n = reduction.nodes().len();
        let q = reduction.admitted_node_count();
        *distribution.entry((c, n, q)).or_default() += 1;
        total_raw_multiplicity += raw_multiplicity;
        pair_count += 1;
        println!(
            "{}\t{}\t{}\t{}\t{}\t0\t0\t{}\t{}",
            record.pair_id,
            raw_multiplicity,
            c,
            n,
            q,
            reduction.guarded_degenerate_candidate_count(),
            zero_reason,
        );
    }
    if pair_count == 0 {
        return Err("empty H0 pair corpus".into());
    }
    eprintln!("H0_CENSUS_PAIRS={pair_count}");
    eprintln!("H0_CENSUS_ZERO_MASKS={zero_mask_count}");
    eprintln!("H0_CENSUS_RAW_MULTIPLICITY={total_raw_multiplicity}");
    eprintln!("H0_CENSUS_E_P=0");
    eprintln!("H0_CENSUS_A_P=0");
    for ((c, n, q), count) in distribution {
        eprintln!("H0_CENSUS_DISTRIBUTION C={c} N={n} Q={q} PAIRS={count}");
    }
    eprintln!("H0_CENSUS=PASS");
    Ok(())
}
