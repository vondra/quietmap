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
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=NOISE_GPU_ARCH");
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
    // NOISE_GPU_DEFINES: extra -D flags for the kernel build (space separated,
    // e.g. "-DPROF_COUNTERS=1"). DEV INSTRUMENT ONLY — every switch it drives
    // defaults to the production value, so an unset var builds the shipped
    // kernel. Without this passthrough the -D levers are silently IGNORED and an
    // A/B measures the default build twice, which is worse than not having them.
    println!("cargo:rerun-if-env-changed=NOISE_GPU_DEFINES");
    let extra_defines: Vec<String> = env::var("NOISE_GPU_DEFINES")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let tile_px = const_from(
        "../raster-reader/src/fused_tile_z13.rs",
        "pub const TILE_PX: usize = ",
    );
    let bin_w = const_from("src/lib.rs", "pub const BIN_W: usize = ");
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
    for entry in fs::read_dir("kernels").expect("kernels/ dir") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cu") {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let ptx = out.join(format!("{stem}.ptx"));
            let status = Command::new("nvcc")
                .args([
                    "-ptx",
                    &format!("-arch={arch}"),
                    "-O3",
                    &format!("-DNPD_NC={num_classes}"),
                    &format!("-DTPX={tile_px}"),
                    &format!("-DBIN_W={bin_w}"),
                    &format!("-DOBST_META_STRIDE={meta_stride}"),
                    &format!("-DFOOT_BOX_STRIDE={foot_box_stride}"),
                    &format!("-DARC_DEGENERATE_SPAN={degenerate_span}"),
                    &format!("-DARC_CP_EPS={cp_eps}"),
                    &format!("-DARC_QUADRATURE_MIN_RAD={arc_quadrature_min}"),
                    &format!("-DGROUND_HARD_FLOOR_DB={ground_hard_floor}"),
                    &format!("-DARC_PENUMBRA_FLOOR_M={penumbra_floor}"),
                    &format!("-DARC_FUSE_HEIGHT_TOL_M={fuse_height_tol}"),
                    &format!("-DARC_FUSE_RANGE_RATIO_LN={fuse_ratio_ln}"),
                ])
                .args(&extra_defines)
                .arg(&path)
                .arg("-o")
                .arg(&ptx)
                .status()
                .expect("nvcc not found — `--features gpu` needs the CUDA toolkit on this host");
            assert!(status.success(), "nvcc failed to compile {path:?}");
            println!("cargo:rerun-if-changed={}", path.display());
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
