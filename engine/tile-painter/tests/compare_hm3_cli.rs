//! End-to-end contract checks for the `compare_hm3` release-gate CLI.

use std::path::Path;
use std::process::{Command, Output};

use tile_painter::grid::TILE_PX;
use tile_painter::wire_hm3::{quantise_lden, write_tile, NO_DATA, SOURCE_ID_ROAD};

fn write_fixture(path: &Path, cells: &[u8]) {
    write_tile(path, cells, SOURCE_ID_ROAD, false).expect("write HM3 fixture");
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_compare_hm3"))
        .args(args)
        .output()
        .expect("run compare_hm3")
}

fn run_owned(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_compare_hm3"))
        .args(args)
        .output()
        .expect("run compare_hm3")
}

fn aggregate_args(
    reference: &str,
    candidates: &[&str],
    wave: &str,
    scoring: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["--aggregate".to_string()];
    for candidate in candidates {
        args.push(reference.to_string());
        args.push((*candidate).to_string());
    }
    args.extend(["--wave".to_string(), wave.to_string()]);
    if let Some(scoring) = scoring {
        args.extend(["--scoring".to_string(), scoring.to_string()]);
    }
    args
}

#[test]
fn legacy_line_aggregate_verdict_and_exit_codes_stay_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let identical_path = dir.path().join("identical.bin");
    let draft_path = dir.path().join("draft.bin");
    let local_break_path = dir.path().join("local-break.bin");
    let local_flip_break_path = dir.path().join("local-flip-break.bin");
    let erased_path = dir.path().join("erased.bin");
    let cells = TILE_PX * TILE_PX;
    let reference = vec![quantise_lden(60.0); cells];
    write_fixture(&reference_path, &reference);
    write_fixture(&identical_path, &reference);

    let reference_arg = reference_path.to_str().unwrap();
    let identical_arg = identical_path.to_str().unwrap();
    let output = run(&[reference_arg, identical_arg]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "both=262144 mean_abs_db=0.0000 max_abs_db=0.000 cells>0.5dB=0 \
         cells>1.0dB=0 presence_changed=0 signed_mean_db=+0.0000 moved=0 \
         cand_louder_pct=0.0\n"
    );

    let wave_two_candidates = vec![identical_arg; 980];
    let output = run_owned(&aggregate_args(
        reference_arg,
        &wave_two_candidates,
        "2",
        None,
    ));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("contract aggregate wave=2 (accurate)"));
    assert!(stdout.contains("painted_cells=256901120"));
    assert!(stdout.contains("presence_changed=0"));
    assert!(stdout.contains("paint_edge_crossings=0"));
    assert!(stdout.contains("aggregate_gate=ok"));
    assert!(stdout.contains("longest 0 allowed=none diagnostic_only"));
    assert!(stdout.contains("flip runs>4 0   allowed=none diagnostic_only"));
    assert!(stdout.contains("verdict=PASS wave=2 (accurate)"));

    // A subset can inspect aggregate arithmetic but never emits a release verdict.
    let output = run(&["--aggregate", reference_arg, identical_arg, "--wave", "2"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("contract aggregate diagnostic_only wave=2 (accurate)"));
    assert!(stdout.contains("expected_benchmark_rows=980"));
    assert!(!stdout.contains("verdict="));

    // A single row is diagnostic only, separate from the historical no-wave line.
    let output = run(&[
        reference_arg,
        identical_arg,
        "--wave",
        "2",
        "--scoring",
        "marginal",
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("contract diagnostic_only wave=2 (accurate) scoring=marginal"));
    assert!(stdout.contains("paint-edge crossings 0"));
    assert!(stdout.contains("diagnostic_only wave=2 (accurate) scoring=marginal"));
    assert!(!stdout.contains("verdict="));

    // Wave 1's aggregate amplitude ladder is binding too.
    let mut draft = reference.clone();
    for cell in draft.iter_mut().take(cells / 8) {
        *cell = quantise_lden(62.0);
    }
    for cell in draft.iter_mut().skip(cells / 8).take(cells / 8) {
        *cell = quantise_lden(58.0);
    }
    write_fixture(&draft_path, &draft);
    let draft_candidates = vec![draft_path.to_str().unwrap(); 250];
    let output = run_owned(&aggregate_args(reference_arg, &draft_candidates, "1", None));
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("aggregate_gate=FAIL"));
    assert!(stdout.contains("verdict=FAIL"));

    // Across the complete 250-row verdict domain, a 70%-broken row contributes
    // 0.28% aggregate debt, below Wave 1's 20% rung. The former 3x row reference
    // (60%) is printed but does not gate; also exercise the explicit scoring label.
    let mut local_break = reference.clone();
    let thirty_five_percent = cells * 35 / 100;
    for cell in local_break.iter_mut().take(thirty_five_percent) {
        *cell = quantise_lden(62.0);
    }
    for cell in local_break
        .iter_mut()
        .skip(thirty_five_percent)
        .take(thirty_five_percent)
    {
        *cell = quantise_lden(58.0);
    }
    write_fixture(&local_break_path, &local_break);
    let local_break_arg = local_break_path.to_str().unwrap();
    let mut local_break_candidates = vec![identical_arg; 250];
    local_break_candidates[249] = local_break_arg;
    let output = run_owned(&aggregate_args(
        reference_arg,
        &local_break_candidates,
        "1",
        Some("marginal"),
    ));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("scoring=marginal rows=250"));
    assert!(stdout.contains("row_reference=3x rows_over=250 row_diagnostic_only"));
    assert!(stdout.contains("row=250"));
    assert!(stdout.contains("verdict=PASS wave=1 (draft) scoring=marginal"));

    // Isolate the former per-row presence gate through the real 250-row CLI: 7,865
    // disconnected deletions are below the aggregate allowance (655,360), but one
    // above this row's inclusive 3% diagnostic reference (7,864).
    let mut local_flip_break = reference.clone();
    let isolated_indices = (0..TILE_PX)
        .step_by(3)
        .flat_map(|y| (0..TILE_PX).step_by(3).map(move |x| y * TILE_PX + x));
    for index in isolated_indices.take(7_865) {
        local_flip_break[index] = NO_DATA;
    }
    write_fixture(&local_flip_break_path, &local_flip_break);
    let local_flip_break_arg = local_flip_break_path.to_str().unwrap();
    let mut local_flip_candidates = vec![identical_arg; 250];
    local_flip_candidates[249] = local_flip_break_arg;
    let output = run_owned(&aggregate_args(
        reference_arg,
        &local_flip_candidates,
        "1",
        None,
    ));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout
        .contains("flips=7865 presence_pct=3.000 presence_3x_reference=7864 OVER diagnostic_only"));
    assert!(stdout.contains("row_reference=3x rows_over=250 row_diagnostic_only"));
    assert!(stdout.contains("diagnostic_presence_rows_over_3x=250"));
    assert!(stdout.contains("eyeball_rows=none"));
    assert!(stdout.contains("verdict=PASS"));

    write_fixture(&erased_path, &vec![NO_DATA; cells]);
    let output = run(&[
        "--aggregate",
        reference_arg,
        erased_path.to_str().unwrap(),
        "--wave",
        "2",
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("contract aggregate diagnostic_only"));
    assert!(stdout.contains("aggregate_gate=OVER"));
    assert!(stdout.contains("expected_benchmark_rows=980"));
    assert!(!stdout.contains("verdict="));

    let erased_candidates = vec![erased_path.to_str().unwrap(); 980];
    let output = run_owned(&aggregate_args(
        reference_arg,
        &erased_candidates,
        "2",
        None,
    ));
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("verdict=FAIL"));
    assert!(stdout.contains("eyeball presence>5% rows=1,2"));
    assert!(stdout.contains(",979,980 bias>2.0dB rows=none inspect_rows="));
    assert!(stdout.contains("eyeball_rows=1,2"));
    assert!(stdout.contains(",979,980 longest_flip_run=262144"));

    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(
        run(&["--aggregate", reference_arg, identical_arg])
            .status
            .code(),
        Some(2)
    );
}
