//! Build script for `noise-gpu` — compiles CUDA kernels to PTX plus the surface
//! production cubin (only under `gpu`) so CUDA-less hosts still build cleanly.
// Compile every kernels/*.cu to PTX for validators and scatter.cu to cubin for
// zero-JIT surface workers. The cubin targets exactly one arch; the
// benchmark/deploy path already rebuilds on each GPU role.
// nvcc is isolated from Cargo's rustflags, so target-cpu=native parity is
// untouched. The arch defaults to this host's own card (see `detect_arch`);
// NOISE_GPU_ARCH overrides it.
//
// Only the `gpu` feature (the gpu-surface/e2-full bins) needs CUDA. Without it the
// crate is the CPU-side lib alone, so skip nvcc entirely — a host with no CUDA
// toolkit (e.g. a CPU-only box) then builds noise-gpu cleanly. nvcc is required only when you
// explicitly build `--features gpu`, which only happens on a GPU host.
mod build_defines;
#[path = "../noise-compute/src/h0_production_selection.rs"]
#[allow(dead_code)]
mod h0_production_selection;
#[path = "../noise-compute/src/h0_production_selection_parser.rs"]
mod h0_production_selection_parser;

use build_defines::parse_experimental_defines;
use h0_production_selection::H0ProductionSelection;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const H0_SELECTION_RECORD_PATH: &str = "../noise-compute/src/h0_production_selection_record.rs";

/// Arch used when this host's own card cannot be determined: Ada (4060/4070).
const DEFAULT_ARCH: &str = "sm_89";

/// Announce the fallback and take `DEFAULT_ARCH`.
fn fall_back(reason: &str) -> String {
    println!(
        "cargo:warning=noise-gpu: {reason}; the AOT cubin targets {DEFAULT_ARCH}. \
         Set NOISE_GPU_ARCH if that is not this card: a rejected cubin costs a \
         PTX JIT in every W1 process, and the W2 exact path refuses to load."
    );
    DEFAULT_ARCH.to_owned()
}

/// `sm_NN` for the first card `nvidia-smi --query-gpu=compute_cap` reports.
///
/// The first row, not a unanimous one, because that is the card CUDA opens as
/// device 0 and is what the fleet's own provisioning picks. `caps_disagree`
/// reports the ambiguity separately so the caller can say so.
fn arch_from_compute_caps(listing: &str) -> Result<String, &'static str> {
    let first = listing
        .lines()
        .map(str::trim)
        .find(|row| !row.is_empty())
        .ok_or("nvidia-smi listed no GPU")?;
    let digits = first.replace('.', "");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("nvidia-smi returned an unparsable compute capability");
    }
    Ok(format!("sm_{digits}"))
}

/// Whether the reported cards differ, so that no one cubin fits all of them.
fn caps_disagree(listing: &str) -> bool {
    let mut rows = listing.lines().map(str::trim).filter(|row| !row.is_empty());
    rows.next()
        .is_some_and(|first| rows.any(|other| other != first))
}

/// Whether this host's nvcc can emit `arch` at all.
///
/// A toolkit older than the card would otherwise turn a build that used to work
/// into `nvcc fatal: Unsupported gpu architecture` — the fixed default merely
/// JIT-ed. Verified both ways: CUDA 11.5 lists sm_75 but not sm_89, CUDA 13.3
/// lists sm_120 but not sm_70. An nvcc too old to answer is left to the compile
/// step, which reports the real error.
fn nvcc_can_emit(arch: &str) -> bool {
    let Ok(listing) = Command::new("nvcc").arg("--list-gpu-code").output() else {
        return true;
    };
    !listing.status.success()
        || String::from_utf8_lossy(&listing.stdout)
            .lines()
            .any(|row| row.trim() == arch)
}

