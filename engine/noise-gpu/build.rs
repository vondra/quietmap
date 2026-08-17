//! Build script for `noise-gpu` — compiles CUDA kernels to PTX (only under the
//! `gpu` feature) so a CUDA-less host still builds the CPU-side lib cleanly.
// Compile every kernels/*.cu to its own PTX (kernels/foo.cu -> $OUT_DIR/foo.ptx)
// at build time via nvcc. This is the production path (vs runtime nvrtc): the PTX
// is embedded in the binary and JIT-finalised by the driver at load, so one build
// runs on any SM >= the arch. nvcc is isolated from Cargo's rustflags, so
// target-cpu=native parity is untouched. NOISE_GPU_ARCH overrides sm_89 (4060).
//
// Only the `gpu` feature (the gpu-surface/e2-full bins) needs CUDA. Without it the
// crate is the CPU-side lib alone, so skip nvcc entirely — a host with no CUDA
// toolkit (e.g. a CPU-only box) then builds noise-gpu cleanly. nvcc is required only when you
// explicitly build `--features gpu`, which only happens on a GPU host.
mod build_defines;

use build_defines::parse_experimental_defines;
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=NOISE_GPU_ARCH");
    println!("cargo:rerun-if-env-changed=NOISE_GPU_DEFINES");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0_DIAGNOSTIC");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0_COUNTERS");
    println!("cargo:rerun-if-changed=build_defines.rs");
    let extra_defines =
        parse_experimental_defines(&env::var("NOISE_GPU_DEFINES").unwrap_or_default())
            .unwrap_or_else(|error| panic!("invalid NOISE_GPU_DEFINES: {error}"));
    // Watch the whole dir, not just each .cu — otherwise ADDING a new kernel
    // (e.g. airborne.cu) doesn't re-run this script, so its .ptx never builds.
    println!("cargo:rerun-if-changed=kernels");
    if env::var_os("CARGO_FEATURE_GPU").is_none() {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = env::var("NOISE_GPU_ARCH").unwrap_or_else(|_| "sm_89".into());
    // NUM_CLASSES is parsed from the generated profiles table and injected as
    // -DNPD_NC so the kernel's NPD LUT stride can never drift from the Rust
    // upload (hardcoded 14 corrupted departures when the pinned 15th class
    // landed; /gg C10b 2026-06-11).
    let gen = "../noise-compute/src/emission/profiles_generated.rs";
    println!("cargo:rerun-if-changed={gen}");
    let num_classes = fs::read_to_string(gen)
        .expect("profiles_generated.rs not found next to noise-gpu")
        .lines()
        .find_map(|l| {
            l.strip_prefix("pub const NUM_CLASSES: usize = ")?
                .strip_suffix(';')
                .map(str::to_owned)
        })
        .expect("NUM_CLASSES const not found in profiles_generated.rs");
    // Same hazard, same cure, for the geometry constants: the kernel indexes its
    // output with TPX² while the HOST sizes that buffer from the Rust TILE_PX, and
    // nothing enforced the equality — a TILE_PX change would have produced silent
    // out-of-bounds device writes and corrupted tiles, not a compile error
    // (2026-08-04 audit). BIN_W is the same hazard for launch geometry.
    // NOISE_GPU_DEFINES is a parsed list of reviewed DEV-INSTRUMENT switches,
    // not a compiler-option passthrough. Every switch defaults to production,
    // so an empty value builds the shipped kernel. Without the explicit levers
    // an A/B can silently measure the default build twice; without the allowlist
    // a later -D can silently replace the host-owned constants injected below.
    let v2_h0 = usize::from(env::var_os("CARGO_FEATURE_V2_H0").is_some());
    let h0_pair_diagnostic = usize::from(env::var_os("CARGO_FEATURE_V2_H0_DIAGNOSTIC").is_some());
    let h0_counters = usize::from(env::var_os("CARGO_FEATURE_V2_H0_COUNTERS").is_some());
    assert!(
        h0_pair_diagnostic == 0 || v2_h0 == 1,
        "the H0 pair diagnostic must compile on the H0 device mirror"
    );
    assert!(
        h0_counters == 0 || v2_h0 == 1,
        "the H0 counter role must compile on the H0 device mirror"
    );
    let tile_px = const_from(
        "../raster-reader/src/fused_tile_z13.rs",
        "pub const TILE_PX: usize = ",
    );
    let bin_w = const_from("src/lib.rs", "pub const BIN_W: usize = ");
    let barrier_abi_version = const_from("src/lib.rs", "pub const BARRIER_ABI_VERSION: usize = ");
    let barrier_stride = const_from("src/lib.rs", "pub const BARRIER_STRIDE: usize = ");
    let source_segment_abi_version = const_from(
        "src/lib.rs",
        "pub const SOURCE_SEGMENT_ABI_VERSION: usize = ",
    );
    let source_segment_stride =
        const_from("src/lib.rs", "pub const SOURCE_SEGMENT_STRIDE: usize = ");
    let line_kernel_argument_count = const_from(
        "src/lib.rs",
        "pub const LINE_KERNEL_ARGUMENT_COUNT: usize = ",
    );
    let surface_meta_abi_version =
        const_from("src/lib.rs", "pub const SURFACE_META_ABI_VERSION: usize = ");
    let surface_meta_slots = const_from("src/lib.rs", "pub const SURFACE_META_SLOTS: usize = ");
    let out_h0_counter_byte_offset = const_from(
        "src/lib.rs",
        "pub const OUT_H0_COUNTER_BYTE_OFFSET: usize = ",
    )
    .replace('_', "");
    let out_h0_counters = const_from("src/lib.rs", "pub const OUT_H0_COUNTERS: usize = ");
    let out_slots_h0 =
        const_from("src/lib.rs", "pub const OUT_SLOTS_H0: usize = ").replace('_', "");
    let h0_pair_diagnostic_abi_version = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_ABI_VERSION: usize = ",
    );
    let h0_pair_diagnostic_header_words = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_HEADER_WORDS: usize = ",
    );
    let h0_pair_diagnostic_node_words = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_NODE_WORDS: usize = ",
    );
    let h0_pair_diagnostic_record_words = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_RECORD_WORDS: usize = ",
    );
    let h0_pair_diagnostic_node_base = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_NODE_BASE: usize = ",
    );
    let h0_pair_diagnostic_record_base = const_from(
        "src/lib.rs",
        "pub const H0_PAIR_DIAGNOSTIC_RECORD_BASE: usize = ",
    );
    let h0_pair_diagnostic_magic =
        numeric_u64_const("src/lib.rs", "pub const H0_PAIR_DIAGNOSTIC_MAGIC: u64 = ");
    let h0_node_cap = const_from(
        "../noise-compute/src/compute/element.rs",
        "pub const H0_NODE_CAP: usize = ",
    );
    let theta_max_expr = const_from(
        "../noise-compute/src/compute/element.rs",
        "pub const THETA_MAX_RAD: f64 = ",
    );
    assert_eq!(
        theta_max_expr, "core::f64::consts::PI / 60.0",
        "THETA_MAX_RAD changed: extend the reviewed Rust-to-CUDA constant generator"
    );
    let theta_max_rad = c_f64(core::f64::consts::PI / 60.0);
    let road_d_floor_m = numeric_f64_const(
        "../noise-compute/src/compute/element.rs",
        "pub const ROAD_D_FLOOR_M: f64 = ",
    );
    let rail_d_floor_m = numeric_f64_const(
        "../noise-compute/src/compute/element.rs",
        "pub const RAIL_D_FLOOR_M: f64 = ",
    );
    let r_atm_base_m_per_rad = numeric_f64_const(
        "../noise-compute/src/compute/element.rs",
        "pub const R_ATM_BASE_M_PER_RAD: f64 = ",
    );
    let a_live_db = numeric_f64_const(
        "../noise-compute/src/compute/element.rs",
        "pub const A_LIVE_DB: f64 = ",
    );
    let line_max_length_m = numeric_f64_const(
        "../noise-compute/src/compute/element.rs",
        "pub const LINE_MAX_LENGTH_M: f64 = ",
    );
    let alpha_atm = numeric_f64_array_const(
        "../noise-compute/src/constants.rs",
        "pub const ALPHA_ATM: [f64; NUM_BANDS] = ",
        8,
    );
    let metres_per_degree_latitude = numeric_f64_const(
        "../noise-compute/src/constants.rs",
        "pub const M_PER_DEG_LAT: f64 = ",
    );
    let alpha_atm_defines = alpha_atm
        .iter()
        .enumerate()
        .map(|(index, value)| format!("#define V2_ALPHA_ATM_{index} {value}\n"))
        .collect::<String>();
    fs::write(
        out.join("qm_streaming_abi_generated.h"),
        format!(
            "#define BARRIER_ABI_VERSION {barrier_abi_version}\n\
             #define BARRIER_STRIDE {barrier_stride}\n\
             #define SOURCE_SEGMENT_ABI_VERSION {source_segment_abi_version}\n\
             #define SOURCE_SEGMENT_STRIDE {source_segment_stride}\n\
             #define LINE_KERNEL_ARGUMENT_COUNT {line_kernel_argument_count}\n\
             #define SURFACE_META_ABI_VERSION {surface_meta_abi_version}\n\
             #define SURFACE_META_SLOTS {surface_meta_slots}\n\
             #define V2_H0 {v2_h0}\n\
             #define OUT_H0_COUNTER_BYTE_OFFSET {out_h0_counter_byte_offset}\n\
             #define OUT_H0_COUNTERS {out_h0_counters}\n\
             #define OUT_SLOTS_H0 {out_slots_h0}\n\
             #define PROF_H0_COUNTERS {h0_counters}\n\
             #define PROF_H0_PAIR_DIAGNOSTIC {h0_pair_diagnostic}\n\
             #define H0_PAIR_DIAGNOSTIC_ABI_VERSION {h0_pair_diagnostic_abi_version}\n\
             #define H0_PAIR_DIAGNOSTIC_HEADER_WORDS {h0_pair_diagnostic_header_words}\n\
             #define H0_PAIR_DIAGNOSTIC_NODE_WORDS {h0_pair_diagnostic_node_words}\n\
             #define H0_PAIR_DIAGNOSTIC_RECORD_WORDS {h0_pair_diagnostic_record_words}\n\
             #define H0_PAIR_DIAGNOSTIC_NODE_BASE {h0_pair_diagnostic_node_base}\n\
             #define H0_PAIR_DIAGNOSTIC_RECORD_BASE {h0_pair_diagnostic_record_base}\n\
             #define H0_PAIR_DIAGNOSTIC_MAGIC {h0_pair_diagnostic_magic}ull\n\
             #define V2_H0_NODE_CAP {h0_node_cap}\n\
             #define V2_THETA_MAX_RAD {theta_max_rad}\n\
             #define V2_ROAD_D_FLOOR_M {road_d_floor_m}\n\
             #define V2_RAIL_D_FLOOR_M {rail_d_floor_m}\n\
             #define V2_R_ATM_BASE_M_PER_RAD {r_atm_base_m_per_rad}\n\
             #define V2_A_LIVE_DB {a_live_db}\n\
             #define V2_LINE_MAX_LENGTH_M {line_max_length_m}\n\
             #define M_LAT {metres_per_degree_latitude}\n\
             {alpha_atm_defines}"
        ),
    )
    .expect("write generated streaming ABI header");
    // Same contract for the two constants the footprint-CSR arc walk added: a
    // drifted stride walks a neighbouring index's grid or mis-reads foot_box.
    let meta_stride = const_from("src/lib.rs", "pub const META_STRIDE: usize = ");
    let foot_box_stride = const_from("src/lib.rs", "pub const FOOT_BOX_STRIDE: usize = ");
    // The arc span floor is a PHYSICS constant, and a physics constant that is a
    // default on one lane and a hand-copied number on the other has now forked
    // the lanes TWICE (2026-08-04: the kernel sat at 0.01 rad for two hours after
    // the CPU sweep turned the gate off, worth up to 11.4 dB on dense geometry).
    // Injecting it makes the kernel mirror the Rust EXPRESSION instead of a copy
    // of its value, so editing one side can no longer move the lanes apart.
    let degenerate_span = const_from(
        "../noise-compute/src/propagation/arc_screening.rs",
        "const DEGENERATE_SPAN_RAD: f64 = ",
    );
    let cp_eps = const_from(
        "../noise-compute/src/propagation/arc_screening.rs",
        "const CP_AZIMUTH_EPS: f64 = ",
    );
    let arc_quadrature_min = const_from(
        "../noise-compute/src/propagation/arc_screening.rs",
        "const ARC_QUADRATURE_MIN_RAD: f64 = ",
    );
    // And the CNOSSOS hard-ground floor. `A_ground` is ONE formula living in
    // `iso9613::ground_atten_db`; CUDA cannot call it, so the kernel mirrors the
    // EXPRESSION and takes the only number in it from the Rust const. Hand-copying
    // it is how the term went missing from nine call sites at once — a lane that
    // disagrees here is 3 dB out over every hard-ground path in the world.
    let ground_hard_floor = const_from(
        "../noise-compute/src/constants.rs",
        "pub const GROUND_HARD_FLOOR_DB: f64 = ",
    );
    let ground_sound_speed = const_from(
        "../noise-compute/src/constants.rs",
        "pub const SPEED_OF_SOUND: f64 = ",
    );
    let ground_alpha0 = const_from(
        "../noise-compute/src/propagation/iso9613.rs",
        "pub const CNOSSOS_GROUND_ALPHA0: f64 = ",
    );
    let ground_delta_zt = const_from(
        "../noise-compute/src/propagation/iso9613.rs",
        "pub const CNOSSOS_GROUND_DELTA_ZT_COEFF: f64 = ",
    );
    let p_fav = const_from(
        "../noise-compute/src/constants.rs",
        "pub const P_FAV: f64 = ",
    );
    // The penumbra δ floor, λ/20 at the LOWEST band — how far below the sight
    // line an obstacle may pass and still attenuate. `constants.rs` writes it as
    // an EXPRESSION over SPEED_OF_SOUND, so mirror the expression here (as
    // `fuse_ratio_ln` does below) instead of hand-copying its value: the formula
    // `sos/63/20` had FIVE hand copies across two languages (constants.rs,
    // arc_screening.rs, and twice in scatter.cu), which is exactly how the ground
    // term went missing from nine call sites. The kernel gets the MAGNITUDE, and
    // derives its signed `ARC_DELTA_REJECT` from it.
    let penumbra_floor = {
        let sos: f64 = const_from(
            "../noise-compute/src/constants.rs",
            "pub const SPEED_OF_SOUND: f64 = ",
        )
        .parse()
        .expect("SPEED_OF_SOUND parses as f64");
        format!("{:e}", sos / 63.0 / 20.0)
    };
    // The skyline's height-stratum width, `arc_screening::ARC_FUSE_HEIGHT_TOL_M`.
    // `arc_fuse_key` floors a height BY it, so a drift here re-partitions which
    // arcs fuse — a decision, not a value, and one the lanes must make alike.
    // Emitted with the `f` suffix ON PURPOSE: the Rust key divides in f32, and a
    // bare `3.0` would make the kernel divide in double and round twice.
    let fuse_height_tol: String = {
        let v: f32 = const_from(
            "../noise-compute/src/propagation/arc_screening.rs",
            "const ARC_FUSE_HEIGHT_TOL_M: f32 = ",
        )
        .parse()
        .expect("ARC_FUSE_HEIGHT_TOL_M parses as f32");
        format!("{v:e}f")
    };
    // The range-stratum DIVISOR, `ln(ARC_FUSE_RANGE_RATIO)` — computed here, in
    // Rust, rather than left to the device's own `logf`.
    //
    // `arc_fuse_key` decides `floor(ln(near)/ln(ratio))`, and a DECISION cannot
    // absorb a rounding difference the way a value can: one ulp on either operand
    // moves an arc into the neighbouring range stratum, where it fuses with a
    // different set of arcs. MEASURED exhaustively on the 5070 over every f32 in
    // 1e-3 .. 1e6 m (250 679 698 values), device stratum vs the Rust lane's:
    //
    //   __logf(near) / __logf(1.5f)   158 disagreements   (as shipped)
    //     logf(near) /   logf(1.5f)   305                 (numerator only: WORSE)
    //     logf(near) / THIS             9                 (both halves mirrored)
    //
    // CUDA's `logf(1.5f)` is 0x3ecf9920 while `__logf(1.5f)` and Rust's
    // `1.5f32.ln()` are both 0x3ecf991f — so the shipped expression was
    // accidentally consistent about where the boundaries are, and swapping only
    // the numerator to the accurate `logf` breaks that consistency faster than it
    // buys accuracy. One ulp on a divisor tilts every boundary the same way.
    // The residual 9 are f32 values within one ulp of a 1.5^k boundary, where
    // CUDA's and glibc's `logf` round the numerator apart; closing those needs a
    // bit-identical `logf`, not a constant.
    let fuse_ratio_ln: f32 = const_from(
        "../noise-compute/src/propagation/arc_screening.rs",
        "const ARC_FUSE_RANGE_RATIO: f32 = ",
    )
    .parse::<f32>()
    .expect("ARC_FUSE_RANGE_RATIO parses as f32")
    .ln();
    // `{:e}` is the shortest representation that round-trips to the same f32, and
    // it can never come out looking like an integer (`2f` is not a C literal).
    let fuse_ratio_ln = format!("{fuse_ratio_ln:e}f");
    // This vector is both the nvcc argument authority and the artifact receipt.
    // Keeping one ordered value prevents a successful binary from being sealed
    // against a hand-reconstructed define list that differs from its PTX.
    let mut nvcc_defines = vec![
        format!("-DNPD_NC={num_classes}"),
        format!("-DTPX={tile_px}"),
        format!("-DBIN_W={bin_w}"),
        format!("-DBARRIER_ABI_VERSION={barrier_abi_version}"),
        format!("-DBARRIER_STRIDE={barrier_stride}"),
        format!("-DSOURCE_SEGMENT_ABI_VERSION={source_segment_abi_version}"),
        format!("-DSOURCE_SEGMENT_STRIDE={source_segment_stride}"),
        format!("-DLINE_KERNEL_ARGUMENT_COUNT={line_kernel_argument_count}"),
        format!("-DSURFACE_META_ABI_VERSION={surface_meta_abi_version}"),
        format!("-DSURFACE_META_SLOTS={surface_meta_slots}"),
        format!("-DV2_H0={v2_h0}"),
        format!("-DOUT_H0_COUNTER_BYTE_OFFSET={out_h0_counter_byte_offset}"),
        format!("-DOUT_H0_COUNTERS={out_h0_counters}"),
        format!("-DOUT_SLOTS_H0={out_slots_h0}"),
        format!("-DPROF_H0_COUNTERS={h0_counters}"),
        format!("-DPROF_H0_PAIR_DIAGNOSTIC={h0_pair_diagnostic}"),
        format!("-DH0_PAIR_DIAGNOSTIC_ABI_VERSION={h0_pair_diagnostic_abi_version}"),
        format!("-DH0_PAIR_DIAGNOSTIC_HEADER_WORDS={h0_pair_diagnostic_header_words}"),
        format!("-DH0_PAIR_DIAGNOSTIC_NODE_WORDS={h0_pair_diagnostic_node_words}"),
        format!("-DH0_PAIR_DIAGNOSTIC_RECORD_WORDS={h0_pair_diagnostic_record_words}"),
        format!("-DH0_PAIR_DIAGNOSTIC_NODE_BASE={h0_pair_diagnostic_node_base}"),
        format!("-DH0_PAIR_DIAGNOSTIC_RECORD_BASE={h0_pair_diagnostic_record_base}"),
        format!("-DH0_PAIR_DIAGNOSTIC_MAGIC={h0_pair_diagnostic_magic}ull"),
        format!("-DV2_H0_NODE_CAP={h0_node_cap}"),
        format!("-DV2_THETA_MAX_RAD={theta_max_rad}"),
        format!("-DV2_ROAD_D_FLOOR_M={road_d_floor_m}"),
        format!("-DV2_RAIL_D_FLOOR_M={rail_d_floor_m}"),
        format!("-DV2_R_ATM_BASE_M_PER_RAD={r_atm_base_m_per_rad}"),
        format!("-DV2_A_LIVE_DB={a_live_db}"),
        format!("-DV2_LINE_MAX_LENGTH_M={line_max_length_m}"),
        format!("-DM_LAT={metres_per_degree_latitude}"),
        format!("-DV2_ALPHA_ATM_0={}", alpha_atm[0]),
        format!("-DV2_ALPHA_ATM_1={}", alpha_atm[1]),
        format!("-DV2_ALPHA_ATM_2={}", alpha_atm[2]),
        format!("-DV2_ALPHA_ATM_3={}", alpha_atm[3]),
        format!("-DV2_ALPHA_ATM_4={}", alpha_atm[4]),
        format!("-DV2_ALPHA_ATM_5={}", alpha_atm[5]),
        format!("-DV2_ALPHA_ATM_6={}", alpha_atm[6]),
        format!("-DV2_ALPHA_ATM_7={}", alpha_atm[7]),
        format!("-DOBST_META_STRIDE={meta_stride}"),
        format!("-DFOOT_BOX_STRIDE={foot_box_stride}"),
        format!("-DARC_DEGENERATE_SPAN={degenerate_span}"),
        format!("-DARC_CP_EPS={cp_eps}"),
        format!("-DARC_QUADRATURE_MIN_RAD={arc_quadrature_min}"),
        format!("-DGROUND_HARD_FLOOR_DB={ground_hard_floor}"),
        format!("-DGROUND_SOUND_SPEED={ground_sound_speed}"),
        format!("-DCNOSSOS_GROUND_ALPHA0={ground_alpha0}"),
        format!("-DCNOSSOS_GROUND_DELTA_ZT_COEFF={ground_delta_zt}"),
        format!("-DP_FAV={p_fav}"),
        format!("-DARC_PENUMBRA_FLOOR_M={penumbra_floor}"),
        format!("-DARC_FUSE_HEIGHT_TOL_M={fuse_height_tol}"),
        format!("-DARC_FUSE_RANGE_RATIO_LN={fuse_ratio_ln}"),
    ];
    nvcc_defines.extend(extra_defines);
    fs::write(
        out.join("nvcc-defines.txt"),
        format!("{}\n", nvcc_defines.join("\n")),
    )
    .expect("write exact nvcc define receipt");
    for entry in fs::read_dir("kernels").expect("kernels/ dir") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cu") {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            println!("cargo:rerun-if-changed={}", path.display());
            // This translation unit exercises the H0-only layout contract. A
            // stock `gpu` build intentionally has no `qm_h0_layout_valid`, so
            // compiling the selftest without the matching host feature creates
            // a third, invalid role instead of testing production.
            if stem == "qm_h0_node_selftest" && v2_h0 == 0 {
                continue;
            }
            let ptx = out.join(format!("{stem}.ptx"));
            let status = Command::new("nvcc")
                .args(["-ptx", &format!("-arch={arch}"), "-O3"])
                .args(&nvcc_defines)
                .arg(&path)
                .arg("-o")
                .arg(&ptx)
                .status()
                .expect("nvcc not found — `--features gpu` needs the CUDA toolkit on this host");
            assert!(status.success(), "nvcc failed to compile {path:?}");
        }
    }
}

