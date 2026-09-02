//! End-to-end contract checks for the `compare_hm3` release-gate CLI.

use std::path::Path;
use std::process::{Command, Output};

use tile_painter::accuracy_contract::WAVE_TWO_BENCHMARK_ROWS;
use tile_painter::grid::TILE_PX;
use tile_painter::wire_hm3::{quantise_lden, write_tile, NO_DATA, SOURCE_ID_RAIL, SOURCE_ID_ROAD};

fn write_fixture(path: &Path, cells: &[u8]) {
    write_fixture_with_source_id(path, cells, SOURCE_ID_ROAD);
}

fn write_fixture_with_source_id(path: &Path, cells: &[u8], source_id: u8) {
    write_tile(path, cells, source_id, false).expect("write HM3 fixture");
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

/// `rows` copies of one reference/candidate pair, scored as the contract.
fn aggregate_args(reference: &str, candidate: &str, rows: usize) -> Vec<String> {
    let mut args = vec!["--aggregate".to_string()];
    for _ in 0..rows {
        args.push(reference.to_string());
        args.push(candidate.to_string());
    }
    args.extend(["--wave".to_string(), "2".to_string()]);
    args
}

#[test]
fn cli_reads_hm3_and_keeps_legacy_output_shape() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let identical_path = dir.path().join("identical.bin");
    let reference = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
    write_fixture(&reference_path, &reference);
    write_fixture(&identical_path, &reference);

    let output = run(&[
        reference_path.to_str().unwrap(),
        identical_path.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "both=262144 mean_abs_db=0.0000 max_abs_db=0.000 cells>0.5dB=0 \
         cells>1.0dB=0 presence_changed=0 signed_mean_db=+0.0000 moved=0 \
         cand_louder_pct=0.0\n"
    );
}

#[test]
fn the_release_verdict_needs_the_complete_benchmark_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let identical_path = dir.path().join("identical.bin");
    let reference = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
    write_fixture(&reference_path, &reference);
    write_fixture(&identical_path, &reference);
    let reference_arg = reference_path.to_str().unwrap();
    let identical_arg = identical_path.to_str().unwrap();

    let output = run_owned(&aggregate_args(
        reference_arg,
        identical_arg,
        WAVE_TWO_BENCHMARK_ROWS,
    ));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!(
        "contract aggregate wave=2 (accurate) scoring=absolute rows={WAVE_TWO_BENCHMARK_ROWS}"
    )));
    assert!(stdout.contains("verdict=PASS"));

    let short = run_owned(&aggregate_args(
        reference_arg,
        identical_arg,
        WAVE_TWO_BENCHMARK_ROWS - 1,
    ));
    assert!(short.status.success(), "a short workset stays diagnostic");
    let stdout = String::from_utf8(short.stdout).unwrap();
    assert!(stdout.contains("contract aggregate diagnostic_only wave=2 (accurate)"));
    assert!(stdout.contains(&format!(
        "expected_benchmark_rows={WAVE_TWO_BENCHMARK_ROWS}"
    )));
    assert!(!stdout.lines().any(|line| line.starts_with("verdict=")));

    // The retired draft wave is refused by name rather than silently rescored.
    let retired = run(&[reference_arg, identical_arg, "--wave", "1"]);
    assert_eq!(retired.status.code(), Some(2));
    assert!(String::from_utf8(retired.stderr)
        .unwrap()
        .contains("retired"));
}

#[test]
fn failed_release_verdict_maps_to_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let erased_path = dir.path().join("erased.bin");
    let cells = TILE_PX * TILE_PX;
    write_fixture(&reference_path, &vec![quantise_lden(60.0); cells]);
    write_fixture(&erased_path, &vec![NO_DATA; cells]);

    let output = run_owned(&aggregate_args(
        reference_path.to_str().unwrap(),
        erased_path.to_str().unwrap(),
        WAVE_TWO_BENCHMARK_ROWS,
    ));
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("verdict=FAIL"));
}

#[test]
fn a_pair_of_two_different_layers_is_refused_before_it_is_scored() {
    // Scoring road against rail measures nothing, and the HM3 source discriminator is
    // the only thing that can catch a mis-wired pair, so it is checked on every pair.
    let dir = tempfile::tempdir().unwrap();
    let road_path = dir.path().join("road.bin");
    let rail_path = dir.path().join("rail.bin");
    let cells = vec![quantise_lden(60.0); TILE_PX * TILE_PX];
    write_fixture_with_source_id(&road_path, &cells, SOURCE_ID_ROAD);
    write_fixture_with_source_id(&rail_path, &cells, SOURCE_ID_RAIL);

    let output = run(&[road_path.to_str().unwrap(), rail_path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("source_id"), "{stderr}");
    assert!(stderr.contains(&format!("{SOURCE_ID_ROAD}")));
    assert!(stderr.contains(&format!("{SOURCE_ID_RAIL}")));
}
