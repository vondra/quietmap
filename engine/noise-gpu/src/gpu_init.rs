//! One-time setup for the gpu-surface batch runner — the items built once and
//! then handed to the hot kernel-launch loop in `gpu_surface.rs`: the `LineLayer`
//! road/rail descriptor (parse, dir, source_id, halo reach, row loader), the warm
//! CUDA device + scatter-PTX module loaders (`warm_device` / `warm_device_on`),
//! the `Progress` heartbeat, and the `NOISE_GPU_TIMING` gate. No per-tile/launch
//! state lives here; that stays in `gpu_surface.rs`.
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use cudarc::driver::{CudaDevice, CudaFunction};
use cudarc::nvrtc::Ptx;
use h3o::{CellIndex, LatLng};
use noise_compute::admin;
use noise_compute::constants::RAILWAY_REACH_CEILING;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::wire_hm3::{SOURCE_ID_RAIL, SOURCE_ID_ROAD};

const SCATTER_PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/scatter.ptx"));
const ROAD_HALO_M: f64 = 10_000.0; // motorway-class reach (matches build_heatmap_surface)

/// `NOISE_GPU_TIMING=1` → bracket every line-kernel launch with CUDA events and
/// emit a `KERNEL_MS=<total>` line (the optimisation harness's median-of-N signal,
/// isolating the kernel from the htod/dtoh copies the host-wall `t_kernel` folds
/// in). Read once: a per-launch env lookup would add host overhead to the very
/// thing being timed. OFF (the default) ⇒ no event create/record/sync at all, so
/// production throughput is untouched.
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
    pub(crate) const fn h0_abi_tag(self) -> usize {
        match self {
            Self::Road => 0,
            Self::Rail => 1,
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

/// CUDA device + scatter PTX, created once — shared by the batch path and --stream (the warm
/// context the cluster used to re-pay on every chunk spawn).
pub(crate) fn warm_device() -> (Arc<CudaDevice>, CudaFunction) {
    warm_device_on(false)
}

/// `with_stream` ⇒ `CudaDevice::new_with_stream` (own stream, shared primary context) so the N
/// --stream workers OVERLAP on the GPU (the gpu_airborne pattern, airborne.rs:252); `false` ⇒ the
/// null-stream device the serial batch path uses. Each call gets its own scatter-PTX module load.
pub(crate) fn warm_device_on(with_stream: bool) -> (Arc<CudaDevice>, CudaFunction) {
    let dev = if with_stream {
        CudaDevice::new_with_stream(0).expect("cuda")
    } else {
        CudaDevice::new(0).expect("cuda")
    };
    dev.load_ptx(Ptx::from_src(SCATTER_PTX), "s", &["line_binned_fused"])
        .expect("ptx");
    let f = dev.get_func("s", "line_binned_fused").expect("fn");
    (dev, f)
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