/// One Rust `pub const <name>: usize = N;` line, parsed for injection into nvcc.
/// Panics loudly: a silently missing constant is exactly the drift this guards.
fn const_from(path: &str, prefix: &str) -> String {
    println!("cargo:rerun-if-changed={path}");
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{path} not readable for constant injection: {e}"))
        .lines()
        .find_map(|l| {
            l.trim()
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
        .unwrap_or_else(|error| panic!("`{prefix}` in {path} is not a numeric f64: {error}"));
    c_f64(value)
}

fn numeric_u64_const(path: &str, prefix: &str) -> String {
    let source = const_from(path, prefix).replace('_', "");
    let value = source
        .strip_prefix("0x")
        .map(|digits| u64::from_str_radix(digits, 16))
        .unwrap_or_else(|| source.parse::<u64>())
        .unwrap_or_else(|error| panic!("`{prefix}` in {path} is not a numeric u64: {error}"));
    format!("0x{value:016x}")
}

fn numeric_f64_array_const(path: &str, prefix: &str, expected_len: usize) -> Vec<String> {
    let source = const_from(path, prefix);
    let body = source
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_else(|| panic!("`{prefix}` in {path} is not an array literal"));
    let values: Vec<_> = body
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            let parsed = value
                .trim()
                .replace('_', "")
                .parse::<f64>()
                .unwrap_or_else(|error| panic!("`{value}` in {path} is not f64: {error}"));
            c_f64(parsed)
        })
        .collect();
    assert_eq!(
        values.len(),
        expected_len,
        "unexpected f64 array length in {path}"
    );
    values
}
