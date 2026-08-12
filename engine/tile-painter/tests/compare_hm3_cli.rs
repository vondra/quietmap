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

    let output = run(&["--aggregate", reference_arg, identical_arg, "--wave", "2"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("contract aggregate wave=2 (accurate)"));
    assert!(stdout.contains("painted_cells=262144"));
    assert!(stdout.contains("presence_changed=0"));
    assert!(stdout.contains("paint_edge_crossings=0"));
    assert!(stdout.contains("verdict=PASS wave=2 (accurate)"));

    // The non-aggregate verdict block and asserted reference label are release
    // interfaces too, separate from the historical no-wave statistics line.
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
    assert!(stdout.contains("contract wave=2 (accurate) scoring=marginal"));
    assert!(stdout.contains("paint-edge crossings 0"));
    assert!(stdout.contains("verdict=PASS wave=2 (accurate) scoring=marginal"));

    // Wave 1 amplitude debt exits successfully but is never mislabeled a clean PASS.
    let mut draft = reference.clone();
    for cell in draft.iter_mut().take(cells / 8) {
        *cell = quantise_lden(62.0);
    }
    for cell in draft.iter_mut().skip(cells / 8).take(cells / 8) {
        *cell = quantise_lden(58.0);
    }
    write_fixture(&draft_path, &draft);
    let output = run(&[
        "--aggregate",
        reference_arg,
        draft_path.to_str().unwrap(),
        "--wave",
        "1",
    ]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("verdict=PASS_WITH_OVERSHOOT"));

    // Four equal rows make a 70%-broken row look like only 17.5% aggregate debt,
    // below Wave 1's 20% rung. The hard 3x row ceiling (60%) must still fail through
    // the real multi-row CLI path; also exercise the explicit scoring label.
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
    let output = run(&[
        "--aggregate",
        reference_arg,
        identical_arg,
        reference_arg,
        identical_arg,
        reference_arg,
        identical_arg,
        reference_arg,
        local_break_arg,
        "--wave",
        "1",
        "--scoring",
        "marginal",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("scoring=marginal rows=4"));
    assert!(stdout.contains("row_limit=3x rows_over=4 FAIL"));
    assert!(stdout.contains("row=4"));
    assert!(stdout.contains("verdict=FAIL wave=1 (draft) scoring=marginal"));

    // Isolate the new per-row flip gate through the real multi-row CLI: 7,865
    // disconnected deletions are below the four-row aggregate allowance (10,486),
    // but one above this row's inclusive 3% allowance (7,864).
    let mut local_flip_break = reference.clone();
    let isolated_indices = (0..TILE_PX)
        .step_by(3)
        .flat_map(|y| (0..TILE_PX).step_by(3).map(move |x| y * TILE_PX + x));
    for index in isolated_indices.take(7_865) {
        local_flip_break[index] = NO_DATA;
    }
    write_fixture(&local_flip_break_path, &local_flip_break);
    let local_flip_break_arg = local_flip_break_path.to_str().unwrap();
    let output = run(&[
        "--aggregate",
        reference_arg,
        identical_arg,
        reference_arg,
        identical_arg,
        reference_arg,
        identical_arg,
        reference_arg,
        local_flip_break_arg,
        "--wave",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("flips=7865 flip_row_allowed=7864 FAIL"));
    assert!(stdout.contains("row_limit=3x rows_over=4  FAIL"));
    assert!(stdout.contains("flip_rows_over=4"));

    write_fixture(&erased_path, &vec![NO_DATA; cells]);
    let output = run(&[
        "--aggregate",
        reference_arg,
        erased_path.to_str().unwrap(),
        "--wave",
        "2",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("verdict=FAIL"));

    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(
        run(&["--aggregate", reference_arg, identical_arg])
            .status
            .code(),
        Some(2)
    );
}
