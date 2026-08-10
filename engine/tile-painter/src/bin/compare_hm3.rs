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
//! Usage: `compare_hm3 <reference.bin> <candidate.bin> [--wave 1|2]
//!         [--scoring absolute|marginal]`

use std::path::Path;
use std::process::ExitCode;

use tile_painter::accuracy_contract::{allowance, score, Score, Scoring, Verdict, Wave};
use tile_painter::wire_hm3;

const USAGE: &str = "usage: compare_hm3 <reference.bin> <candidate.bin> \
[--wave 1|2] [--scoring absolute|marginal]";

struct Args {
    reference: String,
    candidate: String,
    wave: Option<Wave>,
    scoring: Scoring,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
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
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }
    if positional.len() != 2 {
        return Err(USAGE.to_string());
    }
    Ok(Args {
        reference: positional[0].clone(),
        candidate: positional[1].clone(),
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
            100.0 * counts[i] as f64 / s.cells as f64,
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
    // The two hard gates that two tiles can show. The third — physics deleted rather
    // than approximated — is a code-review gate this tool cannot stand in for.
    println!(
        "  flips>=1dB clear of 30dB {}   silenced {}   painted-over-silence {}   \
         allowed {}  {}",
        s.qualifying_flips,
        s.flips_newly_silent,
        s.flips_newly_painted,
        s.flip_allowance(),
        if s.flips_over_budget() { "FAIL" } else { "ok" },
    );
    let bias = s.loud.signed_mean_db().abs();
    println!(
        "  bias |signed_mean_db| {bias:.4}   limit {:.2}  {}",
        tile_painter::accuracy_contract::MAX_SIGNED_MEAN_DB,
        if bias <= tile_painter::accuracy_contract::MAX_SIGNED_MEAN_DB {
            "ok"
        } else {
            "FAIL"
        },
    );
    // One greppable line carrying the numbers that decide a configuration: the top
    // rung's count (which decides a DRAFT, where there is no ceiling above it) and the
    // flip count, for whichever contract was scored.
    let rungs = wave.rungs();
    println!(
        "verdict={} wave={} scoring={} cells>{:.0}dB={} cells>{:.0}dB={} flips={}",
        verdict.label(),
        wave.label(),
        scoring.label(),
        rungs[2].over_db,
        counts[2],
        rungs[3].over_db,
        counts[3],
        s.qualifying_flips,
    );
    verdict
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let reference = match wire_hm3::read_tile(Path::new(&args.reference)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("reading {}: {e}", args.reference);
            return ExitCode::from(2);
        }
    };
    let candidate = match wire_hm3::read_tile(Path::new(&args.candidate)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("reading {}: {e}", args.candidate);
            return ExitCode::from(2);
        }
    };
    if reference.len() != candidate.len() {
        eprintln!(
            "tile cell counts differ: {} vs {}",
            reference.len(),
            candidate.len()
        );
        return ExitCode::from(2);
    }

    let scored = score(&reference, &candidate);
    print_statistics(&scored);
    match args.wave {
        None => ExitCode::SUCCESS,
        Some(wave) => match print_verdict(&scored, wave, args.scoring) {
            Verdict::Fail => ExitCode::FAILURE,
            _ => ExitCode::SUCCESS,
        },
    }
}
