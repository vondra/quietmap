//! Build script for `noise-gpu` — compile the airborne CUDA kernels into one fatbin.
//!
//! CUDA is needed only for the `gpu` feature. The default build keeps the
//! shared Rust helpers available on hosts without a CUDA toolkit.

#[path = "../cuda_archs.rs"]
mod cuda_archs;

use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=NOISE_GPU_ARCH");
    println!("cargo:rerun-if-changed=../cuda_archs.rs");
    // Watch the directory, not only the files currently in it: adding an
    // airborne kernel must rebuild its device image.
    println!("cargo:rerun-if-changed=kernels");
    if env::var_os("CARGO_FEATURE_GPU").is_none() {
        return;
    }

    let out = PathBuf::from(env::var("OUT_DIR").expect("Cargo always sets OUT_DIR"));
    let archs = cuda_archs::cuda_archs();
    let num_classes = const_from(
        "../noise-compute/src/emission/profiles_generated.rs",
        "pub const NUM_CLASSES: usize = ",
    );
    let tile_px = const_from(
        "../raster-reader/src/fused_tile_z13.rs",
        "pub const TILE_PX: usize = ",
    );
    let metres_per_degree_latitude = numeric_f64_const(
        "../noise-compute/src/constants.rs",
        "pub const M_PER_DEG_LAT: f64 = ",
    );
    let aircraft_metres_per_degree_latitude = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/doc29.rs",
        "pub const M_PER_DEG_LAT: f64 = ",
    );

    let terrain_sectors = const_from(
        "../noise-compute/src/emission/aircraft/horizon.rs",
        "pub const HORIZON_SECTORS: usize = ",
    );
    let terrain_bands = const_from(
        "../noise-compute/src/emission/aircraft/horizon.rs",
        "pub const RECEIVER_HORIZON_BANDS: usize = ",
    );
    let terrain_march_samples = const_from(
        "../noise-compute/src/emission/aircraft/horizon.rs",
        "pub const RECEIVER_HORIZON_MARCH_SAMPLES: usize = ",
    );
    let building_local_sectors = const_from(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_LOCAL_HORIZON_SECTORS: usize = ",
    );
    let building_local_bands = const_from(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_LOCAL_HORIZON_BANDS: usize = ",
    );
    let horizon_tangent_scale = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/horizon.rs",
        "pub const RECEIVER_HORIZON_TANGENT_SCALE: f64 = ",
    );
    let terrain_range_scale = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/horizon.rs",
        "pub const RECEIVER_HORIZON_RANGE_SCALE: f64 = ",
    );
    let building_range_scale = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_HORIZON_RANGE_SCALE: f64 = ",
    );
    let building_local_max_m = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_LOCAL_MAX_M: f64 = ",
    );
    let building_range_growth = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_LOCAL_RANGE_GROWTH: f64 = ",
    );
    let building_first_range_break_m = {
        let max_m = building_local_max_m
            .parse::<f64>()
            .expect("BUILDING_LOCAL_MAX_M parses as f64");
        let growth = building_range_growth
            .parse::<f64>()
            .expect("BUILDING_LOCAL_RANGE_GROWTH parses as f64");
        let bands = building_local_bands
            .parse::<i32>()
            .expect("BUILDING_LOCAL_HORIZON_BANDS parses as i32");
        c_f64(max_m / growth.powi(bands - 1))
    };
    let building_min_edge_range_m = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub const BUILDING_MIN_EDGE_RANGE_M: f64 = ",
    );
    let lowest_source_tangent_block_px = const_from(
        "../tile-painter/src/airborne_screening.rs",
        "pub const LOWEST_SOURCE_TANGENT_BLOCK_PX: usize = ",
    );
    let lowest_source_tangent_groups = const_from(
        "../noise-compute/src/emission/aircraft/screening_bounds.rs",
        "pub const LOWEST_SOURCE_TANGENT_SECTOR_GROUPS: usize = ",
    );
    let lowest_source_tangent_margin_rel = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening_bounds.rs",
        "pub const LOWEST_SOURCE_TANGENT_MARGIN_REL: f64 = ",
    );
    let lowest_source_tangent_margin_abs = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening_bounds.rs",
        "pub const LOWEST_SOURCE_TANGENT_MARGIN_ABS: f64 = ",
    );
    let lowest_source_tangent_range_margin_m = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening_bounds.rs",
        "pub const LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M: f64 = ",
    );
    let lowest_source_tangent_angle_margin_rad = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening_bounds.rs",
        "pub const LOWEST_SOURCE_TANGENT_ANGLE_MARGIN_RAD: f64 = ",
    );
    let diffraction_slope = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "const DIFFRACTION_SLOPE_PER_M: f64 = ",
    );
    let diffraction_grazing_db = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "const DIFFRACTION_GRAZING_DB: f64 = ",
    );
    let diffraction_cap_db = numeric_f64_const(
        "../noise-compute/src/emission/aircraft/screening.rs",
        "pub(super) const DIFFRACTION_CAP_DB: f64 = ",
    );

    let screening_slots = [
        ("SCREEN_RECORDS", "const SCREEN_RECORDS: usize = "),
        ("SCREEN_NREG", "const SCREEN_NREG: usize = "),
        ("SCREEN_NEAR_BASE", "const SCREEN_NEAR_BASE: usize = "),
        ("SCREEN_NEAR_COUNT", "const SCREEN_NEAR_COUNT: usize = "),
        ("SCREEN_FAR0_BASE", "const SCREEN_FAR0_BASE: usize = "),
        ("SCREEN_FAR0_COUNT", "const SCREEN_FAR0_COUNT: usize = "),
        ("SCREEN_FAR1_BASE", "const SCREEN_FAR1_BASE: usize = "),
        ("SCREEN_FAR1_COUNT", "const SCREEN_FAR1_COUNT: usize = "),
        ("SCREEN_FAR2_BASE", "const SCREEN_FAR2_BASE: usize = "),
        ("SCREEN_FAR2_COUNT", "const SCREEN_FAR2_COUNT: usize = "),
        (
            "SCREEN_RECORD_OF_PIXEL",
            "const SCREEN_RECORD_OF_PIXEL: usize = ",
        ),
        (
            "SCREEN_TERRAIN_ENTRIES",
            "const SCREEN_TERRAIN_ENTRIES: usize = ",
        ),
        (
            "SCREEN_TERRAIN_MAX_SIN_SQ",
            "const SCREEN_TERRAIN_MAX_SIN_SQ: usize = ",
        ),
        (
            "SCREEN_BUILDING_GLOBAL_MAX_TAN_Q",
            "const SCREEN_BUILDING_GLOBAL_MAX_TAN_Q: usize = ",
        ),
        (
            "SCREEN_BUILDING_LOCAL_ENTRIES",
            "const SCREEN_BUILDING_LOCAL_ENTRIES: usize = ",
        ),
        (
            "SCREEN_BUILDING_LOCAL_MAX_TAN_Q",
            "const SCREEN_BUILDING_LOCAL_MAX_TAN_Q: usize = ",
        ),
    ]
    .map(|(name, prefix)| (name, const_from("src/airborne.rs", prefix)));
    let building_environment_slots = [
        (
            "BUILDING_ENV_INDEX_COUNT",
            "const BUILDING_ENV_INDEX_COUNT: usize = ",
        ),
        (
            "BUILDING_ENV_GRID_GEOMETRY",
            "const BUILDING_ENV_GRID_GEOMETRY: usize = ",
        ),
        (
            "BUILDING_ENV_GRID_LAYOUT",
            "const BUILDING_ENV_GRID_LAYOUT: usize = ",
        ),
        (
            "BUILDING_ENV_CELL_STARTS",
            "const BUILDING_ENV_CELL_STARTS: usize = ",
        ),
        (
            "BUILDING_ENV_EDGE_REFS",
            "const BUILDING_ENV_EDGE_REFS: usize = ",
        ),
        ("BUILDING_ENV_EDGES", "const BUILDING_ENV_EDGES: usize = "),
        (
            "BUILDING_ENV_EDGE_IS_BUILDING",
            "const BUILDING_ENV_EDGE_IS_BUILDING: usize = ",
        ),
        (
            "BUILDING_ENV_DEM_META",
            "const BUILDING_ENV_DEM_META: usize = ",
        ),
        (
            "BUILDING_ENV_DEM_ELEVATION",
            "const BUILDING_ENV_DEM_ELEVATION: usize = ",
        ),
        (
            "BUILDING_ENV_DEM_COLS",
            "const BUILDING_ENV_DEM_COLS: usize = ",
        ),
        (
            "BUILDING_ENV_DEM_ROWS",
            "const BUILDING_ENV_DEM_ROWS: usize = ",
        ),
        (
            "BUILDING_ENV_DIRECTIONS",
            "const BUILDING_ENV_DIRECTIONS: usize = ",
        ),
        (
            "BUILDING_ENV_TERRAIN_SAMPLES",
            "const BUILDING_ENV_TERRAIN_SAMPLES: usize = ",
        ),
        (
            "BUILDING_ENV_CELL_MAX_H",
            "const BUILDING_ENV_CELL_MAX_H: usize = ",
        ),
    ]
    .map(|(name, prefix)| (name, const_from("src/airborne_building_horizon.rs", prefix)));
    let coarse_target_blocks =
        const_from("src/airborne.rs", "const COARSE_TARGET_BLOCKS: usize = ");
    let building_grid_geometry_stride = const_from(
        "src/airborne_building_horizon.rs",
        "const BUILDING_GRID_GEOMETRY_STRIDE: usize = ",
    );
    let building_grid_layout_stride = const_from(
        "src/airborne_building_horizon.rs",
        "const BUILDING_GRID_LAYOUT_STRIDE: usize = ",
    );

    let mut nvcc_defines = vec![
        format!("-DNPD_NC={num_classes}"),
        format!("-DTPX={tile_px}"),
        format!("-DM_LAT={metres_per_degree_latitude}"),
        format!("-DAIRCRAFT_M_LAT={aircraft_metres_per_degree_latitude}"),
        format!("-DTERRAIN_SECTORS={terrain_sectors}"),
        format!("-DTERRAIN_BANDS={terrain_bands}"),
        format!("-DTERRAIN_MARCH_SAMPLES={terrain_march_samples}"),
        format!("-DBUILDING_LOCAL_SECTORS={building_local_sectors}"),
        format!("-DBUILDING_LOCAL_BANDS={building_local_bands}"),
        format!("-DTAN_SCALE_D={horizon_tangent_scale}"),
        format!("-DTERRAIN_RANGE_SCALE_D={terrain_range_scale}"),
        format!("-DBUILDING_RANGE_SCALE_D={building_range_scale}"),
        format!("-DBUILDING_LOCAL_MAX_M_D={building_local_max_m}"),
        format!("-DBUILDING_FIRST_RANGE_BREAK_M_D={building_first_range_break_m}"),
        format!("-DBUILDING_RANGE_GROWTH_D={building_range_growth}"),
        format!("-DBUILDING_MIN_EDGE_RANGE_M_D={building_min_edge_range_m}"),
        format!("-DCOARSE_TARGET_BLOCKS={coarse_target_blocks}"),
        format!("-DBUILDING_GRID_GEOMETRY_STRIDE={building_grid_geometry_stride}"),
        format!("-DBUILDING_GRID_LAYOUT_STRIDE={building_grid_layout_stride}"),
        format!("-DLOWEST_SOURCE_TANGENT_BLOCK_PX={lowest_source_tangent_block_px}"),
        format!("-DLOWEST_SOURCE_TANGENT_GROUPS={lowest_source_tangent_groups}"),
        format!("-DLOWEST_SOURCE_TANGENT_MARGIN_REL_D={lowest_source_tangent_margin_rel}"),
        format!("-DLOWEST_SOURCE_TANGENT_MARGIN_ABS_D={lowest_source_tangent_margin_abs}"),
        format!("-DLOWEST_SOURCE_TANGENT_RANGE_MARGIN_M_D={lowest_source_tangent_range_margin_m}"),
        format!(
            "-DLOWEST_SOURCE_TANGENT_ANGLE_MARGIN_RAD_D={lowest_source_tangent_angle_margin_rad}"
        ),
        format!("-DDIFFRACTION_SLOPE_D={diffraction_slope}"),
        format!("-DDIFFRACTION_GRAZING_DB_D={diffraction_grazing_db}"),
        format!("-DDIFFRACTION_CAP_DB_D={diffraction_cap_db}"),
    ];
    nvcc_defines.extend(
        screening_slots
            .into_iter()
            .map(|(name, value)| format!("-D{name}={value}")),
    );
    nvcc_defines.extend(
        building_environment_slots
            .into_iter()
            .map(|(name, value)| format!("-D{name}={value}")),
    );
    fs::write(
        out.join("nvcc-defines.txt"),
        format!("{}\n", nvcc_defines.join("\n")),
    )
    .expect("write exact nvcc define receipt");

    for entry in fs::read_dir("kernels").expect("kernels/ directory") {
        let path = entry.expect("kernel directory entry").path();
        if path.extension().is_some_and(|extension| extension == "cu") {
            let stem = path
                .file_stem()
                .expect("kernel file stem")
                .to_string_lossy();
            println!("cargo:rerun-if-changed={}", path.display());
            // The fatbin is what the binary loads: SASS for every architecture of this
            // build plus the PTX of the lowest one, so a card the fleet list does not
            // name still JITs (PTX is forward compatible, cubins are not). The card we
            // do rent runs its ahead-of-time image and never JIT-compiles; a driver
            // older than the toolkit cannot JIT this PTX at all
            // (CUDA_ERROR_UNSUPPORTED_PTX_VERSION), which is exactly the
            // minor-version-compatible case the SASS covers.
            //
            // The separate `.ptx` is the model-role receipt of a single-architecture
            // build (scripts/gpu_model_role.py proves its bytes are embedded), which is
            // why the fatbin stays uncompressed.
            let jit = cuda_archs::compute_arch(&archs[0]);
            let mut fatbin_arguments = vec![
                "-fatbin".to_owned(),
                "--compress-mode=none".to_owned(),
                "-O3".to_owned(),
                "-gencode".to_owned(),
                format!("arch={jit},code=[{},{jit}]", archs[0]),
            ];
            for arch in &archs[1..] {
                fatbin_arguments.push("-gencode".to_owned());
                fatbin_arguments.push(format!(
                    "arch={},code={arch}",
                    cuda_archs::compute_arch(arch)
                ));
            }
            let ptx_arguments = vec![
                "-ptx".to_owned(),
                "--compress-mode=none".to_owned(),
                "-O3".to_owned(),
                format!("-arch={}", archs[0]),
            ];
            for (arguments, output) in [
                (ptx_arguments, out.join(format!("{stem}.ptx"))),
                (fatbin_arguments, out.join(format!("{stem}.fatbin"))),
            ] {
                let status = Command::new("nvcc")
                    .args(&arguments)
                    .args(&nvcc_defines)
                    .arg(&path)
                    .arg("-o")
                    .arg(&output)
                    .status()
                    .expect("nvcc not found — `--features gpu` needs the CUDA toolkit");
                assert!(
                    status.success(),
                    "nvcc {} failed to compile {path:?}",
                    arguments[0]
                );
            }
        }
    }
}

/// One Rust `const <name>: usize = N;` line, parsed for injection into nvcc.
fn const_from(path: &str, prefix: &str) -> String {
    println!("cargo:rerun-if-changed={path}");
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path} not readable for constant injection: {error}"))
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix(prefix)?
                .strip_suffix(';')
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("`{prefix}` not found in {path}"))
}

fn c_f64(value: f64) -> String {
    assert!(value.is_finite(), "generated CUDA constant must be finite");
    format!("{value:.17e}")
}

fn numeric_f64_const(path: &str, prefix: &str) -> String {
    let source = const_from(path, prefix);
    let value = source
        .replace('_', "")
        .parse::<f64>()
        .unwrap_or_else(|error| panic!("`{prefix}` in {path} is not numeric: {error}"));
    c_f64(value)
}
