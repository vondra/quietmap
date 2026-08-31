//! One-time setup for the gpu-surface batch runner — the items built once and
//! then handed to the hot kernel-launch loop in `gpu_surface.rs`: the `LineLayer`
//! road/rail descriptor (parse, dir, source_id, halo reach, row loader), the warm
//! CUDA device + AOT scatter module loaders (`warm_device` / `warm_device_on`),
//! the `Progress` heartbeat, and the `NOISE_GPU_TIMING` gate. No per-tile/launch
//! state lives here; that stays in `gpu_surface.rs`.
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use cudarc::driver::{CudaDevice, CudaFunction};
use h3o::{CellIndex, LatLng};
use noise_compute::admin;
use noise_compute::constants::RAILWAY_REACH_CEILING;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::wire_hm3::{SOURCE_ID_RAIL, SOURCE_ID_ROAD};

const SCATTER_CUBIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scatter.cubin"));
const SCATTER_PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/scatter.ptx"));
const SCATTER_CUBIN_SHA256: &str = env!("NOISE_GPU_SCATTER_CUBIN_SHA256");
const ROAD_HALO_M: f64 = 10_000.0; // motorway-class reach (matches build_heatmap_surface)

/// `NOISE_GPU_TIMING=1` brackets each tile's line-GPU workload with CUDA events
/// and emits `KERNEL_MS=<total>`. That is one kernel for stock, or the concurrent
/// cheap + exact envelope for the Cartesian arm; it excludes H2D, D2H, and CPU
/// reconstruction. Read once: a per-launch env lookup would add host overhead to
/// the measurement. OFF (the default) creates and records no timing events.
pub(crate) fn timing_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NOISE_GPU_TIMING").as_deref() == Ok("1"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineLayer {
    Road,
    Rail,
}

impl LineLayer {
    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "road" => Ok(Self::Road),
            "rail" => Ok(Self::Rail),
            _ => bail!("unknown line layer {s:?} (road|rail)"),
        }
    }
    pub(crate) fn dir(self) -> &'static str {
        match self {
            Self::Road => "road",
            Self::Rail => "rail",
        }
    }
    pub(crate) fn source_id(self) -> u8 {
        match self {
            Self::Road => SOURCE_ID_ROAD,
            Self::Rail => SOURCE_ID_RAIL,
        }
    }
    /// Per-layer halo reach; the kernel still culls each source at its own
    /// per-row `max_distance_m` (the rail loader bakes each segment's solved
    /// 25 dB reach), so a block's shared halo can use the widest of these
    /// without changing a shorter-reach layer's output. Rail uses the per-row
    /// reach CEILING so a row extended to the clamp still ray-marches terrain
    /// along its whole path.
    pub(crate) fn halo_m(self) -> f64 {
        match self {
            Self::Road => ROAD_HALO_M,
            Self::Rail => RAILWAY_REACH_CEILING,
        }
    }
    /// Load this layer's `grid_disk(1)` line rows for a region. Road resolves the
    /// admin area for its default-AADT fallback; rail for the C1 per-region period
    /// split (EU freight ~55 % at night). The admin lookup is hoisted OUT of the
    /// match so it reaches `RailData::load_for_r4s` too (Gemini delta 5).
    pub(crate) fn load_rows(
        self,
        h3r4: &Path,
        ring: &[u64],
        cell: CellIndex,
    ) -> Result<Vec<LineRow>> {
        let ll = LatLng::from(cell);
        let admin = admin::admin_for_latlng(ll.lat(), ll.lng());
        Ok(match self {
            Self::Road => RoadData::load_for_r4s(h3r4, ring, admin)
                .context("load roads")?
                .into_rows(),
            Self::Rail => RailData::load_for_r4s(h3r4, ring, admin)
                .context("load rail")?
                .into_rows(),
        })
    }
}

/// CUDA device + AOT scatter module, created once — shared by the batch path and --stream (the warm
/// context the cluster used to re-pay on every chunk spawn).
#[derive(Clone)]
pub(crate) struct LineFunctions {
    /// Exact binned entry from the active role's scatter module. Under W2 it
    /// inherits the candidate defines; exact means no multifidelity reconstruction,
    /// not byte identity with the separately compiled stock role.
    pub(crate) stock: CudaFunction,
    /// Stride-4 anchors use the candidate module's unbinned entry. It shares
    /// the exact candidate physics while avoiding the fused entry's block cull.
    pub(crate) cartesian_unbinned_exact: Option<CudaFunction>,
    pub(crate) multifidelity_cheap: Option<CudaFunction>,
    pub(crate) multifidelity_compact: Option<CudaFunction>,
    /// W1 path: eight independent 32-lane compact receiver buckets per
    /// 256-thread block, loaded only with the candidate symbols.
    pub(crate) multifidelity_compact_packed: Option<CudaFunction>,
}

