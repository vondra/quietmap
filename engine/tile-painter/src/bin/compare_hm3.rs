//! Compare two HM3 tiles cell-by-cell and report Lden drift, in MAGNITUDE and in
//! SIGN. `signed_mean_db` and `cand_louder_pct` exist because the magnitude alone
//! is positive by construction and hides a systematic lean — and a lean is what
//! survives `build-pyramid`, which averages energy, into the overview zoom.
//! Used to gate approximations (coarse-grid, 1 Hz aggregation) against an
//! exact baseline under the engine's ±0.5 dB tile tolerance.
//!
//! With `--wave` it also scores the pair against the accuracy contract
//! (`docs/dev/accuracy-contract.md`) and prints a PASS/FAIL verdict, so a measurement
//! ends in a verdict rather than a table someone has to interpret. Scoring lives in
//! [`tile_painter::accuracy_contract`]; this binary is the driver.
//!
//! Aggregate release verdicts accept any number of reference/candidate pairs and
//! weight their ladder by reference-painted cells, with a hard 3x per-row
//! anti-dilution limit:
//! `compare_hm3 --aggregate <ref> <cand> [<ref> <cand> ...] --wave 1|2`.

use std::path::Path;
use std::process::ExitCode;

use tile_painter::accuracy_contract::{
    allowance, score, AggregateScore, Score, Scoring, Verdict, Wave, LONG_FLIP_RUN_THRESHOLD,
    MAX_FLIP_RUN_LENGTH, MAX_LONG_FLIP_RUNS, ROW_ANTI_DILUTION_MULTIPLIER,
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

fn format_flip_histogram(histogram: &[(usize, usize)]) -> String {
    if histogram.is_empty() {
        return "none".to_string();
    }
    histogram
        .iter()
        .map(|(size, count)| format!("{size}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn print_verdict(s: &Score, wave: Wave, scoring: Scoring) -> Verdict {
    let verdict = s.verdict(wave);
    let counts = s.count_rungs(wave);
    let overshoots = s.amplitude_overshoots(wave);
    println!(
        "contract wave={} scoring={} cells={}",
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
        let budget = if rung.max_fraction == 0.0 {
            "hard ceiling".to_string()
        } else {
            format!("{:.4}% of tile", rung.max_fraction * 100.0)
        };
        println!(
            "    >{:>4.1} dB {:>9} cells {:>7.3}%   allowed {:>9} ({budget})  {}",
            rung.over_db,
            counts[i],
            percentage(counts[i], s.cells),
            allowed,
            mark(!overshoots.contains(&i)),
        );
    }
    println!(
        "  ref<30dB  compared={} max_abs_db={:.3}   limit {:.1} dB  {}",
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
        "  flips>=1dB clear of 30dB {}   silenced {}   painted-over-silence {}   \
         catastrophic_backstop_allowed {}  {}",
        s.qualifying_flips,
        s.flips_newly_silent,
        s.flips_newly_painted,
        s.flip_count_backstop_allowance(),
        hard_mark(!s.flip_count_backstop_over_budget()),
    );
    println!(
        "  flip runs components={} histogram={}   longest {} allowed {}  {}",
        s.flip_components(),
        format_flip_histogram(&s.flip_run_histogram()),
        s.longest_flip_run(),
        MAX_FLIP_RUN_LENGTH,
        hard_mark(s.longest_flip_run() <= MAX_FLIP_RUN_LENGTH),
    );
    println!(
        "  flip runs>{} {}   allowed {}  {}",
        LONG_FLIP_RUN_THRESHOLD,
        s.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD),
        MAX_LONG_FLIP_RUNS,
        hard_mark(s.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD) <= MAX_LONG_FLIP_RUNS),
    );
    let bias = s.loud.signed_mean_db().abs();
    println!(
        "  bias |signed_mean_db| {bias:.4}   limit {:.2}  {}",
        wave.max_signed_mean_db(),
        if !s.bias_over_budget(wave) {
            "ok"
        } else {
            "FAIL"
        },
    );
    // One greppable line carrying amplitude debt, flip geometry, and the deliberately
    // loose catastrophic-area backstop.
    let rungs = wave.rungs();
    println!(
        "verdict={} wave={} scoring={} cells>{:.0}dB={} cells>{:.0}dB={} flips={} \
         flip_backstop_allowed={} longest_flip_run={} flip_runs_gt{}={}",
        verdict.label(),
        wave.label(),
        scoring.label(),
        rungs[2].over_db,
        counts[2],
        rungs[3].over_db,
        counts[3],
        s.qualifying_flips,
        s.flip_count_backstop_allowance(),
        s.longest_flip_run(),
        LONG_FLIP_RUN_THRESHOLD,
        s.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD),
    );
    verdict
}

fn percentage(count: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        100.0 * count as f64 / denominator as f64
    }
}

