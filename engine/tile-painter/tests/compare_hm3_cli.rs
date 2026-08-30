//! End-to-end contract checks for the `compare_hm3` release-gate CLI.

use std::path::Path;
use std::process::{Command, Output};

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

fn release_layer_args(reference: &str, candidate: &str, rows: usize, layer: &str) -> Vec<String> {
    let candidates = vec![candidate; rows];
    let mut args = aggregate_args(reference, &candidates, "1", None);
    args.extend(["--release-layer".to_string(), layer.to_string()]);
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
fn failed_release_verdict_maps_to_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let erased_path = dir.path().join("erased.bin");
    let cells = TILE_PX * TILE_PX;
    write_fixture(&reference_path, &vec![quantise_lden(60.0); cells]);
    write_fixture(&erased_path, &vec![NO_DATA; cells]);

    let output = run_owned(&release_layer_args(
        reference_path.to_str().unwrap(),
        erased_path.to_str().unwrap(),
        125,
        "road",
    ));
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("verdict=FAIL"));
}

#[test]
fn per_layer_release_gate_is_125_rows_fail_closed_and_provenance_checked() {
    let dir = tempfile::tempdir().unwrap();
    let reference_path = dir.path().join("reference.bin");
    let identical_path = dir.path().join("identical.bin");
    let wrong_reference_path = dir.path().join("wrong-reference.bin");
    let wrong_candidate_path = dir.path().join("wrong-candidate.bin");
    let cells = TILE_PX * TILE_PX;
    let reference = vec![quantise_lden(60.0); cells];
    write_fixture(&reference_path, &reference);
    write_fixture(&identical_path, &reference);
    write_fixture_with_source_id(&wrong_reference_path, &reference, SOURCE_ID_RAIL);
    write_fixture_with_source_id(&wrong_candidate_path, &reference, SOURCE_ID_RAIL);

    let reference_arg = reference_path.to_str().unwrap();
    let identical_arg = identical_path.to_str().unwrap();

    let output = run_owned(&release_layer_args(
        reference_arg,
        identical_arg,
        125,
        "road",
    ));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(
        "contract aggregate layer=road scope=release-layer expected_rows=125 wave=1 (draft)"
    ));
    assert!(stdout.contains("verdict=PASS"));
    assert!(stdout.contains("layer=road scope=release-layer expected_rows=125"));

    let rows = 124;
    let output = run_owned(&release_layer_args(
        reference_arg,
        identical_arg,
        rows,
        "road",
    ));
    assert!(output.status.success(), "{rows} rows must stay diagnostic");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(
        "contract aggregate diagnostic_only layer=road scope=release-layer expected_rows=125"
    ));
    assert!(stdout.contains("expected_benchmark_rows=125"));
    assert!(!stdout.lines().any(|line| line.starts_with("verdict=")));

    assert_eq!(
        run(&[
            "--aggregate",
            reference_arg,
            identical_arg,
            "--wave",
            "2",
            "--release-layer",
            "road",
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        run(&[
            reference_arg,
            identical_arg,
            "--wave",
            "1",
            "--release-layer",
            "road",
        ])
        .status
        .code(),
        Some(2)
    );
    for layer in ["total", "unknown"] {
        assert_eq!(
            run(&[
                "--aggregate",
                reference_arg,
                identical_arg,
                "--wave",
                "1",
                "--release-layer",
                layer,
            ])
            .status
            .code(),
            Some(2),
            "{layer} is outside the per-layer release allow-list"
        );
    }

    let wrong_reference_arg = wrong_reference_path.to_str().unwrap();
    let output = run_owned(&release_layer_args(
        wrong_reference_arg,
        identical_arg,
        1,
        "road",
    ));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("reference"));
    assert!(stderr.contains("source_id 2, expected 1"));

    let wrong_candidate_arg = wrong_candidate_path.to_str().unwrap();
    let output = run_owned(&release_layer_args(
        reference_arg,
        wrong_candidate_arg,
        1,
        "road",
    ));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("candidate"));
    assert!(stderr.contains("source_id 2, expected 1"));
}
