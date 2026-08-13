//! Compare two HM3 tiles cell-by-cell and report Lden drift, in MAGNITUDE and in
//! SIGN. `signed_mean_db` and `cand_louder_pct` exist because the magnitude alone
//! is positive by construction and hides a systematic lean — and a lean is what
//! survives `build-pyramid`, which averages energy, into the overview zoom.
//! Used to gate approximations (coarse-grid, 1 Hz aggregation) against an exact
//! baseline under the aggregate accuracy contract.
//!
//! With `--wave` it also prints one pair's contract diagnostics. Only `--aggregate`
//! emits a PASS/FAIL release verdict; a single row or probe never qualifies a wave.
//! Scoring lives in [`tile_painter::accuracy_contract`]; this binary is the driver.
//!
//! Aggregate scoring accepts any number of pairs, but emits a release verdict only at
//! the fixed 250/980-row Wave-1/Wave-2 benchmark sizes. Amplitude and presence use
//! reference-painted cells; bias uses the numerically comparable painted subset.
//! Per-row rates are diagnostics:
//! `compare_hm3 --aggregate <ref> <cand> [<ref> <cand> ...] --wave 1|2`.

use std::path::Path;
use std::process::ExitCode;

use tile_painter::accuracy_contract::{
    allowance, score, AggregateScore, Score, Scoring, Verdict, Wave, DIAGNOSTIC_EXTREME_OVER_DB,
    MAX_AGGREGATE_SIGNED_MEAN_DB, ROW_ANTI_DILUTION_MULTIPLIER, ROW_EYEBALL_PRESENCE_FRACTION,
    ROW_EYEBALL_SIGNED_MEAN_DB,
};
use tile_painter::wire_hm3;

const USAGE: &str = "usage: compare_hm3 <reference.bin> <candidate.bin> \
[--wave 1|2] [--scoring absolute|marginal]\n       compare_hm3 --aggregate \
<reference.bin> <candidate.bin> [<reference.bin> <candidate.bin> ...] \
--wave 1|2 [--scoring absolute|marginal]";

struct PairPaths {
    reference: String,
    candidate: String,
}

struct Args {
    pairs: Vec<PairPaths>,
    aggregate: bool,
    wave: Option<Wave>,
    scoring: Scoring,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
    let mut aggregate = false;
    let mut wave = None;
    let mut scoring = Scoring::Absolute;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--wave" => {
                let v = argv.get(i + 1).ok_or("--wave needs 1 or 2")?;
                wave = Some(Wave::parse(v).ok_or_else(|| format!("bad --wave {v}"))?);
                i += 2;
            }
            "--scoring" => {
                let v = argv.get(i + 1).ok_or("--scoring needs absolute|marginal")?;
                scoring = Scoring::parse(v).ok_or_else(|| format!("bad --scoring {v}"))?;
                i += 2;
            }
            "--aggregate" => {
                aggregate = true;
                i += 1;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }
    if positional.is_empty()
        || !positional.len().is_multiple_of(2)
        || (!aggregate && positional.len() != 2)
    {
        return Err(USAGE.to_string());
    }
    if aggregate && wave.is_none() {
        return Err(format!("--aggregate requires --wave\n{USAGE}"));
    }
    let pairs = positional
        .chunks_exact(2)
        .map(|pair| PairPaths {
            reference: pair[0].clone(),
            candidate: pair[1].clone(),
        })
        .collect();
    Ok(Args {
        pairs,
        aggregate,
        wave,
        scoring,
    })
}

/// The statistics line this tool has always printed, unchanged, so every existing lab
/// script keeps parsing it.
fn print_statistics(s: &Score) {
    let both = s.compared();
    let over_05 = s.loud.cells_over(0.5) + s.quiet.cells_over(0.5);
    let over_10 = s.loud.cells_over(1.0) + s.quiet.cells_over(1.0);
    println!(
        "both={both} mean_abs_db={mean:.4} max_abs_db={max_diff:.3} \
         cells>0.5dB={over_05} cells>1.0dB={over_10} \
         presence_changed={presence_changed} \
         signed_mean_db={signed_mean:+.4} moved={moved} \
         cand_louder_pct={louder_pct:.1}",
        mean = s.mean_abs_db(),
        max_diff = s.max_abs_db(),
        presence_changed = s.presence_changed,
        signed_mean = s.signed_mean_db(),
        moved = s.moved(),
        louder_pct = s.cand_louder_pct(),
    );
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "OVER"
    }
}

