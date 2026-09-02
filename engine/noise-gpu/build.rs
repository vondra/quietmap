//! Build script for `noise-gpu` — compiles CUDA kernels to PTX plus the surface
//! production cubin (only under `gpu`) so CUDA-less hosts still build cleanly.
// Compile every kernels/*.cu to PTX for validators and scatter.cu to cubin for
// zero-JIT surface workers. The cubin targets exactly one arch; the
// benchmark/deploy path already rebuilds on each GPU role.
// nvcc is isolated from Cargo's rustflags, so target-cpu=native parity is
// untouched. The arch defaults to this host's own card and NOISE_GPU_ARCH
// overrides it, both in `build_cuda_arch.rs`, which relevant-source-gpu's
// build script includes from here so the two engines target one card.
//
// Only the `gpu` feature (the gpu-surface/e2-full bins) needs CUDA. Without it the
// crate is the CPU-side lib alone, so skip nvcc entirely — a host with no CUDA
// toolkit (e.g. a CPU-only box) then builds noise-gpu cleanly. nvcc is required only when you
// explicitly build `--features gpu`, which only happens on a GPU host.
#[path = "build_cuda_arch.rs"]
mod build_cuda_arch;
mod build_defines;

use build_defines::parse_experimental_defines;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=NOISE_GPU_ARCH");
    println!("cargo:rerun-if-changed=build_cuda_arch.rs");
    println!("cargo:rerun-if-env-changed=NOISE_GPU_DEFINES");
    println!("cargo:rerun-if-env-changed=NOISE_GPU_SKIP_NVCC");
    println!("cargo:rerun-if-changed=build_defines.rs");
    println!("cargo:rerun-if-changed=../../scripts/reviewed-defines.txt");
    let extra_defines =
        parse_experimental_defines(&env::var("NOISE_GPU_DEFINES").unwrap_or_default())
            .unwrap_or_else(|error| panic!("invalid NOISE_GPU_DEFINES: {error}"));
    // The host runner must allocate and reconstruct the candidate output only
    // when the PTX carries the same compile-time arm. Keep this derived marker
    // in Cargo's generated environment instead of making a runtime env opt-in
    // that could accidentally pair stock PTX with candidate host code.
    // The W1 candidate cheap evaluator is calibrated at the reviewed
    // +5.0 dB ground override with byte stopping disabled. Make the host
    // candidate gate prove that exact PTX configuration, so a bare
    // MULTIFIDELITY_LINE build with either kernel default cannot silently use
    // candidate allocation or reconstruction.
    let has_multifidelity_line = extra_defines
        .iter()
        .any(|define| define == "-DMULTIFIDELITY_LINE" || define == "-DMULTIFIDELITY_LINE=1");
    let has_multifidelity_ground = extra_defines
        .iter()
        .any(|define| define == "-DMULTIFIDELITY_CHEAP_GROUND_DB=5.0");
    let has_multifidelity_compact_byte_stop = extra_defines
        .iter()
        .any(|define| define == "-DMULTIFIDELITY_COMPACT_BYTE_STOP=0");
    let multifidelity_line =
        has_multifidelity_line && has_multifidelity_ground && has_multifidelity_compact_byte_stop;
    println!(
        "cargo:rustc-env=NOISE_GPU_MULTIFIDELITY_LINE={}",
        if multifidelity_line { "1" } else { "0" }
    );
    let z13_stride = explicit_define_value(&extra_defines, "MULTIFIDELITY_Z13_STRIDE");
    let z13_adaptive = explicit_define_value(&extra_defines, "MULTIFIDELITY_Z13_ADAPTIVE");
    let arc_union_before_span_clip =
        explicit_define_value(&extra_defines, "ARC_UNION_BEFORE_SPAN_CLIP");
    let cartesian_unbinned_anchor =
        multifidelity_line && z13_stride == Some("4") && z13_adaptive == Some("0");
    if cartesian_unbinned_anchor {
        assert_eq!(
            explicit_define_value(&extra_defines, "SHADOW_MID_STRIDE"),
            Some("1"),
            "the exact stride4 Cartesian backend requires SHADOW_MID_STRIDE=1"
        );
        assert_eq!(
            arc_union_before_span_clip,
            Some("1"),
            "the exact stride4 Cartesian backend requires ARC_UNION_BEFORE_SPAN_CLIP=1"
        );
    } else {
        assert!(
            arc_union_before_span_clip.is_none(),
            "ARC_UNION_BEFORE_SPAN_CLIP is fenced to the exact stride4 Cartesian backend"
        );
    }
    println!(
        "cargo:rustc-env=NOISE_GPU_MULTIFIDELITY_CARTESIAN_UNBINNED_ANCHOR={}",
        if cartesian_unbinned_anchor { "1" } else { "0" }
    );
    match (z13_stride, z13_adaptive) {
        (Some(stride), Some("0")) => {
            assert!(
                multifidelity_line,
                "MULTIFIDELITY_Z13_STRIDE requires the reviewed W1 multifidelity define trio"
            );
            println!("cargo:rustc-env=NOISE_GPU_MULTIFIDELITY_Z13_STRIDE={stride}");
            println!("cargo:rustc-env=NOISE_GPU_MULTIFIDELITY_Z13_ADAPTIVE=0");
        }
        (Some(_), Some(adaptive)) => panic!(
            "MULTIFIDELITY_Z13_ADAPTIVE must be 0 for the strict W1 z13 ladder, got {adaptive}"
        ),
        (Some(_), None) => {
            panic!("MULTIFIDELITY_Z13_STRIDE requires MULTIFIDELITY_Z13_ADAPTIVE=0")
        }
        (None, Some(_)) => {
            panic!("MULTIFIDELITY_Z13_ADAPTIVE cannot be supplied without MULTIFIDELITY_Z13_STRIDE")
        }
        (None, None) => {}
    }
    // Watch the whole dir, not just each .cu — otherwise ADDING a new kernel
    // (e.g. airborne.cu) doesn't re-run this script, so its .ptx never builds.
    println!("cargo:rerun-if-changed=kernels");
    if env::var_os("CARGO_FEATURE_GPU").is_none() {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    // Host-only experiment checks use the real GPU feature so that the
    // multifidelity binary and tests are type-checked, but deliberately skip
    // nvcc when no CUDA toolkit is available. This branch is scratch-only and
    // never changes the production build path.
    if env::var_os("NOISE_GPU_SKIP_NVCC").is_some() {
        println!("cargo:rustc-env=NOISE_GPU_SCATTER_CUBIN_SHA256=skipped-nvcc-host-check");
        for entry in fs::read_dir("kernels").expect("kernels/ dir") {
            let path = entry.expect("kernel entry").path();
            if path.extension().is_some_and(|extension| extension == "cu") {
                let stem = path.file_stem().expect("kernel stem").to_str().unwrap();
                fs::write(out.join(format!("{stem}.ptx")), "")
                    .expect("write skipped-CUDA PTX placeholder");
                if stem == "scatter" {
                    fs::write(out.join("scatter.cubin"), []).expect("write skipped-CUDA cubin");
                }
            }
        }
        return;
    }
    let arch = build_cuda_arch::cuda_arch("noise-gpu");
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
    let tile_px = const_from(
        "../raster-reader/src/fused_tile_z13.rs",
        "pub const TILE_PX: usize = ",
    );
    let bin_w = const_from("src/lib.rs", "pub const BIN_W: usize = ");
    let barrier_stride = const_from("src/lib.rs", "pub const BARRIER_STRIDE: usize = ");
    let source_segment_stride =
        const_from("src/lib.rs", "pub const SOURCE_SEGMENT_STRIDE: usize = ");
    let line_kernel_argument_count = const_from(
        "src/lib.rs",
        "pub const LINE_KERNEL_ARGUMENT_COUNT: usize = ",
    );
    let surface_meta_slots = const_from("src/lib.rs", "pub const SURFACE_META_SLOTS: usize = ");
    let compact_receiver_record_words = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_RECEIVER_RECORD_WORDS: usize = ",
    );
    let compact_control_words = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_CONTROL_WORDS: usize = ",
    );
    let compact_control_block_words = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS: usize = ",
    );
    let compact_abi_version = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_ABI_VERSION: usize = ",
    );
    let compact_output_stride = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_OUTPUT_STRIDE: usize = ",
    );
    let compact_output_index_slot = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT: usize = ",
    );
    let compact_output_energy_base = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE: usize = ",
    );
    let compact_output_fault_slot = const_from(
        "src/lib.rs",
        "pub const MULTIFIDELITY_COMPACT_OUTPUT_FAULT_SLOT: usize = ",
    );
    let out_arcstat_counters = const_from("src/lib.rs", "pub const OUT_ARCSTAT_COUNTERS: usize = ");
    let metres_per_degree_latitude = numeric_f64_const(
        "../noise-compute/src/constants.rs",
        "pub const M_PER_DEG_LAT: f64 = ",
    );
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
    // The line-screening quadrature's two constants, injected so the kernel cannot drift
    // from the CPU rule it paints: the bucket count (seg_sampling's
    // SEG_SAMPLES_DEFAULT) and the per-bucket arc gate. The gate is SPELLED
    // `<deg>_f64.to_radians()` in seg_sampling.rs and the spelling is
    // load-bearing — `3·π/180` differs from `3.0_f64.to_radians()` by 1 ULP
    // (seg_sampling.rs's own provenance note), and the measured constant went
    // through `to_radians` — so parse the degree literal out of the Rust source
    // and run the SAME `to_radians` here, in Rust, bit-for-bit.
    let seg_samples = const_from(
        "../noise-compute/src/propagation/seg_sampling.rs",
        "pub const SEG_SAMPLES_DEFAULT: usize = ",
    );
    let seg_arc_gate_expr = const_from(
        "../noise-compute/src/propagation/seg_sampling.rs",
        "pub const SEG_ARC_MIN_SPAN_RAD: f64 = ",
    );
    let seg_arc_min_span = {
        let deg: f64 = seg_arc_gate_expr
            .strip_suffix("_f64.to_radians()")
            .unwrap_or_else(|| {
                panic!(
                    "SEG_ARC_MIN_SPAN_RAD must stay spelled `<deg>_f64.to_radians()` \
                     (the kernel mirrors the expression, not the value), got \
                     `{seg_arc_gate_expr}`"
                )
            })
            .parse()
            .expect("SEG_ARC_MIN_SPAN_RAD degree literal must parse as f64");
        c_f64(deg.to_radians())
    };
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
        format!("-DBARRIER_STRIDE={barrier_stride}"),
        format!("-DSOURCE_SEGMENT_STRIDE={source_segment_stride}"),
        format!("-DLINE_KERNEL_ARGUMENT_COUNT={line_kernel_argument_count}"),
        format!("-DSURFACE_META_SLOTS={surface_meta_slots}"),
        format!("-DMULTIFIDELITY_COMPACT_RECEIVER_RECORD_WORDS={compact_receiver_record_words}"),
        format!("-DMULTIFIDELITY_COMPACT_CONTROL_WORDS={compact_control_words}"),
        format!("-DMULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS={compact_control_block_words}"),
        format!("-DMULTIFIDELITY_COMPACT_ABI_VERSION={compact_abi_version}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_STRIDE={compact_output_stride}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT={compact_output_index_slot}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE={compact_output_energy_base}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_FAULT_SLOT={compact_output_fault_slot}"),
        format!("-DOUT_ARCSTAT_COUNTERS={out_arcstat_counters}"),
        format!("-DM_LAT={metres_per_degree_latitude}"),
        format!("-DOBST_META_STRIDE={meta_stride}"),
        format!("-DFOOT_BOX_STRIDE={foot_box_stride}"),
        format!("-DARC_DEGENERATE_SPAN={degenerate_span}"),
        format!("-DSEG_ARC_MIN_SPAN_RAD={seg_arc_min_span}"),
        format!("-DSEG_SAMPLES={seg_samples}"),
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
            let compile = |kind: &str, output: PathBuf| {
                let status = Command::new("nvcc")
                    .args([kind, &format!("-arch={arch}"), "-O3"])
                    .args(&nvcc_defines)
                    .arg(&path)
                    .arg("-o")
                    .arg(&output)
                    .status()
                    .expect(
                        "nvcc not found — `--features gpu` needs the CUDA toolkit on this host",
                    );
                assert!(status.success(), "nvcc {kind} failed to compile {path:?}");
            };
            compile("-ptx", out.join(format!("{stem}.ptx")));
            if stem == "scatter" {
                let scatter_cubin = out.join(format!("{stem}.cubin"));
                compile("-cubin", scatter_cubin.clone());
                assert!(
                    fs::metadata(&scatter_cubin)
                        .expect("stat compiled scatter cubin")
                        .len()
                        > 0,
                    "compiled scatter cubin is empty"
                );
                println!(
                    "cargo:rustc-env=NOISE_GPU_SCATTER_CUBIN_SHA256={}",
                    file_sha256(&scatter_cubin)
                );
            }
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

fn explicit_define_value<'a>(defines: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = format!("-D{name}=");
    defines
        .iter()
        .find_map(|define| define.strip_prefix(prefix.as_str()))
}

fn file_sha256(path: &Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .unwrap_or_else(|error| panic!("run sha256sum for {}: {error}", path.display()));
    assert!(
        output.status.success(),
        "sha256sum failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8(output.stdout).expect("sha256sum output is not UTF-8");
    let digest = stdout
        .split_whitespace()
        .next()
        .expect("sha256sum output omitted the digest");
    assert!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "sha256sum emitted a non-canonical digest: {digest:?}"
    );
    digest.to_owned()
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