/// This host's own compute capability as `sm_NN`, for the AOT cubin.
///
/// Every fleet build is a per-box native build — `.cargo/config.toml` pins
/// `target-cpu=native`, so a binary is never shared between machines — which
/// makes the local card the card that runs this binary. A fixed default was
/// therefore wrong on every non-Ada box, and wrong silently: a 5070 (sm_120)
/// rejects an sm_89 image, so each W1 process JIT-compiled the embedded PTX at
/// startup (measured 2026-08-28, cold CUDA cache, road z12: 57.2 s against
/// 0.2 s, byte-identical tiles). Only W1 fails open; the W2 exact anchors load
/// through `load_embedded_cubin_exact`, which refuses the recompile.
///
/// Never fails the build: an unreadable card, an unparsable answer, or an nvcc
/// that cannot emit the detected arch all fall back to `DEFAULT_ARCH` — the old
/// unconditional default, so no build that worked stops working. Cargo cannot
/// watch a card, so only the tracked `NOISE_GPU_ARCH` forces a rebuild; set it
/// if you reuse a target dir across a GPU swap.
fn detect_arch() -> String {
    let Ok(probe) = Command::new("nvidia-smi")
        .args(["--query-gpu=compute_cap", "--format=csv,noheader,nounits"])
        .output()
    else {
        return fall_back("nvidia-smi is not on PATH");
    };
    if !probe.status.success() {
        return fall_back("nvidia-smi reported no usable GPU");
    }
    let listing = String::from_utf8_lossy(&probe.stdout);
    let arch = match arch_from_compute_caps(&listing) {
        Ok(arch) => arch,
        Err(reason) => return fall_back(reason),
    };
    if !nvcc_can_emit(&arch) {
        return fall_back("this host's nvcc is too old to emit its own card's arch");
    }
    if caps_disagree(&listing) {
        println!(
            "cargo:warning=noise-gpu: this host mixes GPU compute capabilities; \
             the cubin targets {arch} (the first card). Set NOISE_GPU_ARCH to choose another."
        );
    }
    arch
}

#[cfg(test)]
mod arch_tests {
    use super::{arch_from_compute_caps, caps_disagree};

    #[test]
    fn one_card_of_each_fleet_generation_maps_to_its_arch() {
        for (listing, arch) in [
            ("12.0\n", "sm_120"),
            ("7.5\n", "sm_75"),
            ("8.9\n", "sm_89"),
            ("7.0\n", "sm_70"),
            (" 12.0 \r\n", "sm_120"),
            ("7.5\n\n", "sm_75"),
        ] {
            assert_eq!(
                arch_from_compute_caps(listing).unwrap(),
                arch,
                "{listing:?}"
            );
        }
    }

    #[test]
    fn identical_cards_are_not_a_disagreement() {
        let listing = "12.0\n12.0\n12.0\n12.0\n";
        assert_eq!(arch_from_compute_caps(listing).unwrap(), "sm_120");
        assert!(!caps_disagree(listing));
    }

    #[test]
    fn a_mixed_host_takes_the_first_card_and_is_flagged() {
        let listing = "12.0\n7.5\n";
        assert_eq!(arch_from_compute_caps(listing).unwrap(), "sm_120");
        assert!(caps_disagree(listing));
    }