pub(crate) fn multifidelity_ptx_enabled() -> bool {
    option_env!("NOISE_GPU_MULTIFIDELITY_LINE") == Some("1")
}

pub(crate) fn multifidelity_cartesian_unbinned_anchor_enabled() -> bool {
    option_env!("NOISE_GPU_MULTIFIDELITY_CARTESIAN_UNBINNED_ANCHOR") == Some("1")
}

pub(crate) fn warm_device() -> (Arc<CudaDevice>, LineFunctions) {
    warm_device_on(false)
}

/// `with_stream` ⇒ `CudaDevice::new_with_stream` (own stream, shared primary context) so the N
/// --stream workers OVERLAP on the GPU (the gpu_airborne pattern, airborne.rs:252); `false` ⇒ the
/// null-stream device the serial batch path uses. Each call gets its own cubin module load.
pub(crate) fn warm_device_on(with_stream: bool) -> (Arc<CudaDevice>, LineFunctions) {
    let dev = if with_stream {
        CudaDevice::new_with_stream(0).expect("cuda")
    } else {
        CudaDevice::new(0).expect("cuda")
    };
    let mut symbols = vec!["line_binned_fused"];
    if multifidelity_cartesian_unbinned_anchor_enabled() {
        symbols.push("line");
    }
    if multifidelity_ptx_enabled() {
        // AOT CUDA loader vector must include all line symbols: stock, cheap,
        // compact, and packed compact exact. Omitting one can
        // load the image but fail get_func/runtime when a prototype is selected.
        symbols.push("line_multifidelity_cheap_w1");
        symbols.push("line_multifidelity_compact_w1");
        symbols.push("line_multifidelity_compact_packed_w1");
    }
    if multifidelity_cartesian_unbinned_anchor_enabled() {
        // W2 loads only the exact cubin, but role attestation also binds the PTX bytes.
        // Keep the const-folded fallback artifact embedded for byte-for-byte verification.
        std::hint::black_box(SCATTER_PTX);
        noise_gpu::load_embedded_cubin_exact(
            &dev,
            SCATTER_CUBIN,
            SCATTER_CUBIN_SHA256,
            "s",
            &symbols,
        )
        .expect("load required candidate AOT cubin for Cartesian exact anchors");
    } else {
        noise_gpu::load_embedded_cubin_or_ptx(&dev, SCATTER_CUBIN, SCATTER_PTX, "s", &symbols)
            .expect("load scatter cubin or PTX fallback");
    }
    let stock = dev.get_func("s", "line_binned_fused").expect("stock fn");
    let cartesian_unbinned_exact = multifidelity_cartesian_unbinned_anchor_enabled().then(|| {
        dev.get_func("s", "line")
            .expect("candidate unbinned exact fn")
    });
    let multifidelity_cheap = multifidelity_ptx_enabled().then(|| {
        dev.get_func("s", "line_multifidelity_cheap_w1")
            .expect("cheap fn")
    });
    let multifidelity_compact = multifidelity_ptx_enabled().then(|| {
        dev.get_func("s", "line_multifidelity_compact_w1")
            .expect("compact fn")
    });
    let multifidelity_compact_packed = multifidelity_ptx_enabled().then(|| {
        dev.get_func("s", "line_multifidelity_compact_packed_w1")
            .expect("packed compact fn")
    });
    (
        dev,
        LineFunctions {
            stock,
            cartesian_unbinned_exact,
            multifidelity_cheap,
            multifidelity_compact,
            multifidelity_compact_packed,
        },
    )
}

/// Heartbeat so a multi-block region build is observable, not a silent wait.
pub(crate) struct Progress {
    pub(crate) done: usize,
    pub(crate) total: usize,
    pub(crate) last_beat: Instant,
}

impl Progress {
    pub(crate) fn tick(&mut self) {
        self.done += 1;
        if self.last_beat.elapsed().as_secs() >= 30 {
            if self.total > 0 {
                eprintln!(
                    "  … {}/{} tile-layers ({:.0}%)",
                    self.done,
                    self.total,
                    self.done as f64 / self.total as f64 * 100.0
                );
            } else {
                eprintln!("  … {} tile-layers built (stream)", self.done);
            }
            self.last_beat = Instant::now();
        }
    }
}
