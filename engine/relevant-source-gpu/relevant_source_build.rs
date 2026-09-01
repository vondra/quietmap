//! Compile the fixed relevant-source CUDA program into its statically linked host bridge.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const NOISE_CONSTANTS_SOURCE: &str = include_str!("../noise-compute/src/constants.rs");
const PATH_PROFILE_SOURCE: &str = include_str!("../noise-compute/src/propagation/path_profile.rs");
const SEGMENT_SAMPLING_SOURCE: &str =
    include_str!("../noise-compute/src/propagation/seg_sampling.rs");
const SOURCE_FRAME_SOURCE: &str = include_str!("src/source_frame.rs");
const INPUT_TYPES_SOURCE: &str = include_str!("../noise-compute/src/types/inputs.rs");

fn constant_initializer<'a>(source: &'a str, constant_name: &str) -> &'a str {
    let declaration = format!("pub const {constant_name}:");
    let declaration_start = source
        .find(&declaration)
        .unwrap_or_else(|| panic!("canonical constant {constant_name} is absent"));
    let declaration_tail = &source[declaration_start + declaration.len()..];
    let equals = declaration_tail
        .find('=')
        .unwrap_or_else(|| panic!("canonical constant {constant_name} has no initializer"));
    let initializer_tail = &declaration_tail[equals + 1..];
    let semicolon = initializer_tail
        .find(';')
        .unwrap_or_else(|| panic!("canonical constant {constant_name} has no semicolon"));
    initializer_tail[..semicolon].trim()
}

fn canonical_f64(source: &str, constant_name: &str) -> f64 {
    constant_initializer(source, constant_name)
        .replace('_', "")
        .parse()
        .unwrap_or_else(|error| {
            panic!("canonical constant {constant_name} is not literal: {error}")
        })
}

fn canonical_usize(source: &str, constant_name: &str) -> usize {
    constant_initializer(source, constant_name)
        .replace('_', "")
        .parse()
        .unwrap_or_else(|error| {
            panic!("canonical constant {constant_name} is not literal: {error}")
        })
}

fn canonical_bool(source: &str, constant_name: &str) -> bool {
    constant_initializer(source, constant_name)
        .parse()
        .unwrap_or_else(|error| {
            panic!("canonical constant {constant_name} is not literal: {error}")
        })
}

fn canonical_f64_array<const LENGTH: usize>(source: &str, constant_name: &str) -> [f64; LENGTH] {
    let initializer = constant_initializer(source, constant_name);
    let body = initializer
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_else(|| panic!("canonical constant {constant_name} is not an array"));
    let values: Vec<f64> =
        body.split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value.replace('_', "").parse().unwrap_or_else(|error| {
                    panic!("invalid {constant_name} entry {value}: {error}")
                })
            })
            .collect();
    values.try_into().unwrap_or_else(|values: Vec<f64>| {
        panic!(
            "canonical constant {constant_name} has {} entries, expected {LENGTH}",
            values.len()
        )
    })
}

fn cuda_float(value: f64) -> String {
    assert!(value.is_finite(), "generated CUDA constant must be finite");
    format!("{:?}f", value as f32)
}

fn write_cuda_float(header: &mut String, name: &str, value: f64) {
    writeln!(header, "constexpr float {name} = {};", cuda_float(value)).unwrap();
}

fn write_cuda_array<const LENGTH: usize>(header: &mut String, name: &str, values: [f64; LENGTH]) {
    writeln!(
        header,
        "__device__ __constant__ float {name}[QUIETMAP_BAND_COUNT] = {{"
    )
    .unwrap();
    for value in values {
        writeln!(header, "    {},", cuda_float(value)).unwrap();
    }
    header.push_str("};\n");
}