    #[test]
    fn an_unreadable_listing_is_an_error_never_a_bogus_arch() {
        for listing in ["", "\n  \n", "N/A\n", "[N/A]\n", ".\n"] {
            assert!(arch_from_compute_caps(listing).is_err(), "{listing:?}");
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=NOISE_GPU_ARCH");
    println!("cargo:rerun-if-env-changed=NOISE_GPU_DEFINES");
    println!("cargo:rerun-if-env-changed=NOISE_GPU_SKIP_NVCC");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0_DIAGNOSTIC");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_V2_H0_COUNTERS");
    println!("cargo:rerun-if-changed=build_defines.rs");
    println!("cargo:rerun-if-changed=../../scripts/reviewed-defines.txt");
    println!("cargo:rerun-if-changed=../noise-compute/src/h0_production_selection.rs");
    println!("cargo:rerun-if-changed=../noise-compute/src/h0_production_selection_parser.rs");
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
        fs::write(
            out.join("generated_h0_selection.rs"),
            format!(
                "pub const GENERATED_V2_H0_NODE_CAP: usize = 66;\n\
                 pub const GENERATED_V2_THETA_MAX_RAD_BITS: u64 = 0x{:016x};\n",
                (core::f64::consts::PI / 60.0).to_bits()
            ),
        )
        .expect("write skipped-CUDA Rust selection mirror");
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
    let arch = env::var("NOISE_GPU_ARCH")
        .ok()
        .filter(|arch| !arch.is_empty())
        .unwrap_or_else(detect_arch);
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
    let h0_selection = if v2_h0 == 1 {
        // Never watch an absent record in the stock role: Cargo treats a
        // missing watched path as dirty forever and would rerun nvcc on every
        // no-op build. The v2-h0 feature transition is already a rerun key.
        println!("cargo:rerun-if-changed={H0_SELECTION_RECORD_PATH}");
        let source = fs::read_to_string(H0_SELECTION_RECORD_PATH).unwrap_or_else(|error| {
            panic!(
                "feature `v2-h0` requires reviewed `{H0_SELECTION_RECORD_PATH}` after terminal H0_QUADRATURE_ACCEPTED: {error}"
            )
        });
        Some(
            H0ProductionSelection::parse_and_verify(&source)
                .unwrap_or_else(|error| panic!("invalid H0 production selection record: {error}")),
        )
    } else {
        None
    };
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
    let out_h0_counter_byte_offset = const_from(
        "src/lib.rs",
        "pub const OUT_H0_COUNTER_BYTE_OFFSET: usize = ",
    )
    .replace('_', "");
    let h0_output_abi_version =
        const_from("src/lib.rs", "pub const H0_OUTPUT_ABI_VERSION: usize = ");
    let out_h0_counters = const_from("src/lib.rs", "pub const OUT_H0_COUNTERS: usize = ");
    let out_arcstat_counters = const_from("src/lib.rs", "pub const OUT_ARCSTAT_COUNTERS: usize = ");
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
    let h0_pair_diagnostic_magic =
        numeric_u64_const("src/lib.rs", "pub const H0_PAIR_DIAGNOSTIC_MAGIC: u64 = ");
    let (
        h0_node_cap,
        theta_max_rad,
        theta_max_rad_bits,
        h0_selection_schema,
        h0_selection_epoch,
        h0_h_max,
    ) = if let Some(selection) = &h0_selection {
        (
            selection.node_cap.to_string(),
            c_f64(selection.theta_radians()),
            format!("0x{:016x}", selection.theta_radians_bits),
            selection.schema.to_string(),
            selection.epoch.to_string(),
            selection.h_max.to_string(),
        )
    } else {
        (
            "66".to_owned(),
            c_f64(core::f64::consts::PI / 60.0),
            format!("0x{:016x}", (core::f64::consts::PI / 60.0).to_bits()),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
        )
    };
    let h0_pair_diagnostic_record_base = (h0_pair_diagnostic_node_base
        .replace('_', "")
        .parse::<usize>()
        .expect("H0 pair diagnostic node base is usize")
        + h0_node_cap.parse::<usize>().expect("H0 node cap is usize")
            * h0_pair_diagnostic_node_words
                .replace('_', "")
                .parse::<usize>()
                .expect("H0 pair diagnostic node words is usize"))
    .to_string();
    if let Some(selection) = &h0_selection {
        fs::write(
            out.join("h0-production-selection-receipt.txt"),
            selection.render_build_receipt(),
        )
        .expect("write H0 production selection receipt");
    }
    // Rust consumes this generated mirror even when Cargo feature unification
    // enables noise-compute's selection without noise-gpu's v2-h0 role. The
    // compile-time assertions in lib.rs make that mixed role impossible.
    fs::write(
        out.join("generated_h0_selection.rs"),
        format!(
            "pub const GENERATED_V2_H0_NODE_CAP: usize = {h0_node_cap};\n\
             pub const GENERATED_V2_THETA_MAX_RAD_BITS: u64 = {theta_max_rad_bits};\n"
        ),
    )
    .expect("write generated Rust H0 selection mirror");
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
             #define V2_H0_OUTPUT_ABI_VERSION {h0_output_abi_version}\n\
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
             #define V2_H0_SELECTION_SCHEMA {h0_selection_schema}\n\
             #define V2_H0_SELECTION_EPOCH {h0_selection_epoch}ull\n\
             #define V2_H0_NODE_CAP {h0_node_cap}\n\
             #define V2_THETA_MAX_RAD {theta_max_rad}\n\
             #define V2_THETA_MAX_RAD_BITS {theta_max_rad_bits}ull\n\
             #define V2_H0_H_MAX {h0_h_max}\n\
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
    // The §3.5e quadrature's two constants, injected so the kernel cannot drift
    // from the CPU rule it paints: the bucket count (the tile painter's
    // SEG_SAMPLES_DEFAULT) and the per-bucket arc gate. The gate is SPELLED
    // `<deg>_f64.to_radians()` in seg_sampling.rs and the spelling is
    // load-bearing — `3·π/180` differs from `3.0_f64.to_radians()` by 1 ULP
    // (seg_sampling.rs's own provenance note), and the measured constant went
    // through `to_radians` — so parse the degree literal out of the Rust source
    // and run the SAME `to_radians` here, in Rust, bit-for-bit.
    let seg_samples = const_from(
        "../tile-painter/src/scatter_band.rs",
        "const SEG_SAMPLES_DEFAULT: usize = ",
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
        format!("-DBARRIER_ABI_VERSION={barrier_abi_version}"),
        format!("-DBARRIER_STRIDE={barrier_stride}"),
        format!("-DSOURCE_SEGMENT_ABI_VERSION={source_segment_abi_version}"),
        format!("-DSOURCE_SEGMENT_STRIDE={source_segment_stride}"),
        format!("-DLINE_KERNEL_ARGUMENT_COUNT={line_kernel_argument_count}"),
        format!("-DMULTIFIDELITY_COMPACT_RECEIVER_RECORD_WORDS={compact_receiver_record_words}"),
        format!("-DMULTIFIDELITY_COMPACT_CONTROL_WORDS={compact_control_words}"),
        format!("-DMULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS={compact_control_block_words}"),
        format!("-DMULTIFIDELITY_COMPACT_ABI_VERSION={compact_abi_version}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_STRIDE={compact_output_stride}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT={compact_output_index_slot}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE={compact_output_energy_base}"),
        format!("-DMULTIFIDELITY_COMPACT_OUTPUT_FAULT_SLOT={compact_output_fault_slot}"),
        format!("-DSURFACE_META_ABI_VERSION={surface_meta_abi_version}"),
        format!("-DSURFACE_META_SLOTS={surface_meta_slots}"),
        format!("-DV2_H0={v2_h0}"),
        format!("-DV2_H0_OUTPUT_ABI_VERSION={h0_output_abi_version}"),
        format!("-DOUT_H0_COUNTER_BYTE_OFFSET={out_h0_counter_byte_offset}"),
        format!("-DOUT_H0_COUNTERS={out_h0_counters}"),
        format!("-DOUT_ARCSTAT_COUNTERS={out_arcstat_counters}"),
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
        format!("-DV2_H0_SELECTION_SCHEMA={h0_selection_schema}"),
        format!("-DV2_H0_SELECTION_EPOCH={h0_selection_epoch}ull"),
        format!("-DV2_H0_NODE_CAP={h0_node_cap}"),
        format!("-DV2_THETA_MAX_RAD={theta_max_rad}"),
        format!("-DV2_THETA_MAX_RAD_BITS={theta_max_rad_bits}ull"),
        format!("-DV2_H0_H_MAX={h0_h_max}"),
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
            // This translation unit exercises the H0-only layout contract. A
            // stock `gpu` build intentionally has no `qm_h0_layout_valid`, so
            // compiling the selftest without the matching host feature creates
            // a third, invalid role instead of testing production.
            if stem == "qm_h0_node_selftest" && v2_h0 == 0 {
                continue;
            }
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