fn print_aggregate_verdict(
    scores: &[Score],
    paths: &[PairPaths],
    wave: Wave,
    scoring: Scoring,
) -> Verdict {
    let aggregate = AggregateScore::new(scores);
    let verdict = aggregate.verdict(wave);
    let counts = aggregate.count_rungs(wave);
    let aggregate_overshoots = aggregate.amplitude_overshoots(wave);
    let row_overshoots = aggregate.row_amplitude_overshoots(wave);
    let row_flip_overshoots = aggregate.row_flip_count_backstop_overshoots();
    let painted = aggregate.painted_cells();

    println!(
        "contract aggregate wave={} scoring={} rows={} painted_cells={}",
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
            "    >{:>4.1} dB {:>9} cells {:>7.3}%   aggregate_allowed {:>9} {}   \
             row_limit={}x rows_over={} {}",
            rung.over_db,
            counts[rung_index],
            percentage(counts[rung_index], painted),
            allowed,
            mark(!aggregate_overshoots.contains(&rung_index)),
            ROW_ANTI_DILUTION_MULTIPLIER,
            if rows_over.is_empty() {
                "none".to_string()
            } else {
                rows_over.join(",")
            },
            hard_mark(rows_over.is_empty()),
        );
    }

    for (row_index, (row, pair)) in aggregate.rows().iter().zip(paths).enumerate() {
        let row_counts = row.count_rungs(wave);
        let flip_row_ok = !row_flip_overshoots.contains(&row_index);
        println!(
            "  row={} painted={} compared={} presence_changed={} silenced={} \
             paint_edge_crossings={} counts={}/{}/{}/{} bias={:+.4} flips={} \
             flip_row_allowed={} {} \
             longest_flip_run={} flip_runs_gt{}={} reference={} candidate={}",
            row_index + 1,
            row.reference_painted_cells,
            row.compared(),
            row.presence_changed,
            row.flips_newly_silent,
            row.paint_edge_crossings,
            row_counts[0],
            row_counts[1],
            row_counts[2],
            row_counts[3],
            row.loud.signed_mean_db(),
            row.qualifying_flips,
            aggregate.row_flip_count_backstop_allowance(row_index),
            hard_mark(flip_row_ok),
            row.longest_flip_run(),
            LONG_FLIP_RUN_THRESHOLD,
            row.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD),
            pair.reference,
            pair.candidate,
        );
    }

    println!(
        "  ref<30dB max_abs_db={:.3}   limit {:.1} dB per row  {}",
        aggregate.quiet_max_abs_db(),
        wave.quiet_band_max_db(),
        mark(!aggregate.quiet_band_over(wave)),
    );
    println!(
        "  qualifying flips {}   silenced {}   painted-over-silence {}   \
         catastrophic_backstop_allowed {}  {}   row_limit={}x rows_over={}  {}",
        aggregate.qualifying_flips(),
        aggregate.flips_newly_silent(),
        aggregate.flips_newly_painted(),
        aggregate.flip_count_backstop_allowance(),
        hard_mark(!aggregate.flip_count_backstop_over_budget()),
        ROW_ANTI_DILUTION_MULTIPLIER,
        if row_flip_overshoots.is_empty() {
            "none".to_string()
        } else {
            row_flip_overshoots
                .iter()
                .map(|row| (row + 1).to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
        hard_mark(row_flip_overshoots.is_empty()),
    );
    println!(
        "  flip runs components={} histogram={}   longest {} allowed {}  {}",
        aggregate.flip_components(),
        format_flip_histogram(&aggregate.flip_run_histogram()),
        aggregate.longest_flip_run(),
        MAX_FLIP_RUN_LENGTH,
        hard_mark(aggregate.longest_flip_run() <= MAX_FLIP_RUN_LENGTH),
    );
    println!(
        "  flip runs>{} {}   allowed {}  {}",
        LONG_FLIP_RUN_THRESHOLD,
        aggregate.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD),
        MAX_LONG_FLIP_RUNS,
        hard_mark(aggregate.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD) <= MAX_LONG_FLIP_RUNS),
    );
    let bias_rows = scores
        .iter()
        .enumerate()
        .filter(|(_, row)| row.bias_over_budget(wave))
        .map(|(row, _)| (row + 1).to_string())
        .collect::<Vec<_>>();
    println!(
        "  bias per-row |signed_mean_db| limit {:.2}   rows_over={}  {}",
        wave.max_signed_mean_db(),
        if bias_rows.is_empty() {
            "none".to_string()
        } else {
            bias_rows.join(",")
        },
        hard_mark(bias_rows.is_empty()),
    );
    println!(
        "verdict={} wave={} scoring={} aggregate_rows={} painted_cells={} \
         paint_edge_crossings={} \
         cells>{:.0}dB={} cells>{:.0}dB={} flips={} flip_backstop_allowed={} \
         flip_rows_over={} longest_flip_run={} flip_runs_gt{}={}",
        verdict.label(),
        wave.label(),
        scoring.label(),
        scores.len(),
        painted,
        aggregate.paint_edge_crossings(),
        rungs[2].over_db,
        counts[2],
        rungs[3].over_db,
        counts[3],
        aggregate.qualifying_flips(),
        aggregate.flip_count_backstop_allowance(),
        if row_flip_overshoots.is_empty() {
            "none".to_string()
        } else {
            row_flip_overshoots
                .iter()
                .map(|row| (row + 1).to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
        aggregate.longest_flip_run(),
        LONG_FLIP_RUN_THRESHOLD,
        aggregate.flip_runs_longer_than(LONG_FLIP_RUN_THRESHOLD),
    );
    verdict
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

    let verdict = if args.aggregate {
        print_aggregate_verdict(
            &scores,
            &args.pairs,
            args.wave.expect("aggregate requires wave"),
            args.scoring,
        )
    } else {
        print_statistics(&scores[0]);
        match args.wave {
            None => return ExitCode::SUCCESS,
            Some(wave) => print_verdict(&scores[0], wave, args.scoring),
        }
    };
    match verdict {
        Verdict::Fail => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}
