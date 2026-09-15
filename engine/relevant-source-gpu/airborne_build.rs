//! CUDA airborne constants read from the canonical Rust acoustics declarations.
use std::{fmt::Write, fs, path::Path};

pub fn header() -> String {
    let mut out = String::from("// Generated from noise-compute; do not edit.\n#pragma once\n");
    let files = [
        (
            "../noise-compute/src/emission/aircraft/doc29.rs",
            vec![
                ("M_PER_DEG_LAT", "AIRCRAFT_M_LAT"),
                ("PERIOD_SECONDS", "AIRBORNE_PERIOD_SECONDS"),
            ],
        ),
        (
            "../noise-compute/src/emission/aircraft/npd/mod.rs",
            vec![
                ("AIRCRAFT_MAX_HORIZONTAL_REACH_M", "AIRBORNE_REACH_M"),
                ("AIRCRAFT_FAR_FIELD_THRESHOLD_M", "FARFIELD_M"),
                ("FT_PER_M", "FT_PER_M"),
                ("NPD_LUT_BINS", "NPD_NB"),
                ("NPD_LUT_LOG_MIN", "NPD_LOG_MIN"),
                ("NPD_LUT_LOG_MAX", "NPD_LOG_MAX"),
            ],
        ),
        (
            "../noise-compute/src/emission/aircraft/horizon.rs",
            vec![
                ("HORIZON_SECTORS", "TERRAIN_SECTORS"),
                ("RECEIVER_HORIZON_BANDS", "TERRAIN_BANDS"),
                ("RECEIVER_HORIZON_TANGENT_SCALE", "TAN_SCALE_D"),
                ("RECEIVER_HORIZON_RANGE_SCALE", "TERRAIN_RANGE_SCALE_D"),
            ],
        ),
        (
            "../noise-compute/src/emission/aircraft/screening.rs",
            vec![
                ("BUILDING_LOCAL_HORIZON_SECTORS", "BUILDING_LOCAL_SECTORS"),
                ("BUILDING_LOCAL_HORIZON_BANDS", "BUILDING_LOCAL_BANDS"),
                ("BUILDING_HORIZON_RANGE_SCALE", "BUILDING_RANGE_SCALE_D"),
                ("DIFFRACTION_SLOPE_PER_M", "DIFFRACTION_SLOPE_D"),
                ("DIFFRACTION_GRAZING_DB", "DIFFRACTION_GRAZING_DB_D"),
                ("DIFFRACTION_CAP_DB", "DIFFRACTION_CAP_DB_D"),
            ],
        ),
    ];
    for (path, names) in files {
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(path).expect("canonical airborne constants");
        for (name, cuda) in names {
            let needle = format!("const {name}:");
            let line = source
                .lines()
                .find(|line| line.contains(&needle))
                .expect("canonical constant");
            let value = line
                .split_once('=')
                .expect("constant initializer")
                .1
                .split(';')
                .next()
                .unwrap()
                .trim()
                .replace('_', "");
            let value = value.replace('[', "{").replace(']', "}");
            writeln!(out, "#define {cuda} {value}").unwrap();
        }
    }
    let packing = "src/airborne_pack.rs";
    println!("cargo:rerun-if-changed={packing}");
    let source = fs::read_to_string(packing).expect("airborne reduction size");
    let line = source
        .lines()
        .find(|line| line.contains("const AIRBORNE_REDUCTION_ROWS:"))
        .unwrap();
    writeln!(
        out,
        "#define AIRBORNE_REDUCTION_ROWS {}",
        line.split_once('=').unwrap().1.trim().trim_end_matches(';')
    )
    .unwrap();
    let profiles = "../noise-compute/src/emission/profiles_generated.rs";
    println!("cargo:rerun-if-changed={profiles}");
    let source = fs::read_to_string(profiles).expect("canonical aircraft classes");
    let line = source
        .lines()
        .find(|line| line.contains("const NUM_CLASSES:"))
        .unwrap();
    writeln!(
        out,
        "#define NPD_NC {}",
        line.split_once('=').unwrap().1.trim().trim_end_matches(';')
    )
    .unwrap();
    out
}

pub fn compile(output: &Path, arguments: &[String]) -> std::path::PathBuf {
    for file in [
        "airborne.cu",
        "airborne_energy.cuh",
        "airborne_screening.cuh",
    ] {
        println!("cargo:rerun-if-changed=kernels/{file}");
    }
    fs::write(output.join("airborne_defines.cuh"), header()).expect("write airborne constants");
    let object = output.join("airborne.o");
    super::run_checked(
        std::process::Command::new("nvcc")
            .args(arguments)
            // Discrete horizon sectors require the CPU's non-contracted f64 CPA operations.
            .arg("--fmad=false")
            .arg("-I")
            .arg(output)
            .args(["-c", "kernels/airborne.cu", "-o"])
            .arg(&object),
        "nvcc airborne compilation",
    );
    object
}