fn generated_physics_header() -> String {
    let band_frequencies = canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "BAND_FREQ");
    let speed_of_sound = canonical_f64(NOISE_CONSTANTS_SOURCE, "SPEED_OF_SOUND");
    assert_eq!(
        constant_initializer(NOISE_CONSTANTS_SOURCE, "PENUMBRA_DELTA_FLOOR_M"),
        "-SPEED_OF_SOUND / 63.0 / 20.0"
    );
    assert_eq!(
        constant_initializer(PATH_PROFILE_SOURCE, "CELL_M"),
        "crate::constants::M_PER_DEG_LAT / 3600.0"
    );
    assert_eq!(
        constant_initializer(INPUT_TYPES_SOURCE, "BARRIER_PATH_HORIZON_M"),
        "BARRIER_SEGMENT_MAX_HALF_LEN_M + 50.0"
    );
    let favourable_probability = if canonical_bool(NOISE_CONSTANTS_SOURCE, "FAVOURABLE_MIXING") {
        canonical_f64(NOISE_CONSTANTS_SOURCE, "P_FAV")
    } else {
        0.0
    };
    let mut header = String::from(
        "//! Generated only from canonical noise-compute constants; do not edit.\n\n#pragma once\n\n",
    );
    write_cuda_array(&mut header, "QUIETMAP_BAND_FREQUENCIES", band_frequencies);
    write_cuda_array(
        &mut header,
        "QUIETMAP_A_WEIGHTING",
        canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "A_WEIGHTING"),
    );
    write_cuda_array(
        &mut header,
        "QUIETMAP_ATMOSPHERIC_DB_PER_KM",
        canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "ALPHA_ATM"),
    );
    write_cuda_array(
        &mut header,
        "QUIETMAP_VEGETATION_DB_PER_M",
        canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "ALPHA_VEG"),
    );
    write_cuda_array(
        &mut header,
        "QUIETMAP_VEGETATION_CAP_DB",
        canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "MAX_VEG_ATTEN"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_SPEED_OF_SOUND_M_PER_S",
        speed_of_sound,
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_PENUMBRA_DELTA_FLOOR_M",
        -speed_of_sound / band_frequencies[0] / 20.0,
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_HARD_FLOOR_DB",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "GROUND_HARD_FLOOR_DB"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_DEFAULT_RECEIVER_HEIGHT_M",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "DEFAULT_RECEIVER_HEIGHT"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_MINIMUM_SOURCE_HEIGHT_M",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "SOURCE_HEIGHT_ROAD"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_FAVOURABLE_PROBABILITY",
        favourable_probability,
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_FAVOURABLE_CURVATURE_MINIMUM_M",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "FAV_RAY_CURVATURE_MIN_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_FAVOURABLE_CURVATURE_PER_DISTANCE",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "FAV_RAY_CURVATURE_PER_DSR"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_SINGLE_DIFFRACTION_CAP_DB",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "SINGLE_DIFF_CAP"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_RASTER_CELL_M",
        canonical_f64(NOISE_CONSTANTS_SOURCE, "M_PER_DEG_LAT") / 3600.0,
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_NEAR_SAMPLE_M",
        canonical_f64(PATH_PROFILE_SOURCE, "NEAR_OFFSET_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_MINIMUM_FOREST_RUN_M",
        canonical_f64(PATH_PROFILE_SOURCE, "VEGETATION_MIN_RUN_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_BARRIER_PATH_HORIZON_M",
        canonical_f64(INPUT_TYPES_SOURCE, "BARRIER_SEGMENT_MAX_HALF_LEN_M") + 50.0,
    );
    writeln!(
        header,
        "constexpr int QUIETMAP_LINE_DIRECTION_COUNT = {};",
        canonical_usize(SEGMENT_SAMPLING_SOURCE, "SEG_SAMPLES_DEFAULT")
    )
    .unwrap();
    writeln!(
        header,
        "constexpr int QUIETMAP_BLOCK_PIXEL_SIDE = {};",
        canonical_usize(SOURCE_FRAME_SOURCE, "BLOCK_PIXEL_SIDE")
    )
    .unwrap();
    header
}

fn run_checked(command: &mut Command, label: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to start {label}: {error}"));
    assert!(status.success(), "{label} exited with {status}");
}

fn main() {
    println!("cargo:rerun-if-changed=kernels/relevant_source_geometry.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_path.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_attenuation.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_grid_scan.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_obstacles.cuh");
    println!("cargo:rerun-if-changed=kernels/block_source_partition.cu");
    println!("cargo:rerun-if-changed=../noise-compute/src/constants.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/path_profile.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/seg_sampling.rs");
    println!("cargo:rerun-if-changed=src/source_frame.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/types/inputs.rs");
    if env::var_os("CARGO_FEATURE_GPU").is_none() {
        return;
    }

    let output_directory = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    fs::write(
        output_directory.join("relevant_source_physics_constants.cuh"),
        generated_physics_header(),
    )
    .expect("write generated relevant-source physics constants");
    let object_path = output_directory.join("block_source_partition.o");
    let archive_path = output_directory.join("librelevant_source_cuda.a");
    run_checked(
        Command::new("nvcc")
            .args([
                "-std=c++17",
                "-O3",
                "--use_fast_math",
                "-lineinfo",
                "-arch=sm_120",
                "-maxrregcount=40",
                "-Xcompiler",
                "-fPIC",
                "-I",
            ])
            .arg(&output_directory)
            .args(["-c", "kernels/block_source_partition.cu", "-o"])
            .arg(&object_path),
        "nvcc relevant-source compilation",
    );
    run_checked(
        Command::new("ar")
            .arg("crs")
            .arg(&archive_path)
            .arg(&object_path),
        "relevant-source CUDA archive",
    );

    println!(
        "cargo:rustc-link-search=native={}",
        output_directory.display()
    );
    println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
    println!("cargo:rustc-link-lib=static=relevant_source_cuda");
    println!("cargo:rustc-link-lib=dylib=cudart");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_header_reads_the_canonical_physics_sources() {
        let header = generated_physics_header();
        assert!(header.contains("constexpr float QUIETMAP_DEFAULT_RECEIVER_HEIGHT_M = 4.0f;"));
        assert!(header.contains("constexpr int QUIETMAP_LINE_DIRECTION_COUNT = 5;"));
        assert!(header.contains("constexpr int QUIETMAP_BLOCK_PIXEL_SIDE = "));
        assert!(header.contains("constexpr float QUIETMAP_BARRIER_PATH_HORIZON_M = 175.0f;"));
        assert!(header.contains("constexpr float QUIETMAP_PENUMBRA_DELTA_FLOOR_M ="));
    }
}
