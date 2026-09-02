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
const SCATTER_BAND_SOURCE: &str = include_str!("../tile-painter/src/scatter_band.rs");
const ARC_SCREENING_SOURCE: &str =
    include_str!("../noise-compute/src/propagation/arc_screening.rs");
const GEO_SOURCE: &str = include_str!("../noise-compute/src/propagation/geo.rs");
const GROUND_OPS_SOURCE: &str = include_str!("../tile-painter/src/ground_ops.rs");
const PATH_EFFECTS_SOURCE: &str = include_str!("../noise-compute/src/propagation/path_effects.rs");
const ISO9613_SOURCE: &str = include_str!("../noise-compute/src/propagation/iso9613.rs");
const FUSED_TILE_SOURCE: &str = include_str!("../raster-reader/src/fused_tile_z13.rs");

/// The initializer of the ONE `const NAME:` declaration in `source`: a line that
/// starts (after indentation and an optional `pub` / `pub(crate)`) with the
/// declaration, so a mention in a comment or a string never matches, and a
/// second declaration fails the build instead of silently winning.
fn constant_initializer<'a>(source: &'a str, constant_name: &str) -> &'a str {
    let declaration = format!("const {constant_name}:");
    let mut declarations = source.lines().filter(|line| {
        let line = line.trim_start();
        line.strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line)
            .starts_with(&declaration)
    });
    let line = declarations
        .next()
        .unwrap_or_else(|| panic!("canonical constant {constant_name} is absent"));
    assert!(
        declarations.next().is_none(),
        "canonical constant {constant_name} is declared more than once"
    );
    let declaration_start = line
        .find(&declaration)
        .expect("the declaration is on its own line");
    let declaration_tail = &line[declaration_start + declaration.len()..];
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
        "QUIETMAP_SCREENING_MIN_PATH_M",
        canonical_f64(PATH_EFFECTS_SOURCE, "SCREENING_MIN_PATH_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_SOURCE_HEIGHT_FLOOR_M",
        canonical_f64(PATH_EFFECTS_SOURCE, "SOURCE_HEIGHT_FLOOR_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_RECEIVER_HEIGHT_FLOOR_M",
        canonical_f64(PATH_EFFECTS_SOURCE, "RECEIVER_HEIGHT_FLOOR_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_PATH_HEIGHT_FLOOR_M",
        canonical_f64(ISO9613_SOURCE, "GROUND_PATH_HEIGHT_FLOOR_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_SHORT_PATH_FACTOR",
        canonical_f64(ISO9613_SOURCE, "CNOSSOS_GROUND_SHORT_PATH_FACTOR"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_FAVOURABLE_ALPHA0",
        canonical_f64(ISO9613_SOURCE, "CNOSSOS_GROUND_ALPHA0"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_FAVOURABLE_DELTA_ZT",
        canonical_f64(ISO9613_SOURCE, "CNOSSOS_GROUND_DELTA_ZT_COEFF"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_FINITE_LINE_MIN_PERPENDICULAR_M",
        canonical_f64(GEO_SOURCE, "FLC_MIN_PERP_M"),
    );
    writeln!(
        header,
        "constexpr int QUIETMAP_TILE_PIXEL_SIDE = {};",
        canonical_usize(FUSED_TILE_SOURCE, "TILE_PX")
    )
    .unwrap();
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
    write_cuda_float(
        &mut header,
        "QUIETMAP_EXACT_CADENCE_MAX_DISTANCE_M",
        canonical_f64(SCATTER_BAND_SOURCE, "EXACT_CADENCE_MAX_DIST_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_COARSE_MIDDLE_SOURCE_ZONE_M",
        canonical_f64(SCATTER_BAND_SOURCE, "SHADOW_SRC_ZONE_M"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_COARSE_MIDDLE_RECEIVER_ZONE_M",
        canonical_f64(SCATTER_BAND_SOURCE, "SHADOW_RX_ZONE_M"),
    );
    writeln!(
        header,
        "constexpr int QUIETMAP_COARSE_MIDDLE_STRIDE = {};",
        canonical_usize(SCATTER_BAND_SOURCE, "SHADOW_MID_STRIDE")
    )
    .unwrap();
    // Spelled `<deg>_f64.to_radians()` in seg_sampling.rs; mirror the expression.
    let arc_gate_degrees: f64 =
        constant_initializer(SEGMENT_SAMPLING_SOURCE, "SEG_ARC_MIN_SPAN_RAD")
            .strip_suffix("_f64.to_radians()")
            .expect("SEG_ARC_MIN_SPAN_RAD keeps its `<deg>_f64.to_radians()` spelling")
            .parse()
            .expect("SEG_ARC_MIN_SPAN_RAD degree literal parses");
    write_cuda_float(
        &mut header,
        "QUIETMAP_SEG_ARC_MIN_SPAN_RAD",
        arc_gate_degrees.to_radians(),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_ARC_DEGENERATE_SPAN_RAD",
        canonical_f64(ARC_SCREENING_SOURCE, "DEGENERATE_SPAN_RAD"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_ARC_ESCALATE_SPAN_RAD",
        canonical_f64(ARC_SCREENING_SOURCE, "ESCALATE_SPAN_RAD"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_ARC_CP_AZIMUTH_EPS",
        canonical_f64(ARC_SCREENING_SOURCE, "CP_AZIMUTH_EPS"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_ARC_QUADRATURE_MIN_RAD",
        canonical_f64(ARC_SCREENING_SOURCE, "ARC_QUADRATURE_MIN_RAD"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_FREE_FIELD_ATMOSPHERE_DB_PER_M",
        canonical_f64(GEO_SOURCE, "ATM_ALPHA_A_WEIGHTED"),
    );
    write_cuda_float(
        &mut header,
        "QUIETMAP_GROUND_OPS_REFERENCE_OFFSET_M",
        canonical_f64(GROUND_OPS_SOURCE, "GROUND_OPS_REF_OFFSET_M"),
    );
    write_cuda_array(
        &mut header,
        "QUIETMAP_GROUND_BAND_MEAN_CF",
        canonical_f64_array::<8>(NOISE_CONSTANTS_SOURCE, "GROUND_CF"),
    );
    writeln!(
        header,
        "constexpr int QUIETMAP_ARC_ESCALATE_MAX_PARTS = {};",
        canonical_usize(ARC_SCREENING_SOURCE, "ESCALATE_MAX_PARTS")
    )
    .unwrap();
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

const NVCC_ARGUMENTS: [&str; 8] = [
    "-std=c++17",
    "-O3",
    "--use_fast_math",
    "-lineinfo",
    "-arch=sm_120",
    "-maxrregcount=40",
    "-Xcompiler",
    "-fPIC",
];

/// Owner ruling: no f64 in a production kernel (GeForce Blackwell runs it at 1/64).
/// The PTX of the same translation unit is scanned for any `.f64` opcode; one
/// promoted literal or libm call re-promotes a whole per-pair chain, so the build fails.
fn assert_ptx_has_no_f64(output_directory: &std::path::Path) {
    let ptx_path = output_directory.join("block_source_partition.ptx");
    run_checked(
        Command::new("nvcc")
            .args(NVCC_ARGUMENTS)
            .arg("-I")
            .arg(output_directory)
            .args(["-ptx", "kernels/block_source_partition.cu", "-o"])
            .arg(&ptx_path),
        "nvcc relevant-source PTX for the f64 gate",
    );
    let ptx = fs::read_to_string(&ptx_path).expect("read the relevant-source PTX");
    let f64_opcodes = ptx.lines().filter(|line| line.contains(".f64")).count();
    assert_eq!(
        f64_opcodes, 0,
        "relevant-source kernels use f64 in {f64_opcodes} PTX lines; production kernels stay f32"
    );
}

fn main() {
    println!("cargo:rerun-if-changed=kernels/relevant_source_geometry.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_path.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_attenuation.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_grid_scan.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_obstacles.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_arc.cuh");
    println!("cargo:rerun-if-changed=kernels/relevant_source_pair.cuh");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/arc_screening.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/geo.rs");
    println!("cargo:rerun-if-changed=../tile-painter/src/ground_ops.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/path_effects.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/iso9613.rs");
    println!("cargo:rerun-if-changed=../raster-reader/src/fused_tile_z13.rs");
    println!("cargo:rerun-if-changed=kernels/block_source_partition.cu");
    println!("cargo:rerun-if-changed=../noise-compute/src/constants.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/path_profile.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/propagation/seg_sampling.rs");
    println!("cargo:rerun-if-changed=src/source_frame.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/types/inputs.rs");
    println!("cargo:rerun-if-changed=../tile-painter/src/scatter_band.rs");
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
            .args(NVCC_ARGUMENTS)
            .arg("-I")
            .arg(&output_directory)
            .args(["-c", "kernels/block_source_partition.cu", "-o"])
            .arg(&object_path),
        "nvcc relevant-source compilation",
    );
    assert_ptx_has_no_f64(&output_directory);
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
        assert!(header.contains("constexpr int QUIETMAP_COARSE_MIDDLE_STRIDE = 3;"));
        assert!(header.contains("constexpr int QUIETMAP_ARC_ESCALATE_MAX_PARTS = 9;"));
        assert!(header.contains("constexpr int QUIETMAP_TILE_PIXEL_SIDE = 512;"));
        assert!(header.contains("constexpr float QUIETMAP_RECEIVER_HEIGHT_FLOOR_M = 0.5f;"));
        assert!(header.contains("constexpr float QUIETMAP_SEG_ARC_MIN_SPAN_RAD = 0.05235988f;"));
        assert!(header.contains("constexpr float QUIETMAP_PENUMBRA_DELTA_FLOOR_M ="));
    }
}