fn hard_mark(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "FAIL"
    }
}

fn format_rows(rows: &[usize]) -> String {
    if rows.is_empty() {
        "none".to_string()
    } else {
        rows.iter()
            .map(|row| (row + 1).to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn format_percentage(count: usize, denominator: usize) -> String {
    if denominator == 0 && count > 0 {
        "inf".to_string()
    } else {
        format!("{:.3}", percentage(count, denominator))
    }
}

fn format_rung_counts(wave: Wave, counts: &[usize]) -> String {
    wave.rungs()
        .iter()
        .zip(counts)
        .map(|(rung, count)| format!("{:.1}:{count}", rung.over_db))
        .collect::<Vec<_>>()
        .join(",")
}

fn print_row_diagnostics(s: &Score, wave: Wave, scoring: Scoring) {
    let counts = s.count_rungs(wave);
    let overshoots = s.amplitude_overshoots(wave);
    println!(
        "contract diagnostic_only wave={} scoring={} cells={}",
        wave.label(),
        scoring.label(),
        s.cells
    );
    println!(
        "  ref>=30dB compared={} max_abs_db={:.3} signed_mean_db={:+.4} \
         cand_louder_pct={:.1}",
        s.loud.cells,
        s.loud.max_abs_db,
        s.loud.signed_mean_db(),
        s.loud.cand_louder_pct()
    );
    for (i, rung) in wave.rungs().iter().enumerate() {
        let allowed = allowance(s.cells, rung.max_fraction);
        let budget = format!("{:.4}% of tile", rung.max_fraction * 100.0);
        println!(
            "    >{:>4.1} dB {:>9} cells {:>7.3}%   allowed {:>9} ({budget})  {} \
             population={} diagnostic_only",
            rung.over_db,
            counts[i],
            percentage(counts[i], s.cells),
            allowed,
            mark(!overshoots.contains(&i)),
            rung.population_label(),
        );
    }
    if wave == Wave::One {
        println!(
            "    >{:>4.1} dB {:>9} cells   allowed=none diagnostic_only",
            DIAGNOSTIC_EXTREME_OVER_DB,
            s.loud.cells_over(DIAGNOSTIC_EXTREME_OVER_DB),
        );
    }
    println!(
        "  ref<30dB  compared={} max_abs_db={:.3}   aggregate_reference {:.1} dB  {} diagnostic_only",
        s.quiet.cells,
        s.quiet.max_abs_db,
        wave.quiet_band_max_db(),
        mark(!s.quiet_band_over(wave)),
    );
    println!(
        "  paint-edge crossings {}   (all, including the rounding band)",
        s.paint_edge_crossings,
    );
    println!(
        "  qualifying paint-state flips {}   silenced {}   painted-over-silence {}   \
         aggregate_reference_allowance {}  diagnostic_only",
        s.qualifying_flips,
        s.flips_newly_silent,
        s.flips_newly_painted,
        s.presence_allowance(wave),
    );
    let bias = s.loud.signed_mean_db().abs();
    println!(
        "  bias |signed_mean_db| {bias:.4}   aggregate_reference {:.2} diagnostic_only",
        MAX_AGGREGATE_SIGNED_MEAN_DB,
    );
    // One greppable line carrying row diagnostics without claiming a release verdict.
    println!(
        "diagnostic_only wave={} scoring={} rung_counts={} extreme_over_12db={} flips={} \
         presence_gate_cells={} presence_reference={} quiet_max_db={:.1} \
         wave2_unified_tail={}",
        wave.label(),
        scoring.label(),
        format_rung_counts(wave, &counts),
        s.loud.cells_over(DIAGNOSTIC_EXTREME_OVER_DB),
        s.qualifying_flips,
        s.gated_presence_changes,
        s.presence_allowance(wave),
        s.quiet.max_abs_db,
        s.wave_two_unified_tail,
    );
}

fn percentage(count: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        100.0 * count as f64 / denominator as f64
    }
}

fn print_aggregate_score(
    scores: &[Score],
    paths: &[PairPaths],
    wave: Wave,
    scoring: Scoring,
) -> Option<Verdict> {
    let aggregate = AggregateScore::new(scores);
    let verdict = aggregate.verdict(wave);
    let release_eligible = verdict.is_some();
    let decision_mark = |ok| {
        if release_eligible {
            hard_mark(ok)
        } else {
            mark(ok)
        }
    };
    let counts = aggregate.count_rungs(wave);
    let aggregate_overshoots = aggregate.amplitude_overshoots(wave);
    let row_overshoots = aggregate.row_amplitude_overshoots(wave);
    let row_presence_overshoots = aggregate.row_presence_diagnostic_overshoots(wave);
    let painted = aggregate.painted_cells();

    println!(
        "contract aggregate{} wave={} scoring={} rows={} painted_cells={}",
        if release_eligible {
            ""
        } else {
            " diagnostic_only"
        },
        wave.label(),
        scoring.label(),
        scores.len(),
        painted,
    );
    println!(
        "  ref>=30dB max_abs_db={:.3} signed_mean_db={:+.4} cand_louder_pct={:.1} \
         presence_changed={} paint_edge_crossings={}",
        aggregate.loud_max_abs_db(),
        aggregate.signed_mean_db(),
        aggregate.cand_louder_pct(),
        aggregate.presence_changed(),
        aggregate.paint_edge_crossings(),
    );
    let rungs = wave.rungs();
    for (rung_index, rung) in rungs.iter().enumerate() {
        let allowed = allowance(painted, rung.max_fraction);
        let rows_over = row_overshoots
            .iter()
            .filter(|(_, over_rung)| *over_rung == rung_index)
            .map(|(row, _)| (row + 1).to_string())
            .collect::<Vec<_>>();
        println!(
            "    >{:>4.1} dB {:>9} cells {:>7.3}%   aggregate_allowed {:>9} \
             aggregate_gate={}   population={} row_reference={}x rows_over={} \
             row_diagnostic_only",
            rung.over_db,
            counts[rung_index],
            percentage(counts[rung_index], painted),
            allowed,
            decision_mark(!aggregate_overshoots.contains(&rung_index)),
            rung.population_label(),
            ROW_ANTI_DILUTION_MULTIPLIER,
            if rows_over.is_empty() {
                "none".to_string()
            } else {
                rows_over.join(",")
            },
        );
    }
    if wave == Wave::One {
        println!(
            "    >{:>4.1} dB {:>9} cells   aggregate_allowed=none diagnostic_only",
            DIAGNOSTIC_EXTREME_OVER_DB,
            aggregate.diagnostic_extreme_count(),
        );
    }

    for (row_index, (row, pair)) in aggregate.rows().iter().zip(paths).enumerate() {
        let row_counts = row.count_rungs(wave);
        let presence_row_over = row_presence_overshoots.contains(&row_index);
        println!(
            "  row={} painted={} compared={} presence_changed={} presence_gate_cells={} silenced={} \
             paint_edge_crossings={} counts={} painted_max_db={:.1} quiet_max_db={:.1} \
             extreme_over_12db={} bias={:+.4} flips={} \
             presence_pct={} presence_3x_reference={} {} diagnostic_only \
             wave2_unified_tail={} reference={} candidate={}",
            row_index + 1,
            row.reference_painted_cells,
            row.compared(),
            row.presence_changed,
            row.gated_presence_changes,
            row.flips_newly_silent,
            row.paint_edge_crossings,
            format_rung_counts(wave, &row_counts),
            row.loud.max_abs_db,
            row.quiet.max_abs_db,
            row.loud.cells_over(DIAGNOSTIC_EXTREME_OVER_DB),
            row.loud.signed_mean_db(),
            row.qualifying_flips,
            format_percentage(row.gated_presence_changes, row.reference_painted_cells),
            aggregate.row_presence_diagnostic_allowance(row_index, wave),
            mark(!presence_row_over),
            row.wave_two_unified_tail,
            pair.reference,
            pair.candidate,
        );
    }

    println!(
        "  ref<30dB max_abs_db={:.3}   limit {:.1} dB  aggregate_gate={}",
        aggregate.quiet_max_abs_db(),
        wave.quiet_band_max_db(),
        decision_mark(!aggregate.quiet_band_over(wave)),
    );
    println!(
        "  aggregate presence gate cells {}   qualifying flips {}   raw NO_DATA mismatches {}   \
         silenced {}   painted-over-silence {}   \
         aggregate_presence_allowed {}  aggregate_gate={}   \
         row_reference={}x rows_over={} row_diagnostic_only",
        aggregate.gated_presence_changes(),
        aggregate.qualifying_flips(),
        aggregate.presence_changed(),
        aggregate.flips_newly_silent(),
        aggregate.flips_newly_painted(),
        aggregate.presence_allowance(wave),
        decision_mark(!aggregate.presence_over_budget(wave)),
        ROW_ANTI_DILUTION_MULTIPLIER,
        if row_presence_overshoots.is_empty() {
            "none".to_string()
        } else {
            row_presence_overshoots
                .iter()
                .map(|row| (row + 1).to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
    );
    let bias_rows = scores
        .iter()
        .enumerate()
        .filter(|(_, row)| row.loud.signed_mean_db().abs() > MAX_AGGREGATE_SIGNED_MEAN_DB)
        .map(|(row, _)| (row + 1).to_string())
        .collect::<Vec<_>>();
    println!(
        "  bias aggregate |signed_mean_db| {:.4}   limit {:.2}  aggregate_gate={}",
        aggregate.signed_mean_db().abs(),
        MAX_AGGREGATE_SIGNED_MEAN_DB,
        decision_mark(!aggregate.bias_over_budget()),
    );
    println!(
        "  bias per-row reference {:.2}   rows_over={} diagnostic_only",
        MAX_AGGREGATE_SIGNED_MEAN_DB,
        if bias_rows.is_empty() {
            "none".to_string()
        } else {
            bias_rows.join(",")
        },
    );
    let presence_eyeball = aggregate.row_presence_eyeball_rows();
    let bias_eyeball = aggregate.row_bias_eyeball_rows();
    let mut eyeball_rows = presence_eyeball
        .iter()
        .chain(&bias_eyeball)
        .copied()
        .collect::<Vec<_>>();
    eyeball_rows.sort_unstable();
    eyeball_rows.dedup();
    println!(
        "  eyeball presence>{:.0}% rows={} bias>{:.1}dB rows={} inspect_rows={} diagnostic_only",
        ROW_EYEBALL_PRESENCE_FRACTION * 100.0,
        format_rows(&presence_eyeball),
        ROW_EYEBALL_SIGNED_MEAN_DB,
        format_rows(&bias_eyeball),
        format_rows(&eyeball_rows),
    );
    let summary = format!(
        "wave={} scoring={} aggregate_rows={} painted_cells={} aggregate_bias={:+.4} \
         paint_edge_crossings={} rung_counts={} extreme_over_12db={} flips={} \
         presence_gate_cells={} \
         aggregate_presence_allowed={} \
         quiet_max_db={:.1} wave2_unified_tail={} \
         diagnostic_presence_rows_over_3x={} eyeball_rows={}",
        wave.label(),
        scoring.label(),
        scores.len(),
        painted,
        aggregate.signed_mean_db(),
        aggregate.paint_edge_crossings(),
        format_rung_counts(wave, &counts),
        aggregate.diagnostic_extreme_count(),
        aggregate.qualifying_flips(),
        aggregate.gated_presence_changes(),
        aggregate.presence_allowance(wave),
        aggregate.quiet_max_abs_db(),
        aggregate.wave_two_unified_tail(),
        format_rows(&row_presence_overshoots),
        format_rows(&eyeball_rows),
    );
    if let Some(verdict) = verdict {
        println!("verdict={} {summary}", verdict.label());
        Some(verdict)
    } else {
        println!(
            "diagnostic_only {summary} expected_benchmark_rows={}",
            wave.benchmark_rows()
        );
        None
    }
}

fn read_score(pair: &PairPaths) -> Result<Score, String> {
    let reference = wire_hm3::read_tile(Path::new(&pair.reference))
        .map_err(|error| format!("reading {}: {error}", pair.reference))?;
    let candidate = wire_hm3::read_tile(Path::new(&pair.candidate))
        .map_err(|error| format!("reading {}: {error}", pair.candidate))?;
    if reference.len() != candidate.len() {
        return Err(format!(
            "tile cell counts differ for {} and {}: {} vs {}",
            pair.reference,
            pair.candidate,
            reference.len(),
            candidate.len(),
        ));
    }
    Ok(score(&reference, &candidate))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut scores = Vec::with_capacity(args.pairs.len());
    for pair in &args.pairs {
        match read_score(pair) {
            Ok(scored) => scores.push(scored),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        }
    }

    if args.aggregate {
        let verdict = print_aggregate_score(
            &scores,
            &args.pairs,
            args.wave.expect("aggregate requires wave"),
            args.scoring,
        );
        return match verdict {
            Some(Verdict::Fail) => ExitCode::FAILURE,
            Some(Verdict::Pass) | None => ExitCode::SUCCESS,
        };
    } else {
        print_statistics(&scores[0]);
        if let Some(wave) = args.wave {
            print_row_diagnostics(&scores[0], wave, args.scoring);
        }
    }
    ExitCode::SUCCESS
}
