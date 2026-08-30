//! GPU surface heatmap batch runner for the LINE layers (road + rail) — the
//! production wiring of the binned scatter kernel. Road and rail are both
//! `LineRow` sources feeding the identical CNOSSOS line-source physics, so one
//! kernel (`line_binned`) serves both; only the loader, halo reach, and HM3
//! source_id differ. Builds one tile block's shared 10 km halo once, then per
//! tile per layer: load rows, bin sources per 8×8 block, run the kernel,
//! collapse to Lden u8, write `{output}/{layer}/{z}/x/y.bin` and (if a baseline
//! exists) diff it. Reports per-layer throughput.
//!
//!   # one grid-aligned block (dev/bench), diff vs baseline:
//!   NOISE_GPU_BASELINE=/root/baseline gpu-surface --layers rail 4510 2786 4
//!   # a whole region (production), road+rail → HM3:
//!   NOISE_GPU_PREPARED=/dev/shm/qmap/prepared DATA_YEAR=2026 \
//!     gpu-surface --layers road,rail --bbox 38.27,-9.78,39.17,-8.50 --output OUT
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use cudarc::driver::sys::CUevent_flags;
use cudarc::driver::{
    result, CudaDevice, CudaFunction, CudaSlice, CudaStream, DevicePtrMut, LaunchAsync,
    LaunchConfig,
};
use h3o::CellIndex;
use noise_compute::admin;
use noise_gpu::{
    pack_sources, pack_tile, upload_obstacles, ObstDev, SourceBuffers, SurfaceKernelTileParameters,
    TileBuffers, BIN_W, N_BINS,
};
use raster_reader::fused_tile_z13::{default_batch_size, TileBatch};
use raster_reader::RealRasters;
use rayon::prelude::*;
use silent_tile_census::{build_reach_census, drop_unreachable_tiles, unlink_stale_tile};
use tile_painter::accumulator::{TileAccumulator, NUM_PERIODS};
use tile_painter::engine_spans::EngineCellSpans;
use tile_painter::grid::{tile_range, TILE_PX};
use tile_painter::region_runner::{
    announce_stream_cell_started, batch_slot, block_batch_origin, read_r4_file, region_tiles,
    split_configured_layers, split_stream_line, tile_centre_r4,
};
use tile_painter::renderer_evidence::{
    maybe_run_static_attestation, DependencyProfile, RegionTerminalStatus, RendererEvidence,
    RuntimeParameters, StaticAttestationParameters, RENDERER_EVIDENCE_FLAG,
    RENDERER_STATIC_ATTEST_FLAG,
};
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::{InteriorEstimate, ObstacleData};
use tile_painter::wire_hm3::{collapse_lden_surface_u8, read_tile, write_tile};

// One-time GPU/layer setup lives in the sibling `gpu_init` module; the hot
// kernel-launch path (process_block/region, run_stream, main) stays here.
#[path = "gpu_init.rs"]
mod gpu_init;
mod silent_tile_census;
use gpu_init::{
    multifidelity_cartesian_unbinned_anchor_enabled, timing_enabled, warm_device, warm_device_on,
    LineFunctions, LineLayer, Progress,
};

const NO_DATA: u8 = 255;
// `meta[9]`: since the surface kernel moved to byte-space stopping this is an
// ON/OFF, not a threshold — non-zero means "stop each pixel once its HM3 byte is
// decided", 0 means "compute every pair". It keeps the η name and the 0.40 value
// so one env (`SURFACE_BUDGET_ETA=0`) still puts BOTH lanes on the exact path,
// which is what the e2 parity gate needs. See `scatter_band::byte_stop_enabled`.
const ETA: f64 = 0.40;
const TW: f64 = 8.0; // pack_tile swizzle width — the binned kernel ignores it (only
                     // the un-binned `rail` bench kernel in e2-full swizzles by it)

#[cfg(feature = "v2-h0")]
fn h0_exact_counter(output: &[f32], index: usize) -> u64 {
    assert!(index < noise_gpu::OUT_H0_COUNTERS);
    let first_slot = noise_gpu::OUT_H0_COUNTER_BYTE_OFFSET / std::mem::size_of::<f32>()
        + index * (std::mem::size_of::<u64>() / std::mem::size_of::<f32>());
    u64::from(output[first_slot].to_bits()) | (u64::from(output[first_slot + 1].to_bits()) << 32)
}

/// Process-wide byte budget for host-resident tile blocks: bounds building and
/// ready blocks across all stream workers and both buffer halves for every
/// worker. A per-worker block-count window is not a memory bound (2 workers ×
/// current+next × window ⇒ up to
/// 8 batches). Blocks reserve their exact pre-build size (the same
/// `FusedGrid::grid_dims` math the build allocates with), correct it to the
/// measured size after building, and release when the GPU loop drops them.
struct PipelineByteGate {
    held: std::sync::Mutex<u64>,
    cv: std::sync::Condvar,
    budget: u64,
}

/// RAII reservation of gate bytes for ONE whole chunk. Acquired by the
/// builder BEFORE any block of the chunk is built (one permit per chunk —
/// per-block permits inside a rayon collect deadlock the moment a chunk's
/// aggregate exceeds the budget: finished blocks hold bytes while a sibling
/// waits, and nothing releases until the whole collect returns. Dropping the
/// permit releases — panic-safe.
struct ChunkPermit {
    bytes: u64,
}

impl ChunkPermit {
    fn adjust_to(&mut self, measured: u64) {
        pipeline_gate().adjust(self.bytes, measured);
        self.bytes = measured;
    }
}

impl Drop for ChunkPermit {
    fn drop(&mut self) {
        pipeline_gate().release(self.bytes);
    }
}

impl PipelineByteGate {
    /// Block until `bytes` fits in the budget, then reserve them. A request
    /// larger than the whole budget is admitted once nothing else is held —
    /// one chunk always makes progress. Deadlock-freedom: only BUILDER
    /// threads wait here; GPU workers never `acquire`. The last
    /// `Arc<ChunkPermit>` drop (a worker, after the slowest block of the
    /// chunk) releases; that thread cannot wait on this gate, so a builder
    /// waiting for space is never waited on by the thread that owns it.
    fn acquire(&self, bytes: u64) -> ChunkPermit {
        let mut held = self.held.lock().unwrap();
        while *held > 0 && *held + bytes > self.budget {
            held = self.cv.wait(held).unwrap();
        }
        *held += bytes;
        ChunkPermit { bytes }
    }

    /// Correct a reservation from the pre-build estimate to the measured
    /// size. Never blocks (the bytes are already resident); shrinking wakes
    /// waiters. Saturating like `release` — accounting drift must degrade to
    /// a too-loose gate, never wrap into a stream hang.
    fn adjust(&self, from: u64, to: u64) {
        let mut held = self.held.lock().unwrap();
        *held = held.saturating_sub(from).saturating_add(to);
        if to < from {
            self.cv.notify_all();
        }
    }

    fn release(&self, bytes: u64) {
        let mut held = self.held.lock().unwrap();
        *held = held.saturating_sub(bytes);
        self.cv.notify_all();
    }
}

/// `NOISE_GPU_PIPELINE_MB` (default 3072) sizes the gate. 30 m regions run
/// ~120–300 MB/block, so the default keeps today's overlap; 10 m-field
/// regions (~9× halo bytes) self-limit to ~1–2 resident blocks — the z13
/// plan's E1 acceptance — with no per-resolution configuration.
fn pipeline_gate() -> &'static PipelineByteGate {
    static GATE: std::sync::OnceLock<PipelineByteGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| PipelineByteGate {
        held: std::sync::Mutex::new(0),
        cv: std::sync::Condvar::new(),
        budget: std::env::var("NOISE_GPU_PIPELINE_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&mb| mb >= 1)
            .unwrap_or(3072)
            * (1 << 20),
    })
}

thread_local! {
    /// One `RealRasters` per rayon worker, REUSED across every region/cell. This is a
    /// DELIBERATE, twice-decided design — do not "fix" it to a single shared store again:
    ///
    /// 2026-07-15 morning, a shared process-wide store replaced this to stop an aggregate
    /// mmap leak (48 threads × per-thread caps pinned ~12 GB of tmpfs, one tile mapped 31×,
    /// two boxes starved 8h/2h because the agent's evict() rightly refuses to delete mapped
    /// files). The leak diagnosis was right; the fix location was wrong: sharing put slot
    /// Mutexes + LRU touch atomics onto the crop hot path, and with 24-48 rayon workers
    /// sampling millions of pixels the cache-line traffic gutted crop throughput — GPUs
    /// dropped from saturated to ~30-50% duty in 4-7s bursts (owner caught it on the
    /// dashboard within the hour; CPU read "96% busy" doing coherence work). Reverted the
    /// same day: per-thread stores have ZERO cross-thread synchronization, which is what
    /// line-rate cropping actually needs — the L2/cache-locality argument, per the owner.
    ///
    /// The DISK side of the original incident is solved in the right layers instead:
    /// the agent's lease budget counts evictable cache as free (economics fix), and its
    /// starved self-heal recycles a pinned-full engine within ~3 minutes (fresh process =
    /// zero mmaps = evict can finally reclaim) — the 8-hour silent stall cannot recur.
    static RASTERS: RefCell<Option<RealRasters>> = const { RefCell::new(None) };
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

/// Whether this binary was built with the W1-only multifidelity PTX arm.
/// `build.rs` derives the marker from the exact `NOISE_GPU_DEFINES` token, so
/// the host cannot accidentally pair stock output allocation with candidate
/// reconstruction at runtime.
fn multifidelity_line_enabled() -> bool {
    option_env!("NOISE_GPU_MULTIFIDELITY_LINE") == Some("1")
}

/// The z13 W1 lift is a compile-time profile, not a runtime tuning knob. The
/// marker pair is emitted only after `build.rs` has accepted the exact define
/// values, so a binary cannot claim a stride that its PTX did not compile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MultifidelityZ13Profile {
    stride: MultifidelityStride,
}

fn multifidelity_z13_profile() -> Result<Option<MultifidelityZ13Profile>> {
    match (
        option_env!("NOISE_GPU_MULTIFIDELITY_Z13_STRIDE"),
        option_env!("NOISE_GPU_MULTIFIDELITY_Z13_ADAPTIVE"),
    ) {
        (None, None) => Ok(None),
        (Some(stride), Some("0")) => {
            let pixels: usize = stride
                .parse()
                .with_context(|| format!("invalid compiled z13 stride marker `{stride}`"))?;
            let stride = MultifidelityStride::from_pixels(pixels)
                .with_context(|| format!("unsupported compiled z13 stride marker `{stride}`"))?;
            Ok(Some(MultifidelityZ13Profile { stride }))
        }
        (Some(stride), Some(adaptive)) => bail!(
            "compiled z13 profile has adaptive/replay={adaptive:?}; strict ladder requires 0 (stride {stride})"
        ),
        (Some(stride), None) => bail!(
            "compiled z13 profile stride {stride} is missing its adaptive=0 marker"
        ),
        (None, Some(adaptive)) => bail!(
            "compiled z13 profile has adaptive marker {adaptive:?} without a stride marker"
        ),
    }
}

/// Runtime anchor lattice used by the W1 candidate. The compact CUDA launch
/// receives an explicit record count; keeping the lattice here as a closed
/// value object makes axis construction, masks, reconstruction, and allocation
/// share one stride instead of a compile-time anchor-count ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MultifidelityStride {
    // Production picks stride 8 for both dense W1 roads and W1 rail; the W2 role
    // compiles its own stride in. Stride4 and Stride32 remain test fixtures for the
    // shared reconstruction machinery.
    #[allow(dead_code)]
    Stride4,
    Stride8,
    Stride16,
    #[allow(dead_code)]
    Stride32,
}

impl MultifidelityStride {
    const fn pixels(self) -> usize {
        match self {
            Self::Stride4 => 4,
            Self::Stride8 => 8,
            Self::Stride16 => 16,
            Self::Stride32 => 32,
        }
    }

    const fn from_pixels(pixels: usize) -> Option<Self> {
        match pixels {
            4 => Some(Self::Stride4),
            8 => Some(Self::Stride8),
            16 => Some(Self::Stride16),
            32 => Some(Self::Stride32),
            _ => None,
        }
    }

    fn anchor_axis(self) -> Vec<usize> {
        let mut axis: Vec<usize> = (0..TILE_PX).step_by(self.pixels()).collect();
        if axis.last().copied() != Some(TILE_PX - 1) {
            axis.push(TILE_PX - 1);
        }
        axis
    }

    fn anchor_count(self) -> usize {
        self.anchor_axis().len()
    }

    fn anchor_record_count(self) -> usize {
        self.anchor_count() * self.anchor_count()
    }

    fn compact_output_len(self) -> usize {
        self.anchor_record_count() * noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE
    }

    fn is_anchor(self, py: usize, px: usize) -> bool {
        (py.is_multiple_of(self.pixels()) || py == TILE_PX - 1)
            && (px.is_multiple_of(self.pixels()) || px == TILE_PX - 1)
    }
}

/// Inputs deliberately exposed to the pure candidate selector. This bounded
/// A/B selects either the active role's exact binned fallback or a reviewed
/// W1/z13 lattice from loaded region content, without coordinate allowlists,
/// runtime environment knobs, or a new launch ABI.
#[derive(Clone, Copy, Eq, PartialEq)]
struct MultifidelitySelectionInputs {
    layer: LineLayer,
    nsrc: usize,
    requested_stride: Option<MultifidelityStride>,
}

/// Use the active role's exact binned fallback for sparse roads, where exact work
/// stays bounded by the much smaller source set, and the denser stride8 for both
/// dense W1 roads and W1 rail (see the arm below for the measurements).
/// This is calibrated from normalized rows loaded from a region's
/// `grid_disk(1)`: the z12 W1 rings measured 3,125–5,987 road rows in Sahara
/// versus more than one million in Dobříš/Ruzyně. The 6,000-source boundary
/// covers exactly 23,615 of 86,666 regions in the 2026 census while touching
/// only 0.477% of world road source mass. A fixed content threshold keeps the
/// rule geographic-data driven and applies to unseen areas. `None` means the
/// role-exact binned entry; `Some` means multifidelity reconstruction.
const ROAD_SPARSE_STOCK_MAX_SOURCES: usize = 6_000;

fn select_multifidelity_stride(
    inputs: MultifidelitySelectionInputs,
) -> Option<MultifidelityStride> {
    match inputs.layer {
        LineLayer::Road if inputs.nsrc <= ROAD_SPARSE_STOCK_MAX_SOURCES => None,
        // Dense W1 road and W1 rail both take the denser lattice (sparse roads are
        // already exact, above). Measured on the four wbench-orig benchmark cells
        // against the frozen reference: road's >1 dB drift more than halves,
        // 25.195 % -> 12.082 %, back inside the contract's 20 % rung, with
        // >2 dB 11.651 -> 4.157 % and >6 dB 0.746 -> 0.197 %. The one regression is
        // road's single worst cell, max_abs_db 19.0 -> 19.5, one u8 step. Rail's own
        // 21.4 % -> 10.3 % came earlier with its lattice and is unchanged here.
        //
        // On those same four cells the seven layers paint concurrently and the total
        // wall moves 223.5 -> 224.7 s, since road's lane (162.5 -> 214.4 s) stays under
        // the pole. That is a benchmark figure, not a claim about any production
        // dispatch: a worker that paints road and rail serially pays the road increase
        // in full.
        //
        // `requested_stride` is Some only for z13 (see the z-guard near the profile
        // check), so the fallback below is the W1 lattice and cannot reach the W2 role.
        LineLayer::Road | LineLayer::Rail => Some(
            inputs
                .requested_stride
                .unwrap_or(MultifidelityStride::Stride8),
        ),
    }
}

/// Tile coordinates sampled by the selected runtime stride. The final pixel is
/// explicit because a stride lattice normally lands before the 511 boundary;
/// omitting it would leave the final interpolation interval without an exact
/// corner.
fn multifidelity_anchor_axis(stride: MultifidelityStride) -> Vec<usize> {
    stride.anchor_axis()
}

/// Launch the selected exact tail on top of the fixed anchor lattice. The
/// per-layer rule in [`multifidelity_receiver_mask_with_replay`] decides which
/// stride blocks earn it.
const MULTIFIDELITY_REPLAY_SELECTED_BLOCKS: bool = true;

/// Whether this layer's tiles can reach the selected exact tail at all. Road
/// cannot, see the rule's comment. This has to be answerable BEFORE the mask
/// exists, because the stop-event record below the anchor joins needs to know
/// whether anything will follow it; the mask alone would answer too late.
fn multifidelity_layer_replays(layer: LineLayer) -> bool {
    MULTIFIDELITY_REPLAY_SELECTED_BLOCKS && matches!(layer, LineLayer::Rail)
}

/// Decode the explicit compact output ABI. Every record carries its own dense
/// pixel index, so a stale count, short allocation, fractional index, or
/// out-of-tile write is rejected before reconstruction can turn it into a
/// tile. The device kernel performs the matching count/stride bounds check.
fn decode_multifidelity_compact_output(
    output: &[f32],
    record_count: usize,
) -> Result<Vec<(usize, [f32; 3], f32)>, String> {
    let stride = noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE;
    let expected = record_count
        .checked_mul(stride)
        .ok_or_else(|| "compact output length overflow".to_string())?;
    if output.len() != expected {
        return Err(format!(
            "compact output length {} != {} records × {} words",
            output.len(),
            record_count,
            stride
        ));
    }
    let mut decoded = Vec::with_capacity(record_count);
    let mut seen = vec![false; TILE_PX * TILE_PX];
    for (record, words) in output.chunks_exact(stride).enumerate() {
        let index_f = words[noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT];
        if !index_f.is_finite() || index_f < 0.0 || index_f.fract() != 0.0 {
            return Err(format!(
                "compact output record {record} has invalid dense index {index_f}"
            ));
        }
        let index = index_f as usize;
        if index >= TILE_PX * TILE_PX {
            return Err(format!(
                "compact output record {record} dense index {index} exceeds {}",
                TILE_PX * TILE_PX
            ));
        }
        if seen[index] {
            return Err(format!(
                "compact output record {record} repeats dense index {index}"
            ));
        }
        seen[index] = true;
        let energy_base = noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE;
        let energies = [
            words[energy_base],
            words[energy_base + 1],
            words[energy_base + 2],
        ];
        if energies
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(format!("compact output record {record} has invalid energy"));
        }
        let fault = words[noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_FAULT_SLOT];
        if !fault.is_finite() || fault < 0.0 {
            return Err(format!(
                "compact output record {record} has invalid fault count"
            ));
        }
        decoded.push((index, energies, fault));
    }
    Ok(decoded)
}

fn multifidelity_compact_fault_sum(output: &[f32], record_count: usize) -> Result<f32, String> {
    let records = decode_multifidelity_compact_output(output, record_count)?;
    Ok(records.into_iter().map(|(_, _, fault)| fault).sum())
}

fn add_multifidelity_fault_total_to_dense_slot(
    dense_output: &mut [f32],
    multifidelity_fault_total: f32,
) {
    dense_output[noise_gpu::OUT_FAULT_SLOT] += multifidelity_fault_total;
}

// The stride-4 axis is Cartesian: 129 latitude coordinates by 129 longitude
// coordinates. Nine groups of at most 16 latitude coordinates each reuse the
// first row of nine ordinary 16x16 unbinned blocks. The final block/group pads by
// repeating coordinate 511; all 256 lanes remain ordinary valid receivers and
// the repeated results are discarded on the host.
const MULTIFIDELITY_STOCK_CARTESIAN_AXIS_COORDINATES: usize = 129;
const MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_AXIS_COORDINATES.div_ceil(BIN_W);
const MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_AXIS_COORDINATES.div_ceil(BIN_W);
const MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS * BIN_W;
const MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_PIXELS_END: usize =
    (BIN_W - 1) * TILE_PX + MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS;
const MULTIFIDELITY_STOCK_CARTESIAN_RXAR_VALUES: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_PIXELS_END * 2;
const MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_PIXELS_END * NUM_PERIODS;
const MULTIFIDELITY_STOCK_CARTESIAN_COMPUTED_RECEIVERS: usize =
    MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS
        * MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS
        * BIN_W
        * BIN_W;
const MULTIFIDELITY_STOCK_CARTESIAN_META_BYTE_STOP_SLOT: usize = 9;
const MULTIFIDELITY_STOCK_CARTESIAN_META_TILE_WIDTH_SLOT: usize = 10;
const MULTIFIDELITY_STOCK_CARTESIAN_META_OUTPUT_SLOTS_SLOT: usize = 13;

const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS == 9);
const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS == 9);
const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS == 144);
const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_PIXELS_END == 7_824);
const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES <= noise_gpu::OUT_FAULT_SLOT);
const _: () = assert!(MULTIFIDELITY_STOCK_CARTESIAN_COMPUTED_RECEIVERS == 20_736);

#[derive(Clone, Debug, Eq, PartialEq)]
struct MultifidelityStockCartesianPlan {
    axis: Vec<usize>,
}

impl MultifidelityStockCartesianPlan {
    fn stride4() -> Result<Self, String> {
        let axis = multifidelity_anchor_axis(MultifidelityStride::Stride4);
        if axis.len() != MULTIFIDELITY_STOCK_CARTESIAN_AXIS_COORDINATES {
            return Err(format!(
                "stride4 stock Cartesian axis has {} coordinates, expected {}",
                axis.len(),
                MULTIFIDELITY_STOCK_CARTESIAN_AXIS_COORDINATES
            ));
        }
        if axis.first().copied() != Some(0) || axis.last().copied() != Some(TILE_PX - 1) {
            return Err("stride4 stock Cartesian axis omits a tile boundary".to_string());
        }
        Ok(Self { axis })
    }

    fn anchor_record_count(&self) -> usize {
        self.axis.len() * self.axis.len()
    }

    fn latitude_group_range(&self, group: usize) -> Result<std::ops::Range<usize>, String> {
        if group >= MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS {
            return Err(format!(
                "stock Cartesian latitude group {group} exceeds plan"
            ));
        }
        let start = group * BIN_W;
        Ok(start..(start + BIN_W).min(self.axis.len()))
    }

    fn source_dense_index(
        &self,
        group: usize,
        synthetic_y: usize,
        synthetic_x: usize,
    ) -> Result<usize, String> {
        if synthetic_y >= BIN_W {
            return Err(format!(
                "stock Cartesian synthetic row {synthetic_y} exceeds one stock block"
            ));
        }
        let synthetic_columns = MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS;
        if synthetic_x >= synthetic_columns {
            return Err(format!(
                "stock Cartesian synthetic column {synthetic_x} exceeds {synthetic_columns}"
            ));
        }
        let group_range = self.latitude_group_range(group)?;
        let axis_y = (group_range.start + synthetic_y).min(self.axis.len() - 1);
        let axis_x = synthetic_x.min(self.axis.len() - 1);
        Ok(self.axis[axis_y] * TILE_PX + self.axis[axis_x])
    }
}

fn multifidelity_stock_cartesian_exact_meta(source: &[f64]) -> Result<Vec<f64>, String> {
    if source.len() != noise_gpu::SURFACE_META_SLOTS {
        return Err(format!(
            "stock Cartesian metadata length {} != {}",
            source.len(),
            noise_gpu::SURFACE_META_SLOTS
        ));
    }
    if source[MULTIFIDELITY_STOCK_CARTESIAN_META_OUTPUT_SLOTS_SLOT]
        != noise_gpu::OUT_SLOTS_PROD as f64
    {
        return Err(format!(
            "stock Cartesian output ABI {} != {}",
            source[MULTIFIDELITY_STOCK_CARTESIAN_META_OUTPUT_SLOTS_SLOT],
            noise_gpu::OUT_SLOTS_PROD
        ));
    }
    let mut exact = source.to_vec();
    exact[MULTIFIDELITY_STOCK_CARTESIAN_META_BYTE_STOP_SLOT] = 0.0;
    exact[MULTIFIDELITY_STOCK_CARTESIAN_META_TILE_WIDTH_SLOT] = BIN_W as f64;
    Ok(exact)
}

/// Populate the dense-index receiver ABI consumed by the candidate's unbinned
/// entry. Only rows 0..16 and columns 0..144 are launched, so `rxar` ends after
/// that exact pointer footprint; the split latitude/longitude `rxll` ABI keeps
/// its canonical shape. Every launched padded lane duplicates a real boundary
/// receiver instead of carrying an invalid sentinel into the kernel.
fn pack_multifidelity_stock_cartesian_group(
    plan: &MultifidelityStockCartesianPlan,
    source_rxll: &[f64],
    source_rxar: &[f32],
    group: usize,
) -> Result<(Vec<f64>, Vec<f32>), String> {
    if source_rxll.len() != TILE_PX * 2 {
        return Err(format!(
            "stock Cartesian rxll length {} != {}",
            source_rxll.len(),
            TILE_PX * 2
        ));
    }
    if source_rxar.len() != TILE_PX * TILE_PX * 2 {
        return Err(format!(
            "stock Cartesian rxar length {} != {}",
            source_rxar.len(),
            TILE_PX * TILE_PX * 2
        ));
    }
    plan.latitude_group_range(group)?;

    let mut packed_rxll = vec![0.0f64; TILE_PX * 2];
    for (synthetic_y, packed) in packed_rxll[..TILE_PX].iter_mut().enumerate() {
        let bounded_y = synthetic_y.min(BIN_W - 1);
        let source_index = plan.source_dense_index(group, bounded_y, 0)?;
        *packed = source_rxll[source_index / TILE_PX];
    }
    for synthetic_x in 0..TILE_PX {
        let bounded_x = synthetic_x.min(MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS - 1);
        let source_index = plan.source_dense_index(group, 0, bounded_x)?;
        packed_rxll[TILE_PX + synthetic_x] = source_rxll[TILE_PX + source_index % TILE_PX];
    }

    let mut packed_rxar = vec![0.0f32; MULTIFIDELITY_STOCK_CARTESIAN_RXAR_VALUES];
    for synthetic_y in 0..BIN_W {
        for synthetic_x in 0..MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS {
            let source_index = plan.source_dense_index(group, synthetic_y, synthetic_x)?;
            let packed_index = synthetic_y * TILE_PX + synthetic_x;
            let values = [
                source_rxar[source_index * 2],
                source_rxar[source_index * 2 + 1],
            ];
            if !source_rxll[source_index / TILE_PX].is_finite()
                || !source_rxll[TILE_PX + source_index % TILE_PX].is_finite()
                || values.iter().any(|value| !value.is_finite())
            {
                return Err(format!(
                    "stock Cartesian receiver {source_index} contains non-finite input"
                ));
            }
            packed_rxar[packed_index * 2..packed_index * 2 + 2].copy_from_slice(&values);
        }
    }
    Ok((packed_rxll, packed_rxar))
}

#[cfg(test)]
fn multifidelity_stock_cartesian_output_prefix_sentinel() -> Vec<f32> {
    vec![f32::NAN; MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES]
}

fn multifidelity_stock_cartesian_compact_output_sentinel(
    plan: &MultifidelityStockCartesianPlan,
) -> Vec<f32> {
    let mut output =
        vec![f32::NAN; plan.anchor_record_count() * noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE];
    for record in output.chunks_exact_mut(noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE) {
        record[noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT] = -1.0;
    }
    output
}

/// Convert one unbinned dense output back to the existing compact host ABI. Each
/// compact index begins at -1 and each dense energy begins at NaN, so a missed
/// group, missed lane, repeated group, or partial launch fails decoding.
fn extract_multifidelity_stock_cartesian_group(
    plan: &MultifidelityStockCartesianPlan,
    group: usize,
    dense_output: &[f32],
    dense_fault: f32,
    compact_output: &mut [f32],
) -> Result<f32, String> {
    if dense_output.len() != MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES {
        return Err(format!(
            "stock Cartesian launched output length {} != {}",
            dense_output.len(),
            MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES
        ));
    }
    let compact_stride = noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE;
    let expected_compact_len = plan
        .anchor_record_count()
        .checked_mul(compact_stride)
        .ok_or_else(|| "stock Cartesian compact output length overflow".to_string())?;
    if compact_output.len() != expected_compact_len {
        return Err(format!(
            "stock Cartesian compact output length {} != {expected_compact_len}",
            compact_output.len()
        ));
    }

    // The unbinned kernel launched every 16x144 lane, including the duplicated
    // Cartesian padding. Validate the complete launched rectangle before
    // discarding those lanes; otherwise a partial block or a stale padding
    // write can pass through the exact-anchor receipt unnoticed.
    let launched_columns = MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS;
    for synthetic_y in 0..BIN_W {
        for synthetic_x in 0..launched_columns {
            let source_energy = (synthetic_y * TILE_PX + synthetic_x) * NUM_PERIODS;
            let energies = &dense_output[source_energy..source_energy + NUM_PERIODS];
            if energies
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                return Err(format!(
                    "stock Cartesian launched lane ({synthetic_y},{synthetic_x}) has an unwritten or invalid energy"
                ));
            }
        }
    }

    let group_range = plan.latitude_group_range(group)?;
    for (synthetic_y, axis_y) in group_range.enumerate() {
        for axis_x in 0..plan.axis.len() {
            let source_energy = (synthetic_y * TILE_PX + axis_x) * NUM_PERIODS;
            let dense_index = plan.axis[axis_y] * TILE_PX + plan.axis[axis_x];
            let record = axis_y * plan.axis.len() + axis_x;
            let target = record * compact_stride;
            if compact_output[target + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT] != -1.0 {
                return Err(format!(
                    "stock Cartesian anchor record {record} was written more than once"
                ));
            }
            let energies = &dense_output[source_energy..source_energy + NUM_PERIODS];
            compact_output[target + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT] =
                dense_index as f32;
            let energy_base = target + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE;
            compact_output[energy_base..energy_base + NUM_PERIODS].copy_from_slice(energies);
            compact_output[target + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_FAULT_SLOT] = 0.0;
        }
    }

    let fault = dense_fault;
    if !fault.is_finite() || fault < 0.0 {
        return Err(format!(
            "stock Cartesian group {group} has invalid fault count {fault}"
        ));
    }
    Ok(fault)
}

/// Padding receivers duplicate physical boundary pixels, but the unbinned
/// kernel exposes only one launch-global ARC fault counter. A nonzero Cartesian
/// total therefore cannot be attributed to real anchors without a per-lane
/// fault ABI; fail closed before it is mixed into the dense receipt.
fn require_zero_multifidelity_stock_cartesian_fault(fault_total: f32) -> Result<(), String> {
    if fault_total == 0.0 {
        Ok(())
    } else {
        Err(format!(
            "stock Cartesian exact launch reported {fault_total:.0} ARC faults; refusing to mix the aggregate because duplicate padding lanes are not dense-equivalent"
        ))
    }
}

/// W2's per-call pre-clip arc union may increase fixed-capacity interval demand.
/// Its exact Cartesian anchors already reject every ARC fault; apply the same
/// fail-closed policy to launch A before its dense cheap field can be
/// reconstructed and written. Stock and the accepted W1 role retain their
/// historical loud-warning policy.
fn require_zero_w2_dense_arc_fault(w2_profile: bool, fault_delta: f32) -> Result<(), String> {
    if !w2_profile || fault_delta == 0.0 {
        Ok(())
    } else {
        Err(format!(
            "W2 stride4 dense launch A reported {fault_delta:.0} ARC faults; refusing to reconstruct an under-screened tile"
        ))
    }
}

fn validate_multifidelity_stock_cartesian_output(
    plan: &MultifidelityStockCartesianPlan,
    compact_output: &[f32],
) -> Result<(), String> {
    let decoded = decode_multifidelity_compact_output(compact_output, plan.anchor_record_count())?;
    for (record, ((dense_index, _, _), expected_index)) in decoded
        .into_iter()
        .zip(
            plan.axis
                .iter()
                .flat_map(|&py| plan.axis.iter().map(move |&px| py * TILE_PX + px)),
        )
        .enumerate()
    {
        if dense_index != expected_index {
            return Err(format!(
                "stock Cartesian record {record} index {dense_index} != {expected_index}"
            ));
        }
    }
    Ok(())
}

struct MultifidelityStockCartesianDeviceGroup {
    group: usize,
    rxll: CudaSlice<f64>,
    rxar: CudaSlice<f32>,
    output: CudaSlice<f32>,
}

fn allocate_multifidelity_stock_cartesian_output(
    dev: &Arc<CudaDevice>,
    group: usize,
) -> Result<CudaSlice<f32>> {
    let mut output = dev
        .alloc_zeros::<f32>(noise_gpu::OUT_SLOTS_PROD)
        .with_context(|| format!("allocate stock Cartesian group {group} output"))?;
    let mut launched_prefix = output.slice_mut(..MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES);
    // The kernel writes only the launched 16x144 prefix plus the distant global
    // fault slot. 0xff in every byte is an IEEE-754 NaN, preserving the exact
    // unwritten-lane sentinel without uploading the unused 3 MiB dense tail;
    // alloc_zeros keeps the fault counter at its required initial 0.0.
    unsafe {
        result::memset_d8_async(
            *launched_prefix.device_ptr_mut(),
            0xff,
            MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES * std::mem::size_of::<f32>(),
            *dev.cu_stream(),
        )
    }
    .with_context(|| format!("initialize stock Cartesian group {group} output sentinel"))?;
    Ok(output)
}

fn prepare_multifidelity_stock_cartesian_device_groups(
    dev: &Arc<CudaDevice>,
    plan: &MultifidelityStockCartesianPlan,
    source_rxll: &[f64],
    source_rxar: &[f32],
) -> Result<Vec<MultifidelityStockCartesianDeviceGroup>> {
    (0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS)
        .map(|group| {
            let (rxll, rxar) =
                pack_multifidelity_stock_cartesian_group(plan, source_rxll, source_rxar, group)
                    .map_err(anyhow::Error::msg)?;
            Ok(MultifidelityStockCartesianDeviceGroup {
                group,
                rxll: dev
                    .htod_copy(rxll)
                    .with_context(|| format!("upload stock Cartesian group {group} rxll"))?,
                rxar: dev
                    .htod_copy(rxar)
                    .with_context(|| format!("upload stock Cartesian group {group} rxar"))?,
                output: allocate_multifidelity_stock_cartesian_output(dev, group)?,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn launch_multifidelity_stock_cartesian_device_groups(
    function: &CudaFunction,
    streams: &[CudaStream],
    groups: &mut [MultifidelityStockCartesianDeviceGroup],
    d_elev: &CudaSlice<f32>,
    d_inner: &CudaSlice<f32>,
    d_cover: &CudaSlice<u8>,
    d_meta: &CudaSlice<f64>,
    d_seg: &CudaSlice<f64>,
    d_sp: &CudaSlice<f64>,
    d_semis: &CudaSlice<f32>,
    d_barr: &CudaSlice<f64>,
    d_obstacles: &CudaSlice<u64>,
) -> Result<()> {
    assert_eq!(
        groups.len(),
        MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS,
        "stock Cartesian launch must contain every latitude group"
    );
    assert_eq!(
        streams.len(),
        groups.len(),
        "each stock Cartesian group requires one independent CUDA stream"
    );
    let launch_config = LaunchConfig {
        grid_dim: (MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS as u32, 1, 1),
        block_dim: ((BIN_W * BIN_W) as u32, 1, 1),
        shared_mem_bytes: 0,
    };
    // Each wave has private rxll/rxar/output allocations. All other arguments
    // are immutable, so the nine streams cannot race. The streams were forked
    // only after every H2D upload; their initial default-stream dependency makes
    // those inputs visible before any wave begins.
    for (expected_group, (stream, group)) in streams.iter().zip(groups.iter_mut()).enumerate() {
        assert_eq!(group.group, expected_group, "stock Cartesian group order");
        unsafe {
            function
                .clone()
                .launch_on_stream(
                    stream,
                    launch_config,
                    noise_gpu::line_kernel_arguments!(
                        d_elev,
                        d_inner,
                        d_cover,
                        d_meta,
                        d_seg,
                        d_sp,
                        d_semis,
                        &group.rxll,
                        &group.rxar,
                        d_barr,
                        d_obstacles,
                        &mut group.output,
                    ),
                )
                .with_context(|| {
                    format!("launch candidate Cartesian unbinned anchor group {expected_group}")
                })?;
        }
    }
    Ok(())
}

fn download_multifidelity_stock_cartesian_device_groups(
    dev: &Arc<CudaDevice>,
    plan: &MultifidelityStockCartesianPlan,
    groups: &[MultifidelityStockCartesianDeviceGroup],
) -> Result<(Vec<f32>, f32)> {
    let mut compact_output = multifidelity_stock_cartesian_compact_output_sentinel(plan);
    let mut fault_total = 0.0f32;
    for (expected_group, group) in groups.iter().enumerate() {
        anyhow::ensure!(
            group.group == expected_group,
            "stock Cartesian device group {} appears at {expected_group}",
            group.group
        );
        let dense_output = dev
            .dtoh_sync_copy(
                &group
                    .output
                    .slice(..MULTIFIDELITY_STOCK_CARTESIAN_OUTPUT_VALUES),
            )
            .with_context(|| {
                format!(
                    "download stock Cartesian group {} launched output",
                    group.group
                )
            })?;
        let dense_fault = dev
            .dtoh_sync_copy(
                &group
                    .output
                    .slice(noise_gpu::OUT_FAULT_SLOT..noise_gpu::OUT_FAULT_SLOT + 1),
            )
            .with_context(|| format!("download stock Cartesian group {} fault", group.group))?[0];
        let fault = extract_multifidelity_stock_cartesian_group(
            plan,
            group.group,
            &dense_output,
            dense_fault,
            &mut compact_output,
        )
        .map_err(anyhow::Error::msg)?;
        fault_total += fault;
        anyhow::ensure!(
            fault_total.is_finite(),
            "stock Cartesian aggregate fault count overflowed"
        );
    }
    require_zero_multifidelity_stock_cartesian_fault(fault_total).map_err(anyhow::Error::msg)?;
    validate_multifidelity_stock_cartesian_output(plan, &compact_output)
        .map_err(anyhow::Error::msg)?;
    Ok((compact_output, fault_total))
}

/// Pack dense receiver inputs into the compact kernel's explicit four-word
/// record ABI. The compact receiver pass never receives or indexes the dense `rxll`
/// or `rxar` arrays; this host-side conversion is the only dense read.
fn pack_multifidelity_compact_receivers(
    rxll: &[f64],
    rxar: &[f32],
    dense_indices: &[usize],
) -> Result<Vec<u64>, String> {
    if rxll.len() != TILE_PX * 2 {
        return Err(format!("rxll length {} != {}", rxll.len(), TILE_PX * 2));
    }
    if rxar.len() != TILE_PX * TILE_PX * 2 {
        return Err(format!(
            "rxar length {} != {}",
            rxar.len(),
            TILE_PX * TILE_PX * 2
        ));
    }
    let words = dense_indices
        .len()
        .checked_mul(noise_gpu::MULTIFIDELITY_COMPACT_RECEIVER_RECORD_WORDS)
        .ok_or_else(|| "compact receiver length overflow".to_string())?;
    let mut packed = Vec::with_capacity(words);
    for &dense_index in dense_indices {
        if dense_index >= TILE_PX * TILE_PX {
            return Err(format!("compact receiver index {dense_index} exceeds tile"));
        }
        let py = dense_index / TILE_PX;
        let px = dense_index % TILE_PX;
        let lat = rxll[py];
        let lon = rxll[TILE_PX + px];
        let altitude = rxar[dense_index * 2];
        let reflection = rxar[dense_index * 2 + 1];
        if !lat.is_finite() || !lon.is_finite() || !altitude.is_finite() || !reflection.is_finite()
        {
            return Err(format!(
                "compact receiver {dense_index} contains non-finite input"
            ));
        }
        packed.extend_from_slice(&[
            lat.to_bits(),
            lon.to_bits(),
            u64::from(altitude.to_bits()) | (u64::from(reflection.to_bits()) << 32),
            dense_index as u64,
        ]);
    }
    Ok(packed)
}

fn multifidelity_compact_control(record_count: usize) -> Vec<u64> {
    assert!(record_count <= TILE_PX * TILE_PX);
    vec![
        record_count as u64,
        noise_gpu::MULTIFIDELITY_COMPACT_ABI_VERSION as u64,
        noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE as u64,
    ]
}

struct MultifidelityInterpolation {
    stride: MultifidelityStride,
    cheap_accum: TileAccumulator,
    cheap_cells: Vec<u8>,
    exact_accum: TileAccumulator,
    /// Per-anchor exact energy, indexed by the anchor lattice rather than by
    /// the dense tile. Zero is a real silent period, not a missing record.
    exact_anchor_energy: Vec<[f64; NUM_PERIODS]>,
    /// Multiplicative exact/cheap correction in natural-log energy space. A
    /// period is valid only when both anchor energies are strictly positive;
    /// zero-sided corners use the linear-energy fallback below.
    log_energy_correction: Vec<[f64; NUM_PERIODS]>,
    log_energy_correction_valid: Vec<[bool; NUM_PERIODS]>,
    residual: Vec<f64>,
    valid: Vec<bool>,
    axis: Vec<usize>,
    lower: Vec<usize>,
    upper: Vec<usize>,
    fraction: Vec<f64>,
}

/// Decode launch-A's cheap field and compact exact fixed-stride anchor output
/// once on the host. The receiver mask and final HM3 reconstruction both
/// consume this object; no reference tile or popup oracle enters either path.
fn multifidelity_interpolation(
    stride: MultifidelityStride,
    cheap_gpu: &[f32],
    exact_anchor_output: &[f32],
) -> MultifidelityInterpolation {
    assert!(cheap_gpu.len() >= noise_gpu::OUT_SLOTS_MULTIFIDELITY);
    let exact =
        decode_multifidelity_compact_output(exact_anchor_output, stride.anchor_record_count())
            .unwrap_or_else(|error| panic!("invalid compact anchor output: {error}"));
    let mut cheap_accum = TileAccumulator::new();
    cheap_accum
        .energy
        .copy_from_slice(&cheap_gpu[..noise_gpu::OUT_ENERGY_SLOTS]);

    let axis = multifidelity_anchor_axis(stride);
    assert_eq!(axis.len(), stride.anchor_count());
    let mut exact_accum = TileAccumulator::new();
    for (dense_index, energies, _) in exact {
        let py = dense_index / TILE_PX;
        let px = dense_index % TILE_PX;
        assert!(multifidelity_is_anchor(stride, py, px));
        let target = dense_index * NUM_PERIODS;
        exact_accum.energy[target..target + NUM_PERIODS].copy_from_slice(&energies);
    }
    let anchor_count = axis.len() * axis.len();
    let mut exact_anchor_energy = vec![[0.0f64; NUM_PERIODS]; anchor_count];
    let mut log_energy_correction = vec![[0.0f64; NUM_PERIODS]; anchor_count];
    let mut log_energy_correction_valid = vec![[false; NUM_PERIODS]; anchor_count];
    for (ay, &py) in axis.iter().enumerate() {
        for (ax, &px) in axis.iter().enumerate() {
            let dense_index = py * TILE_PX + px;
            let anchor = ay * axis.len() + ax;
            let target = dense_index * NUM_PERIODS;
            for period in 0..NUM_PERIODS {
                let exact = f64::from(exact_accum.energy[target + period]);
                let cheap = f64::from(cheap_accum.energy[target + period]);
                // The compact decoder already rejects invalid exact values.
                // Keep the explicit finite/non-negative check here as the
                // reconstruction boundary: a malformed cheap f32 becomes a
                // silent fallback, never NaN/inf in a logarithm or tile.
                let exact = if exact.is_finite() && exact >= 0.0 {
                    exact
                } else {
                    0.0
                };
                exact_anchor_energy[anchor][period] = exact;
                if exact > 0.0 && cheap.is_finite() && cheap > 0.0 {
                    let ratio_log = (exact / cheap).ln();
                    if ratio_log.is_finite() {
                        log_energy_correction[anchor][period] = ratio_log;
                        log_energy_correction_valid[anchor][period] = true;
                    }
                }
            }
        }
    }
    let cheap_cells = collapse_lden_surface_u8(&cheap_accum);
    let exact_cells = collapse_lden_surface_u8(&exact_accum);
    let mut residual = vec![0.0f64; axis.len() * axis.len()];
    let mut valid = vec![false; axis.len() * axis.len()];
    for (ay, &py) in axis.iter().enumerate() {
        for (ax, &px) in axis.iter().enumerate() {
            let index = py * TILE_PX + px;
            let anchor = ay * axis.len() + ax;
            valid[anchor] = exact_cells[index] != tile_painter::wire_hm3::NO_DATA
                && cheap_cells[index] != tile_painter::wire_hm3::NO_DATA;
            if valid[anchor] {
                residual[anchor] = (exact_cells[index] as f64 - cheap_cells[index] as f64) / 2.0;
            }
        }
    }

    let mut lower = vec![0usize; TILE_PX];
    let mut upper = vec![0usize; TILE_PX];
    let mut fraction = vec![0.0f64; TILE_PX];
    for p in 0..TILE_PX {
        let lo = if p >= *axis.last().expect("anchor axis") {
            axis.len() - 2
        } else {
            axis.partition_point(|&anchor| anchor <= p)
                .saturating_sub(1)
        };
        let hi = lo + 1;
        lower[p] = lo;
        upper[p] = hi;
        fraction[p] = (p - axis[lo]) as f64 / (axis[hi] - axis[lo]) as f64;
    }

    MultifidelityInterpolation {
        stride,
        cheap_accum,
        cheap_cells,
        exact_accum,
        exact_anchor_energy,
        log_energy_correction,
        log_energy_correction_valid,
        residual,
        valid,
        axis,
        lower,
        upper,
        fraction,
    }
}

#[inline]
fn multifidelity_is_anchor(stride: MultifidelityStride, py: usize, px: usize) -> bool {
    stride.is_anchor(py, px)
}

#[inline]
fn multifidelity_morton_part(mut value: u32) -> u32 {
    value &= 0x1ff;
    value = (value | (value << 8)) & 0x00ff00ff;
    value = (value | (value << 4)) & 0x0f0f0f0f;
    value = (value | (value << 2)) & 0x33333333;
    (value | (value << 1)) & 0x55555555
}

#[inline]
fn multifidelity_morton_key(dense_index: usize) -> u32 {
    let py = (dense_index / TILE_PX) as u32;
    let px = (dense_index % TILE_PX) as u32;
    multifidelity_morton_part(px) | (multifidelity_morton_part(py) << 1)
}

// Keep this as one host-side knob so isolated source builds can compare compact
// envelope widths without changing the receiver/output ABI.
const MULTIFIDELITY_COMPACT_BUCKET_PX: usize = 32;
const MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK: usize = BIN_W * BIN_W;
/// The packed prototype keeps one independent source-cull envelope per warp.
/// `BIN_W` is 16, so eight 32-lane warps retain the existing 256-thread launch
/// and the compact receiver ABI without widening a warp's local receiver set.
const MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK: usize = 8;
const MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_WARP: usize = 32;
const MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_BLOCK: usize =
    MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK * MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_WARP;

const _: () = assert!(
    MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_BLOCK == MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MultifidelityCompactLaunch {
    record_offset: usize,
    record_count: usize,
}

#[derive(Clone, Debug, Default)]
struct MultifidelityCompactPlan {
    indices: Vec<usize>,
    launches: Vec<MultifidelityCompactLaunch>,
}

/// One 32-lane warp descriptor in the packed CUDA prototype. A zero
/// `record_count` is an explicit inactive tail descriptor; keeping the selected
/// compile-time descriptor count in every block makes the existing header +
/// offset/count control ABI usable without adding a control-length argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MultifidelityCompactPackedWarp {
    record_offset: usize,
    record_count: usize,
}

/// Packed compact plan: each CUDA block owns up to eight independent 32×32
/// receiver buckets, one bucket descriptor per warp. The dense receiver index
/// remains in each record, so output order and reconstruction are unchanged.
#[derive(Clone, Debug, Default)]
struct MultifidelityCompactPackedPlan {
    indices: Vec<usize>,
    launches: Vec<[MultifidelityCompactPackedWarp; MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK]>,
}

#[inline]
fn multifidelity_compact_bucket(dense_index: usize) -> (usize, usize) {
    (
        (dense_index / TILE_PX) / MULTIFIDELITY_COMPACT_BUCKET_PX,
        (dense_index % TILE_PX) / MULTIFIDELITY_COMPACT_BUCKET_PX,
    )
}

/// Order compact receivers by aligned bucket macrotiles and split each bucket
/// into independent ≤256-record block chunks. A block never mixes buckets, so
/// every conservative source-cull envelope stays within the bucket span per axis,
/// including the explicit 511 boundary receiver. Source traversal inside each
/// receiver remains ascending, and explicit dense indices make block order
/// irrelevant to reconstruction.
fn multifidelity_compact_plan(dense_indices: &[usize]) -> Result<MultifidelityCompactPlan, String> {
    let mut ordered = dense_indices.to_vec();
    if let Some(&index) = ordered.iter().find(|&&index| index >= TILE_PX * TILE_PX) {
        return Err(format!("compact receiver index {index} exceeds tile"));
    }
    ordered.sort_unstable_by_key(|&index| {
        (
            multifidelity_compact_bucket(index),
            multifidelity_morton_key(index),
            index,
        )
    });
    if let Some(pair) = ordered.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("compact receiver index {} is duplicated", pair[0]));
    }
    let mut launches = Vec::new();
    let mut bucket_start = 0;
    while bucket_start < ordered.len() {
        let bucket = multifidelity_compact_bucket(ordered[bucket_start]);
        let mut bucket_end = bucket_start + 1;
        while bucket_end < ordered.len()
            && multifidelity_compact_bucket(ordered[bucket_end]) == bucket
        {
            bucket_end += 1;
        }
        let mut record_offset = bucket_start;
        while record_offset < bucket_end {
            let record_count =
                (bucket_end - record_offset).min(MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK);
            launches.push(MultifidelityCompactLaunch {
                record_offset,
                record_count,
            });
            record_offset += record_count;
        }
        bucket_start = bucket_end;
    }
    Ok(MultifidelityCompactPlan {
        indices: ordered,
        launches,
    })
}

/// Build the packed prototype plan. Records are first ordered exactly like the
/// existing compact plan, then split into 32-record warp descriptors while
/// staying inside one aligned 32×32 receiver bucket. Descriptors are grouped
/// eight per CUDA block; a group may contain descriptors from adjacent buckets,
/// but each warp computes its own envelope and never shares a cull decision.
fn multifidelity_compact_packed_plan(
    dense_indices: &[usize],
) -> Result<MultifidelityCompactPackedPlan, String> {
    let mut ordered = dense_indices.to_vec();
    if let Some(&index) = ordered.iter().find(|&&index| index >= TILE_PX * TILE_PX) {
        return Err(format!("compact receiver index {index} exceeds tile"));
    }
    ordered.sort_unstable_by_key(|&index| {
        (
            multifidelity_compact_bucket(index),
            multifidelity_morton_key(index),
            index,
        )
    });
    if let Some(pair) = ordered.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("compact receiver index {} is duplicated", pair[0]));
    }

    let mut descriptors = Vec::new();
    let mut bucket_start = 0;
    while bucket_start < ordered.len() {
        let bucket = multifidelity_compact_bucket(ordered[bucket_start]);
        let mut bucket_end = bucket_start + 1;
        while bucket_end < ordered.len()
            && multifidelity_compact_bucket(ordered[bucket_end]) == bucket
        {
            bucket_end += 1;
        }
        let mut record_offset = bucket_start;
        while record_offset < bucket_end {
            let record_count =
                (bucket_end - record_offset).min(MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_WARP);
            descriptors.push(MultifidelityCompactPackedWarp {
                record_offset,
                record_count,
            });
            record_offset += record_count;
        }
        bucket_start = bucket_end;
    }

    let empty = MultifidelityCompactPackedWarp {
        record_offset: 0,
        record_count: 0,
    };
    let mut launches = Vec::new();
    for descriptor_group in descriptors.chunks(MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK) {
        let mut block = [empty; MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK];
        block[..descriptor_group.len()].copy_from_slice(descriptor_group);
        launches.push(block);
    }
    Ok(MultifidelityCompactPackedPlan {
        indices: ordered,
        launches,
    })
}

#[inline]
fn multifidelity_bilinear_corners(
    data: &MultifidelityInterpolation,
    py: usize,
    px: usize,
) -> ([usize; 4], [f64; 4]) {
    let y0 = data.lower[py];
    let y1 = data.upper[py];
    let x0 = data.lower[px];
    let x1 = data.upper[px];
    let corners = [
        y0 * data.axis.len() + x0,
        y0 * data.axis.len() + x1,
        y1 * data.axis.len() + x0,
        y1 * data.axis.len() + x1,
    ];
    let fy = data.fraction[py];
    let fx = data.fraction[px];
    (
        corners,
        [
            (1.0 - fy) * (1.0 - fx),
            (1.0 - fy) * fx,
            fy * (1.0 - fx),
            fy * fx,
        ],
    )
}

/// Reconstruct one period from launch-A cheap energy and the exact anchor
/// field. Positive, well-conditioned corners interpolate the exact/cheap
/// ratio in log energy (a smooth attenuation correction); a zero-sided corner
/// falls back to bilinear linear exact energy. This keeps silence finite and
/// avoids inventing an infinity by taking `log(0)`.
#[inline]
fn multifidelity_bilinear_period_energy(
    data: &MultifidelityInterpolation,
    py: usize,
    px: usize,
    period: usize,
) -> Option<f64> {
    let (corners, weights) = multifidelity_bilinear_corners(data, py, px);
    if corners
        .iter()
        .any(|&corner| !data.log_energy_correction_valid[corner][period])
    {
        let exact = corners
            .iter()
            .zip(weights)
            .map(|(&corner, weight)| data.exact_anchor_energy[corner][period] * weight)
            .sum::<f64>();
        return exact
            .is_finite()
            .then_some(exact.clamp(0.0, f64::from(f32::MAX)));
    }

    let log_correction = corners
        .iter()
        .zip(weights)
        .map(|(&corner, weight)| data.log_energy_correction[corner][period] * weight)
        .sum::<f64>();
    let cheap_index = (py * TILE_PX + px) * NUM_PERIODS + period;
    let cheap = f64::from(data.cheap_accum.energy[cheap_index]);
    if log_correction.is_finite() && cheap.is_finite() && cheap > 0.0 {
        let corrected = cheap * log_correction.exp();
        if corrected.is_finite() && corrected >= 0.0 && corrected <= f64::from(f32::MAX) {
            return Some(corrected);
        }
    }

    // A finite exact fallback also covers cheap zero/negative/NaN values and
    // an overflowing multiplicative correction. Exact anchor energies are f32
    // values, so this weighted sum is bounded by f32::MAX.
    let exact = corners
        .iter()
        .zip(weights)
        .map(|(&corner, weight)| data.exact_anchor_energy[corner][period] * weight)
        .sum::<f64>();
    exact
        .is_finite()
        .then_some(exact.clamp(0.0, f64::from(f32::MAX)))
}

#[inline]
fn multifidelity_interior_is_present(
    data: &MultifidelityInterpolation,
    py: usize,
    px: usize,
) -> bool {
    let index = py * TILE_PX + px;
    // Keep the W1 cheap presence authority for every z13 arm. In particular,
    // stride4 must not switch to the 0.40 anchor-probability contour: that
    // would make the ladder change two variables at once. Exact anchors remain
    // authoritative for their lattice cells.
    data.cheap_cells[index] != tile_painter::wire_hm3::NO_DATA
}

/// Build launch C's exact receiver list from launch-A observables only: compact
/// exact-anchor residuals and cheap HM3 state. One rule for both layers; which
/// layers reach this at all is [`multifidelity_layer_replays`]. The compact
/// kernel receives only the selected records, never dense receiver arrays.
fn multifidelity_receiver_mask(layer: LineLayer, data: &MultifidelityInterpolation) -> Vec<f32> {
    multifidelity_receiver_mask_with_replay(layer, data, multifidelity_layer_replays(layer))
}

fn multifidelity_receiver_mask_with_replay(
    layer: LineLayer,
    data: &MultifidelityInterpolation,
    replay_selected_blocks: bool,
) -> Vec<f32> {
    let mut mask = vec![0.0f32; TILE_PX * TILE_PX];
    if !replay_selected_blocks {
        report_multifidelity_mask(layer, &mask, data);
        return mask;
    }
    // The stride lattice is not tile-divisible: the final interval is the
    // actual [axis[len - 2], axis[len - 1]) tail (496..511 for stride 16).
    // Iterate adjacent axis windows themselves so tail receivers participate
    // in selection and no range crosses the tile edge.
    for (by, py_window) in data.axis.windows(2).enumerate() {
        let py_start = py_window[0];
        let py_end = py_window[1];
        for (bx, px_window) in data.axis.windows(2).enumerate() {
            let px_start = px_window[0];
            let px_end = px_window[1];
            let corners = [
                by * data.axis.len() + bx,
                by * data.axis.len() + bx + 1,
                (by + 1) * data.axis.len() + bx,
                (by + 1) * data.axis.len() + bx + 1,
            ];
            let mut residual_low = f64::INFINITY;
            let mut residual_high = f64::NEG_INFINITY;
            for &corner in &corners {
                if data.valid[corner] {
                    residual_low = residual_low.min(data.residual[corner]);
                    residual_high = residual_high.max(data.residual[corner]);
                }
            }
            if !residual_low.is_finite() {
                residual_low = 0.0;
                residual_high = 0.0;
            }
            let residual_range = residual_high - residual_low;
            // A block earns the exact tail when its anchors DISAGREE about the
            // correction: that is the bilinear fill between them saying it cannot be
            // trusted. The rule used to also fire on a step in the cheap field, but
            // that arm selected NOTHING — 0 px across all 43 rail tiles of wbench-orig
            // — and it could not have: the cheap field never calls obstacle screening
            // (kernels/scatter.cu:3726), so it has no shadow edge to step over. It
            // cost a walk over all 64 pixels of every block to measure that.
            //
            // 12 dB is what the wall affords, not what the rule would like. Measured on
            // wbench-orig against the exact-CPU reference, each arm paired against its
            // own baseline on an idle card: 8 dB selects 4.60 % of a tile and puts the
            // wall at 248.1 s, past its 238.5 s ceiling; 12 dB selects 1.33 % and lands
            // at 226.5 s against a 219.4 s baseline, taking rail's >1 dB rung
            // 10.288 -> 8.684 %, >2 dB 4.241 -> 3.169 %, >6 dB 0.390 -> 0.199 %, with
            // max_abs 20.5 -> 19.5 dB and the quiet band 20.0 -> 18.5 dB. The wall is
            // what binds, not the lane: all seven lanes share one card, so 1 s of rail
            // lane costs about 0.3 s of wall.
            //
            // Road's old rule tested the SIZE of the anchor correction instead of the
            // disagreement, and the cheap field it ranged over never calls obstacle
            // screening (kernels/scatter.cu:3726), so it had no edge to find: it
            // selected 0.3-0.5 % of a tile and bought 19 of road's 17 614 cells past
            // the >6 dB rung, for +49.7 s on its own lane.
            let exact_block = residual_range >= 12.0;
            if exact_block {
                for py in py_start..py_end {
                    for px in px_start..px_end {
                        mask[py * TILE_PX + px] = 1.0;
                    }
                }
            }
        }
    }
    // Exact anchors are already present in launch B's compact output. Keep
    // them out of launch C even when their stride-window selector block was selected;
    // reconstruction restores their authoritative values separately.
    for &py in &data.axis {
        for &px in &data.axis {
            mask[py * TILE_PX + px] = 0.0;
        }
    }
    report_multifidelity_mask(layer, &mask, data);
    mask
}

/// One line per tile per layer: how much of it the selector bought back exactly. A
/// layer that never replays still reports its zero, so the log answers the question
/// for the whole run and not only for the layers that spend.
fn report_multifidelity_mask(layer: LineLayer, mask: &[f32], data: &MultifidelityInterpolation) {
    let selected = mask.iter().filter(|&&value| value >= 0.5).count();
    let anchor_count = data.axis.len() * data.axis.len();
    eprintln!(
        "MULTIFIDELITY_MASK layer={} replay={selected}/{} ({:.4}%) anchors={} exact_total={} ({:.4}%)",
        layer.dir(),
        mask.len(),
        selected as f64 * 100.0 / mask.len() as f64,
        anchor_count,
        selected + anchor_count,
        (selected + anchor_count) as f64 * 100.0 / mask.len() as f64,
    );
}

/// Reconstruct a dense HM3 tile from cheap full-grid energies, compact exact
/// fixed-stride anchors, and compact selected replay records. Every non-authoritative
/// receiver gets a per-period energy correction before the one final canonical
/// HM3 collapse; compact outputs are mapped by their explicit dense index.
fn reconstruct_multifidelity_cells(
    data: &MultifidelityInterpolation,
    exact_replay_output: &[f32],
) -> Vec<u8> {
    let replay_count = exact_replay_output.len() / noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE;
    let replay = decode_multifidelity_compact_output(exact_replay_output, replay_count)
        .unwrap_or_else(|error| panic!("invalid compact replay output: {error}"));
    let mut output_accum = TileAccumulator::new();
    output_accum
        .energy
        .copy_from_slice(&data.cheap_accum.energy);
    for &py in &data.axis {
        for &px in &data.axis {
            let target = (py * TILE_PX + px) * NUM_PERIODS;
            output_accum.energy[target..target + NUM_PERIODS]
                .copy_from_slice(&data.exact_accum.energy[target..target + NUM_PERIODS]);
        }
    }
    let mut replay_mask = vec![false; TILE_PX * TILE_PX];
    for (dense_index, energies, _) in replay {
        replay_mask[dense_index] = true;
        let target = dense_index * NUM_PERIODS;
        output_accum.energy[target..target + NUM_PERIODS].copy_from_slice(&energies);
    }

    // Reconstruct in the raw three-period energy field. Positive anchor
    // periods use an interpolated log(exact/cheap) correction, while a zero-
    // sided/silent corner uses the linear exact-energy fallback. Exact anchors
    // remain authoritative; interior presence follows the dense cheap field so
    // the audibility contour is not quantised to the sparse anchor lattice.
    let mut reconstructed = vec![false; TILE_PX * TILE_PX];
    for py in 0..TILE_PX {
        for px in 0..TILE_PX {
            let index = py * TILE_PX + px;
            if multifidelity_is_anchor(data.stride, py, px) || replay_mask[index] {
                continue;
            }
            if !multifidelity_interior_is_present(data, py, px) {
                continue;
            }
            let target = index * NUM_PERIODS;
            let mut any_period = false;
            for period in 0..NUM_PERIODS {
                if let Some(energy) = multifidelity_bilinear_period_energy(data, py, px, period) {
                    output_accum.energy[target + period] = energy as f32;
                    any_period = true;
                }
            }
            reconstructed[index] = any_period;
        }
    }

    // This is the only HM3 collapse in the reconstructed path. In particular,
    // no interpolated byte or byte residual can alter the authoritative exact
    // anchor/replay energies after quantisation.
    let output_cells = collapse_lden_surface_u8(&output_accum);
    let mut cells = vec![tile_painter::wire_hm3::NO_DATA; TILE_PX * TILE_PX];
    for py in 0..TILE_PX {
        for px in 0..TILE_PX {
            let index = py * TILE_PX + px;
            // Exact anchors and launch-C lanes are authoritative outputs.
            if multifidelity_is_anchor(data.stride, py, px) || replay_mask[index] {
                cells[index] = output_cells[index];
                continue;
            }
            if reconstructed[index] || multifidelity_interior_is_present(data, py, px) {
                // If an unexpected non-finite input made all period helpers
                // decline, this preserves launch A after the same presence
                // gate; normal records take the reconstructed branch above.
                cells[index] = output_cells[index];
            }
        }
    }
    cells
}

/// `QM_GPU_TILE_TIMES=1` — one `tile-time` stderr line per (tile, layer), the
/// per-tile distribution instrument of the gather redesign (its §8 task 0(i):
/// lane sums hide the benchmark's heavy tail — Sahara tiles run ~0.1 s while
/// dense town tiles run minutes, and budget arithmetic needs the distribution).
/// Off by default: a world worker builds thousands of tile-layers and the
/// boxlog must not carry one line each. `wall_ms` is the host wall from H2D
/// start to D2H join and so INCLUDES the next tile's CPU prep overlapped under
/// this kernel. Its compact JSON payload uses `kernel_ms=null` plus status
/// `unavailable` unless `NOISE_GPU_TIMING=1` measured a finite CUDA-event
/// duration.
fn tile_times_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("QM_GPU_TILE_TIMES").as_deref() == Ok("1"))
}

/// Opt-in acceptance census for the reviewed rail z12/2206/1391 fixture. This
/// is deliberately separate from ordinary production: it allocates the 10 MiB
/// PROF_COUNTERS tail and requires a matching counter-instrumented PTX.
fn rail_arcstat_census_required() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NOISE_GPU_REQUIRE_RAIL_ARCSTAT_CENSUS").as_deref() == Ok("1"))
}

static RAIL_ARCSTAT_CENSUS_PASSES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Per-layer end-to-end counters (timings in seconds, summed over tiles).
#[derive(Clone, Default)]
struct LayerStat {
    t_kernel: f64,
    /// Isolated kernel time from CUDA events bracketing the launch (ms), summed
    /// over the layer's tiles. Only populated when `NOISE_GPU_TIMING=1`; the
    /// optimisation harness's median-of-N `KERNEL_MS` (vs the host-wall `t_kernel`,
    /// which folds in the htod/dtoh copies + sync). Stays 0 when timing is off.
    kernel_ms: f64,
    kernel_calls: usize,
    t_bins: f64,
    t_load: f64,
    /// Host-wall time of the synchronous H2D calls whose boundaries already exist. The shared
    /// block halo upload is not yet included, so the event names this as a measured subset.
    t_h2d: f64,
    t_encode: f64,
    t_write: f64,
    /// All-source-empty cells still have correctness work: stale output files from an older
    /// repaint must be removed. Keep that operation separate from an encode/write that never ran.
    t_cleanup: f64,
    max_diff: i32,
    n_diff: usize,
    n_cmp: usize, // both-present cells compared (drift-gate denominator)
    n_le1: usize, // |Δ| ≤ 1 byte (0.5 dB)
    n_le3: usize, // |Δ| ≤ 3 bytes (1.5 dB)
    n_baseline: usize,
    n_written: usize,
    bytes_written: usize,
    n_cleanup_checked: usize,
    n_cleanup_removed: usize,
    n_tiles: usize,
}

impl LayerStat {
    fn absorb(&mut self, other: Self) {
        self.t_kernel += other.t_kernel;
        self.kernel_ms += other.kernel_ms;
        self.kernel_calls += other.kernel_calls;
        self.t_bins += other.t_bins;
        self.t_load += other.t_load;
        self.t_h2d += other.t_h2d;
        self.t_encode += other.t_encode;
        self.t_write += other.t_write;
        self.t_cleanup += other.t_cleanup;
        self.max_diff = self.max_diff.max(other.max_diff);
        self.n_diff += other.n_diff;
        self.n_cmp += other.n_cmp;
        self.n_le1 += other.n_le1;
        self.n_le3 += other.n_le3;
        self.n_baseline += other.n_baseline;
        self.n_written += other.n_written;
        self.bytes_written += other.bytes_written;
        self.n_cleanup_checked += other.n_cleanup_checked;
        self.n_cleanup_removed += other.n_cleanup_removed;
        self.n_tiles += other.n_tiles;
    }
}

#[allow(dead_code)]
struct RegionResult {
    written: usize,
    skipped: usize,
    raster: std::time::Duration,
}

struct Cfg {
    z: u8,
    batch_n: u32,
    halo_m: f64,
    h3r4: PathBuf,
    baseline: String,
    output: Option<String>,
    /// `QM_GPU_BARRIERS` — upload each region's `barriers.arrow` walls so the
    /// kernel screens them on the GPU (the exact ray×segment crossings of
    /// `barrier_best_candidate`, the CUDA twin of `path_effects` §1).
    /// Default ON since 2026-08-02 IN THE ENGINE ITSELF (owner directive
    /// 2026-06-13: every GPU surface build screens its own barriers). It used
    /// to be a wrapper-supplied env (v1 cluster-build-chunk.sh forced =1) and
    /// the v2 orchestrator rewrite silently lost it — every fleet GPU
    /// road/rail paint ran wall-blind until Voznice exposed it. `=0` remains
    /// the explicit barrier-blind baseline (what
    /// `tests/barrier_screening.rs` compares ON against, via the programmatic
    /// flag).
    barriers_enabled: bool,
}

/// A layer's GPU-resident source buffers (`seg`, `sp`, `semis`), uploaded once per
/// region (by the caller) and shared across every block/tile of that region.
type LayerSrc = (LineLayer, (CudaSlice<f64>, CudaSlice<f64>, CudaSlice<f32>));

/// CPU-side region payload loaded once, then swept by any number of GPU tile
/// workers. Source H2D still happens per worker (each CUDA context owns its
/// slices); the expensive host work — rows, packed SOA, barriers, obstacles —
/// is not repeated per worker or per tile.
struct HostRegion {
    r4: u64,
    region_rows: Vec<(LineLayer, Vec<LineRow>)>,
    barriers: BarrierData,
    obstacles: ObstacleData,
    src_host: Vec<(LineLayer, SourceBuffers)>,
    /// Empty when the census is off (baseline/evidence runs keep the full sweep
    /// so their silent-tile counters still see every tile).
    reach: Vec<(LineLayer, std::collections::HashSet<(u32, u32)>)>,
}

/// One cropped halo-block ready for `process_block`. The region's tiles in this
/// block are independent GPU jobs over the same uploaded sources.
struct GpuBlockJob {
    host: Arc<HostRegion>,
    cell: Arc<CellGate>,
    batch: TileBatch,
    interiors: Vec<Option<InteriorEstimate>>,
    block_tiles: Vec<(u32, u32)>,
    /// Last clone of the chunk byte-gate permit. Builders `acquire`; workers
    /// never do. Drop of the last job releases the chunk — no circular wait.
    #[allow(dead_code)]
    permit: Arc<ChunkPermit>,
}

/// Per-cell completion gate so a loaded region's tiles can sit in the same
/// queue as another region's tiles without mixing `done` accounting.
struct CellGate {
    r4: u64,
    remaining: AtomicUsize,
    crop_done: AtomicBool,
    emitted: AtomicBool,
    failed: Mutex<Option<String>>,
    stats: Mutex<BTreeMap<&'static str, LayerStat>>,
    total: usize,
    t0: Instant,
    raster: Mutex<std::time::Duration>,
    interval_id: u64,
    worker_slot: usize,
    tiles: Vec<(u32, u32)>,
    effective: Vec<LineLayer>,
}

/// Stdin cell queue shared by the reader thread and the coordinator.
type StreamCellQueue = Arc<(Mutex<(VecDeque<(u64, Option<Vec<String>>)>, bool)>, Condvar)>;

/// Stdout + evidence needed when the last halo-block of a cell finishes (any
/// tile-worker may be the one that trips the gate).
struct StreamSink {
    out: Arc<Mutex<std::io::Stdout>>,
    evidence: RendererEvidence,
    cfg: Arc<Cfg>,
    z: u8,
    work: StreamCellQueue,
}

fn close_pending_stream_cells(work: &StreamCellQueue) {
    let (lock, cv) = &**work;
    let mut g = lock.lock().unwrap();
    g.0.clear();
    g.1 = true;
    cv.notify_all();
}

/// Spans line then protocol `fail`. False means stdout is gone — stop claiming.
fn emit_stream_fail(
    sink: &StreamSink,
    r4: u64,
    worker_slot: usize,
    interval_id: u64,
    t0: Instant,
    line: &str,
) -> bool {
    let mut spans = EngineCellSpans::new(r4, "gpu-surface", worker_slot, t0);
    spans.finish_failed(t0.elapsed(), line);
    sink.evidence
        .region_terminal(
            r4,
            worker_slot,
            interval_id,
            RegionTerminalStatus::Fail,
            0,
            0,
            Some(line),
        )
        .expect("emit GPU surface region failure");
    write_stream_protocol_lines(sink, &spans.line(), line)
}

fn write_stream_protocol_lines(sink: &StreamSink, spans_line: &str, protocol_line: &str) -> bool {
    let mut o = sink.out.lock().unwrap();
    let ok = writeln!(o, "{spans_line}").is_ok()
        && writeln!(o, "{protocol_line}").is_ok()
        && o.flush().is_ok();
    drop(o);
    if !ok {
        eprintln!("stream: stdout closed while emitting {protocol_line}");
        close_pending_stream_cells(&sink.work);
    }
    ok
}

/// If a tile-worker panics between pop and `finish_block_job`, this still
/// decrements inflight/`remaining` and closes stdin work so the coordinator
/// cannot park forever in `wait_below` / `cv.wait`.
struct JobAccountGuard<'a> {
    cell: Arc<CellGate>,
    pool: &'a TileJobPool,
    sink: &'a StreamSink,
    armed: bool,
}

impl Drop for JobAccountGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        *self.cell.failed.lock().unwrap() = Some("tile-worker panicked".into());
        self.cell.remaining.fetch_sub(1, Ordering::SeqCst);
        self.pool.job_finished();
        close_pending_stream_cells(&self.sink.work);
        let _ = emit_cell_if_complete(&self.cell, self.sink);
    }
}

fn load_region_host(
    r4: u64,
    layers: &[LineLayer],
    cfg: &Cfg,
    stats: &mut BTreeMap<&'static str, LayerStat>,
) -> Result<Option<HostRegion>> {
    let cell = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let mut region_rows: Vec<(LineLayer, Vec<LineRow>)> = Vec::with_capacity(layers.len());
    for &layer in layers {
        let tl = Instant::now();
        let r = layer.load_rows(&cfg.h3r4, &ring, cell)?;
        stats.entry(layer.dir()).or_default().t_load += tl.elapsed().as_secs_f64();
        region_rows.push((layer, r));
    }
    if region_rows.iter().all(|(_, rows)| rows.is_empty()) {
        return Ok(None);
    }
    let barriers = if cfg.barriers_enabled {
        BarrierData::load_for_r4s(&cfg.h3r4, &ring).context("load barriers")?
    } else {
        BarrierData::from_segments(Vec::new())
    };
    let obstacles = ObstacleData::load_for_r4s(&cfg.h3r4, r4, &ring)
        .with_context(|| format!("load obstacles R4 {r4:015x}"))?;
    let mut src_host: Vec<(LineLayer, SourceBuffers)> = Vec::with_capacity(region_rows.len());
    for (layer, rows) in &region_rows {
        let tp = Instant::now();
        let packed = pack_sources(rows);
        stats.entry(layer.dir()).or_default().t_bins += tp.elapsed().as_secs_f64();
        src_host.push((*layer, packed));
    }
    // Evidence and baseline runs keep the full sweep: their drift counters are
    // defined over every tile. Same predicate the rest of the file uses.
    let census_on =
        cfg.baseline.is_empty() && std::env::var(RENDERER_EVIDENCE_FLAG).as_deref() != Ok("1");
    let reach = if census_on {
        build_reach_census(cfg.z, &region_tiles(r4, cfg.z), &region_rows)
    } else {
        Vec::new()
    };
    Ok(Some(HostRegion {
        r4,
        region_rows,
        reach,
        barriers,
        obstacles,
        src_host,
    }))
}

fn upload_host_sources(
    dev: &Arc<CudaDevice>,
    host: &HostRegion,
    stats: &mut BTreeMap<&'static str, LayerStat>,
) -> Result<(Vec<LayerSrc>, ObstDev)> {
    let mut src_dev: Vec<LayerSrc> = Vec::with_capacity(host.src_host.len());
    for (layer, packed) in &host.src_host {
        let th = Instant::now();
        let uploaded = (
            dev.htod_copy(packed.seg.clone()).expect("seg"),
            dev.htod_copy(packed.sp.clone()).expect("sp"),
            dev.htod_copy(packed.semis.clone()).expect("semis"),
        );
        stats.entry(layer.dir()).or_default().t_h2d += th.elapsed().as_secs_f64();
        src_dev.push((*layer, uploaded));
    }
    let obst_dev = upload_obstacles(dev, host.obstacles.set())?;
    Ok((src_dev, obst_dev))
}

fn accumulate_baseline_diff(
    stats: &mut BTreeMap<&'static str, LayerStat>,
    layer: LineLayer,
    cfg: &Cfg,
    tx: u32,
    ty: u32,
    cells: &[u8],
) -> Result<()> {
    if cfg.baseline.is_empty() {
        return Ok(());
    }
    let bp = Path::new(&cfg.baseline)
        .join(layer.dir())
        .join(cfg.z.to_string())
        .join(tx.to_string())
        .join(format!("{ty}.bin"));
    if !bp.exists() {
        return Ok(());
    }
    let b = read_tile(&bp)?;
    let st = stats.entry(layer.dir()).or_default();
    for ci in 0..cells.len().min(b.len()) {
        let (c, bb) = (cells[ci], b[ci]);
        let differ = if c != NO_DATA && bb != NO_DATA {
            let d = (c as i32 - bb as i32).abs();
            st.max_diff = st.max_diff.max(d);
            st.n_cmp += 1;
            if d <= 1 {
                st.n_le1 += 1;
            }
            if d <= 3 {
                st.n_le3 += 1;
            }
            d > 0
        } else {
            c != bb
        };
        if differ {
            st.n_diff += 1;
        }
    }
    st.n_baseline += 1;
    Ok(())
}

/// Compute every `(tile, layer)` in `block_tiles` on the GPU, using the caller-built
/// shared halo (`batch`, cropped in parallel across the region's blocks), the region's
/// pre-loaded rows, and pre-uploaded sources (`src_dev`) — all built/uploaded once per
/// centre-R4 region by the caller, not re-read or re-uploaded per block or tile.
#[allow(clippy::too_many_arguments)]
fn process_block(
    dev: &Arc<CudaDevice>,
    functions: &LineFunctions,
    batch: &TileBatch,
    interiors: &[Option<InteriorEstimate>],
    cfg: &Cfg,
    block_tiles: &[(u32, u32)],
    region_rows: &[(LineLayer, Vec<LineRow>)],
    reach: &[(LineLayer, std::collections::HashSet<(u32, u32)>)],
    src_dev: &[LayerSrc],
    barriers: &BarrierData,
    obst_dev: &ObstDev,
    stats: &mut BTreeMap<&'static str, LayerStat>,
    prog: &mut Progress,
) -> Result<()> {
    let halo = &batch.tiles[0].halo;
    let halo_geom = halo.geom();
    let (_, _, _, rows, cols) = halo_geom;
    let z13_profile = multifidelity_z13_profile()?;
    let candidate_build_on =
        multifidelity_line_enabled() && (cfg.z == 12 || (cfg.z == 13 && z13_profile.is_some()));

    let elev: Vec<f32> = halo.pixels().iter().map(|p| p.elevation).collect();
    // Noise barriers reach the kernel as the VECTOR per-tile `for_tile` slice
    // (exact ray×segment crossings in `barrier_best_candidate`), never as a
    // raster burn —
    // `FusedGrid::burn_building_max` was measured acoustically unsound (the ray
    // cadence steps over a one-cell-thin wall on most paths; mean +3.7 / max
    // +13.8 dB under-screening; decision record: tile-painter
    // tests/barrier_screening.rs).
    let mut cover = Vec::with_capacity(rows * cols * 2);
    for p in halo.pixels() {
        cover.push(p.forest);
        cover.push(p.imd);
    }
    let d_elev = dev.htod_copy(elev).expect("elev");
    let d_cover = dev.htod_copy(cover).expect("cover");
    // Ordinary production allocates energies + ARC FAULT only. The explicit
    // rail acceptance census adds the 10 MiB PROF_COUNTERS tail; `pack_tile`
    // carries the exact size in meta[13], so a counter PTX remains safe under a
    // normal production binary and a required census fails if counters are absent.
    let require_arcstat = rail_arcstat_census_required();
    if require_arcstat && candidate_build_on {
        bail!("rail ARCSTAT census is incompatible with the multifidelity W1 candidate");
    }
    if require_arcstat && cfg!(feature = "v2-h0") {
        bail!("rail ARCSTAT census is only defined for the stock surface role");
    }
    let out_slots = if require_arcstat {
        noise_gpu::OUT_SLOTS_PROF
    } else if candidate_build_on {
        noise_gpu::OUT_SLOTS_MULTIFIDELITY
    } else if cfg!(feature = "v2-h0") {
        noise_gpu::OUT_SLOTS_H0
    } else {
        noise_gpu::OUT_SLOTS_PROD
    };
    let mut d_out = dev.alloc_zeros::<f32>(out_slots).expect("out");
    // Arcs the kernel had to drop for ARC_MAX_MERGED overflow, cumulative over
    // this block (the buffer is allocated once and the kernel only ever adds).
    #[cfg(not(feature = "v2-h0"))]
    let mut arc_drops_seen = 0f32;
    #[cfg(feature = "v2-h0")]
    let arc_drops_seen = 0f32;
    // Launch A's ordinary output is cumulative across this block, while the
    // compact exact launches use fresh per-tile allocations. Keep their fault
    // count cumulative before combining it with d_out, otherwise the previous
    // tile's exact drops would be subtracted twice by the existing ARC delta.
    let mut multifidelity_faults_seen = 0f32;
    #[cfg(feature = "v2-h0")]
    let mut h0_counts_seen = [0_u64; noise_gpu::OUT_H0_COUNTERS];
    let mut arcstat_seen = [0f64; noise_gpu::OUT_ARCSTAT_COUNTERS];
    let launch_cfg = LaunchConfig {
        grid_dim: (N_BINS as u32, 1, 1),
        block_dim: ((BIN_W * BIN_W) as u32, 1, 1),
        shared_mem_bytes: 0,
    };

    // Software pipeline: a CUDA launch is async, so while tile N's kernel runs on
    // the GPU we bin+pack tile N+1 on the CPU (the cores otherwise idle during the
    // GPU wait). Single-threaded; dtoh_sync_copy is the join that waits for the
    // kernel. Nonempty per-(tile,layer) work stays in the same order. A layer
    // with zero rows is all-NO_DATA (write_tile=0): skip its kernels, unlink
    // stale tiles, still tick progress and compare an implicit silent tile
    // against NOISE_GPU_BASELINE when that is set.
    // Order by LAYER first (all road, then all rail), not interleaved: the pipeline
    // overlaps tile N+1's prep with tile N's kernel, so consecutive same-layer items
    // (similar kernel ≈ similar prep cost) overlap far better than road↔rail swings.
    for (layer, rows) in region_rows {
        if !rows.is_empty() {
            continue;
        }
        let cleanup_started = Instant::now();
        let mut removed = 0usize;
        if let Some(root) = &cfg.output {
            for &(tx, ty) in block_tiles {
                removed += usize::from(unlink_stale_tile(root, *layer, cfg.z, tx, ty)?);
            }
        }
        let silent = vec![NO_DATA; TILE_PX * TILE_PX];
        for &(tx, ty) in block_tiles {
            accumulate_baseline_diff(stats, *layer, cfg, tx, ty, &silent)?;
            stats.entry(layer.dir()).or_default().n_tiles += 1;
            prog.tick();
        }
        let st = stats.entry(layer.dir()).or_default();
        st.t_cleanup += cleanup_started.elapsed().as_secs_f64();
        st.n_cleanup_checked += block_tiles.len();
        st.n_cleanup_removed += removed;
    }
    let mut items: Vec<(u32, u32, LineLayer)> = region_rows
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .flat_map(|(l, _)| block_tiles.iter().map(move |&(tx, ty)| (tx, ty, *l)))
        .collect();
    // Silent-tile census (built once per region in `load_region_host`): drop the
    // pairs no source can reach. Their kernel would render all-NO_DATA and
    // `write_tile` would return 0 bytes, so the only thing lost is the work —
    // but the stale-output unlink that the all-silent path performs must still
    // happen, or a rebuild would leave a previous build's tile behind.
    if !reach.is_empty() {
        let mut dropped = Vec::new();
        items.retain(|&(tx, ty, layer)| {
            let reachable = reach
                .iter()
                .find(|(l, _)| *l == layer)
                .is_none_or(|(_, set)| set.contains(&(tx, ty)));
            if !reachable {
                dropped.push((tx, ty, layer));
            }
            reachable
        });
        for (tx, ty, layer) in dropped {
            if let Some(root) = &cfg.output {
                unlink_stale_tile(root, layer, cfg.z, tx, ty)?;
            }
            prog.tick();
        }
    }
    // GPU-side binning: the kernel (line_binned_fused) does the per-block source cull
    // itself, so per-tile prep is just the pack — no CPU build_pixel_bins (the old
    // prep-bound bottleneck). t_bins now measures only pack_tile (sub-ms). The
    // per-tile barrier slice rides along: reach-culled + sorted by for_tile, so
    // the kernel's early-break scans only the walls a path can actually reach.
    let prep = |it: (u32, u32, LineLayer)| -> TileBuffers {
        let (tx, ty, layer) = it;
        let tile = &batch.tiles[batch_slot(batch, tx, ty)];
        let tile_barriers = barriers.for_tile(&tile.bbox, cfg.halo_m);
        let nsrc = region_rows
            .iter()
            .find(|(l, _)| *l == layer)
            .expect("layer rows")
            .1
            .len();
        pack_tile(
            tile,
            halo_geom,
            &tile_barriers,
            SurfaceKernelTileParameters {
                byte_stop_control: ETA,
                swizzle_width: TW,
                source_count: nsrc,
                output_slots: out_slots,
                line_layer_tag: layer.h0_abi_tag(),
            },
        )
    };
    let prep_timed = |it: (u32, u32, LineLayer), stats: &mut BTreeMap<&'static str, LayerStat>| {
        let t = Instant::now();
        let p = prep(it);
        stats.entry(it.2.dir()).or_default().t_bins += t.elapsed().as_secs_f64();
        (it, p)
    };

    let mut iter = items.into_iter();
    let mut pending = iter.next().map(|it| prep_timed(it, stats));
    while let Some(((tx, ty, layer), bufs)) = pending {
        let tk = Instant::now();
        let multifidelity_stride = candidate_build_on
            .then(|| {
                let nsrc = region_rows
                    .iter()
                    .find(|(candidate_layer, _)| *candidate_layer == layer)
                    .expect("layer rows")
                    .1
                    .len();
                let requested_stride =
                    (cfg.z == 13).then(|| z13_profile.expect("z13 candidate profile").stride);
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer,
                    nsrc,
                    requested_stride,
                })
            })
            .flatten();
        let candidate_on = multifidelity_stride.is_some();
        let d_inner = dev.htod_copy(bufs.inner).expect("inner");
        let meta_host = bufs.meta;
        let d_meta = dev.htod_copy(meta_host.clone()).expect("meta");
        // Region-resident sources (uploaded once per layer above) — not re-uploaded per tile.
        // (nsrc rides in meta[12] — pack_tile; the freed launch slot carries the
        // obstacle pointer table.)
        let (d_seg, d_sp, d_semis) = &src_dev
            .iter()
            .find(|(l, _)| *l == layer)
            .expect("layer src")
            .1;
        // Cheap and unbinned exact keep the dense receiver ABI. Compact exact
        // passes receive a host-packed list; stride4 instead repacks the same
        // canonical receiver arrays into nine Cartesian unbinned launches.
        // Retaining one host copy is also necessary for launch C because the
        // selector chooses replay indices after the dense upload is consumed.
        let compact_receiver_host = candidate_on.then(|| (bufs.rxll.clone(), bufs.rxar.clone()));
        let stock_cartesian_anchor_plan = (multifidelity_stride
            == Some(MultifidelityStride::Stride4))
        .then(MultifidelityStockCartesianPlan::stride4)
        .transpose()
        .map_err(anyhow::Error::msg)?;
        if stock_cartesian_anchor_plan.is_some() {
            anyhow::ensure!(
                functions.cartesian_unbinned_exact.is_some(),
                "stride4 profile is missing its candidate Cartesian unbinned CUDA function"
            );
        }
        let compact_anchor_plan = match multifidelity_stride {
            Some(stride) if stock_cartesian_anchor_plan.is_none() => {
                let axis = multifidelity_anchor_axis(stride);
                let dense_indices = axis
                    .iter()
                    .flat_map(|&py| axis.iter().map(move |&px| py * TILE_PX + px))
                    .collect::<Vec<_>>();
                multifidelity_compact_packed_plan(&dense_indices)
                    .expect("packed compact anchor plan")
            }
            _ => MultifidelityCompactPackedPlan::default(),
        };
        let compact_anchor_words = match (
            compact_anchor_plan.indices.is_empty(),
            compact_receiver_host.as_ref(),
        ) {
            (false, Some((rxll, rxar))) => {
                pack_multifidelity_compact_receivers(rxll, rxar, &compact_anchor_plan.indices)
                    .expect("compact anchor receiver pack")
            }
            _ => Vec::new(),
        };
        let mut stock_cartesian_device_groups =
            if let Some(plan) = stock_cartesian_anchor_plan.as_ref() {
                let (rxll, rxar) = compact_receiver_host
                    .as_ref()
                    .expect("stock Cartesian receiver host copy");
                prepare_multifidelity_stock_cartesian_device_groups(dev, plan, rxll, rxar)?
            } else {
                Vec::new()
            };
        let d_stock_cartesian_meta = if stock_cartesian_anchor_plan.is_some() {
            let exact_meta =
                multifidelity_stock_cartesian_exact_meta(&meta_host).map_err(anyhow::Error::msg)?;
            Some(dev.htod_copy(exact_meta).expect("stock Cartesian meta"))
        } else {
            None
        };
        let d_rxll = dev.htod_copy(bufs.rxll).expect("rxll");
        let d_rxar = dev.htod_copy(bufs.rxar).expect("rxar");
        let d_barr = dev.htod_copy(bufs.barr).expect("barr");
        let h2d_done = Instant::now();
        stats.entry(layer.dir()).or_default().t_h2d += h2d_done.duration_since(tk).as_secs_f64();
        // Fork only after every shared and wave-private input upload. The
        // initial dependency makes those inputs visible without putting stream
        // creation inside the timed GPU envelope.
        let stock_cartesian_streams = if stock_cartesian_anchor_plan.is_some() {
            (0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS)
                .map(|group| {
                    dev.fork_default_stream()
                        .with_context(|| format!("fork stock Cartesian anchor stream {group}"))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        // CUDA-event bracket (timing only). Ordinary/compact work stays on the
        // default stream. After recording start, every Cartesian stream gets an
        // explicit second default-stream dependency that fences its exact work
        // behind this timestamp. The backend later joins all nine waves before
        // stop, measuring one complete concurrent GPU envelope rather than nine
        // misleading per-wave underloads.
        let kernel_evt = timing_enabled().then(|| {
            let stream = *dev.cu_stream();
            let start = result::event::create(CUevent_flags::CU_EVENT_DEFAULT).expect("evt start");
            let stop = result::event::create(CUevent_flags::CU_EVENT_DEFAULT).expect("evt stop");
            unsafe { result::event::record(start, stream).expect("record start") };
            (start, stop, stream)
        });
        if kernel_evt.is_some() {
            for (group, stream) in stock_cartesian_streams.iter().enumerate() {
                stream.wait_for_default().with_context(|| {
                    format!("fence stock Cartesian anchor stream {group} behind timing start")
                })?;
            }
        }
        // Optional phase census for the W1 design review. These CUDA events
        // isolate sequential dense cheap, compact anchors, and compact selected
        // replay. Cartesian anchors deliberately overlap launch A on nine
        // streams, so the sequential phase receipt is undefined for that arm.
        let stage_timing = candidate_on
            && stock_cartesian_anchor_plan.is_none()
            && std::env::var("QM_MULTIFIDELITY_STAGE_TIMES").as_deref() == Ok("1");
        let stage_events = stage_timing.then(|| {
            let make = || {
                result::event::create(CUevent_flags::CU_EVENT_DEFAULT)
                    .expect("multifidelity stage event")
            };
            (
                make(),
                make(),
                make(),
                make(),
                make(),
                make(),
                *dev.cu_stream(),
            )
        });
        let mut stage_replay_recorded = false;
        let mut kernel_stop_recorded = false;
        if let Some((dense_start, _, _, _, _, _, stream)) = stage_events.as_ref() {
            unsafe {
                result::event::record(*dense_start, *stream).expect("record dense stage start");
            }
        }
        let dense_function = if candidate_on {
            functions
                .multifidelity_cheap
                .as_ref()
                .expect("multifidelity cheap PTX function")
        } else {
            &functions.stock
        };
        unsafe {
            dense_function
                .clone()
                .launch(
                    launch_cfg,
                    noise_gpu::line_kernel_arguments!(
                        &d_elev,
                        &d_inner,
                        &d_cover,
                        &d_meta,
                        d_seg,
                        d_sp,
                        d_semis,
                        &d_rxll,
                        &d_rxar,
                        &d_barr,
                        &obst_dev.table,
                        &mut d_out,
                    ),
                )
                .expect("launch");
        }
        // Preserve the original stock timing contract: its stop event is
        // recorded immediately after launch, before any candidate-only work.
        if !candidate_on {
            if let Some((_, stop, stream)) = kernel_evt {
                unsafe { result::event::record(stop, stream).expect("record stop") };
                kernel_stop_recorded = true;
            }
        }
        // Dense stage stop is queued immediately after launch A.
        if let Some((_, dense_stop, _, _, _, _, stream)) = stage_events.as_ref() {
            unsafe {
                result::event::record(*dense_stop, *stream).expect("record dense stage stop");
            }
        }
        // Queue all 81 unbinned exact blocks immediately after launch A. The forked
        // streams wait only for the preceding H2D uploads, not for launch A, so
        // cheap and exact work can occupy the device concurrently.
        if stock_cartesian_anchor_plan.is_some() {
            launch_multifidelity_stock_cartesian_device_groups(
                functions
                    .cartesian_unbinned_exact
                    .as_ref()
                    .expect("candidate Cartesian unbinned CUDA function"),
                &stock_cartesian_streams,
                &mut stock_cartesian_device_groups,
                &d_elev,
                &d_inner,
                &d_cover,
                d_stock_cartesian_meta
                    .as_ref()
                    .expect("stock Cartesian exact metadata"),
                d_seg,
                d_sp,
                d_semis,
                &d_barr,
                &obst_dev.table,
            )?;
            // Queue every exact-wave join immediately, before host preparation
            // or D2H can insert an idle gap into the CUDA event envelope. The
            // default stream already contains launch A, so stop follows both
            // cheap completion and all nine exact completions. A layer that cannot
            // reach launch C stops here; for one that can, the stop is recorded below
            // instead — after the replay launch when there is one, and after the
            // selector when the mask came out empty.
            for (group, stream) in stock_cartesian_streams.iter().enumerate() {
                dev.wait_for(stream)
                    .with_context(|| format!("join stock Cartesian anchor stream {group}"))?;
            }
            if !multifidelity_layer_replays(layer) {
                if let Some((_, stop, stream)) = kernel_evt {
                    unsafe { result::event::record(stop, stream).expect("record stop") };
                    kernel_stop_recorded = true;
                }
            }
        }
        // Drop adds a redundant default-stream dependency and destroys the
        // forked streams while every private buffer is still alive. On unwind,
        // declaration order preserves the same streams-before-buffers lifetime.
        drop(stock_cartesian_streams);
        // Overlap: prep the NEXT item on the CPU while launch A runs on the GPU.
        pending = iter.next().map(|it| prep_timed(it, stats));

        let mut exact_replay_output = Vec::new();
        let mut candidate_interpolation = None;
        let mut gpu;
        if candidate_on {
            let stride = multifidelity_stride.expect("candidate stride");
            let anchor_count = stride.anchor_record_count();
            let mut compact_anchor_device = if stock_cartesian_anchor_plan.is_none() {
                assert_eq!(compact_anchor_plan.indices.len(), anchor_count);
                Some((
                    dev.htod_copy(compact_anchor_words)
                        .expect("compact anchor receivers"),
                    dev.htod_copy(multifidelity_compact_packed_plan_controls(
                        &compact_anchor_plan,
                    ))
                    .expect("compact anchor control"),
                    dev.alloc_zeros::<f32>(stride.compact_output_len())
                        .expect("compact anchor output"),
                ))
            } else {
                None
            };
            if let Some((_, _, anchor_start, _, _, _, stream)) = stage_events.as_ref() {
                unsafe {
                    result::event::record(*anchor_start, *stream)
                        .expect("record anchor stage start");
                }
            }
            if stock_cartesian_anchor_plan.is_none() {
                let (d_anchor_receivers, d_anchor_control, d_anchor_out) = compact_anchor_device
                    .as_mut()
                    .expect("compact anchor device buffers");
                launch_multifidelity_compact_records_packed(
                    functions
                        .multifidelity_compact_packed
                        .as_ref()
                        .expect("multifidelity packed compact PTX function"),
                    &compact_anchor_plan,
                    &d_elev,
                    &d_inner,
                    &d_cover,
                    &d_meta,
                    d_seg,
                    d_sp,
                    d_semis,
                    d_anchor_receivers,
                    d_anchor_control,
                    &d_barr,
                    &obst_dev.table,
                    d_anchor_out,
                );
            }
            if let Some((_, _, _, anchor_stop, _, _, stream)) = stage_events.as_ref() {
                unsafe {
                    result::event::record(*anchor_stop, *stream).expect("record anchor stage stop");
                }
            }

            // The selector sees only cheap full-grid output and compact exact
            // anchors. It never reads a dense exact tail or mutates d_out.
            gpu = dev
                .dtoh_sync_copy(&d_out)
                .expect("multifidelity cheap dtoh");
            let (exact_anchor_output, anchor_fault) =
                if let Some(plan) = stock_cartesian_anchor_plan.as_ref() {
                    download_multifidelity_stock_cartesian_device_groups(
                        dev,
                        plan,
                        &stock_cartesian_device_groups,
                    )?
                } else {
                    let (_, _, d_anchor_out) = compact_anchor_device
                        .as_ref()
                        .expect("compact anchor device buffers");
                    let exact_anchor_output = dev
                        .dtoh_sync_copy(d_anchor_out)
                        .expect("multifidelity compact anchor dtoh");
                    let anchor_fault =
                        multifidelity_compact_fault_sum(&exact_anchor_output, anchor_count)
                            .expect("multifidelity compact anchor fault decode");
                    (exact_anchor_output, anchor_fault)
                };
            let interpolation = multifidelity_interpolation(stride, &gpu, &exact_anchor_output);
            let receiver_mask = multifidelity_receiver_mask(layer, &interpolation);
            candidate_interpolation = Some(interpolation);
            let replay_indices_unsorted: Vec<usize> = receiver_mask
                .iter()
                .enumerate()
                .filter_map(|(index, &value)| (value >= 0.5).then_some(index))
                .collect();
            let replay_plan =
                multifidelity_compact_plan(&replay_indices_unsorted).expect("compact replay plan");
            let mut exact_fault_this_tile = anchor_fault;

            if replay_plan.indices.is_empty() {
                if !kernel_stop_recorded {
                    if let Some((_, stop, stream)) = kernel_evt {
                        unsafe { result::event::record(stop, stream).expect("record stop") };
                        unsafe { result::stream::synchronize(stream).expect("synchronize stop") };
                        kernel_stop_recorded = true;
                    }
                }
            } else {
                let (rxll, rxar) = compact_receiver_host
                    .as_ref()
                    .expect("compact receiver host copy");
                let replay_words =
                    pack_multifidelity_compact_receivers(rxll, rxar, &replay_plan.indices)
                        .expect("compact replay receiver pack");
                let replay_count = replay_plan.indices.len();
                let d_replay_receivers = dev
                    .htod_copy(replay_words)
                    .expect("compact replay receivers");
                let d_replay_control = dev
                    .htod_copy(multifidelity_compact_plan_controls(&replay_plan))
                    .expect("compact replay control");
                let mut d_replay_out = dev
                    .alloc_zeros::<f32>(
                        replay_count * noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE,
                    )
                    .expect("compact replay output");
                if let Some((_, _, _, _, replay_start, _, stream)) = stage_events.as_ref() {
                    unsafe {
                        result::event::record(*replay_start, *stream)
                            .expect("record replay stage start");
                    }
                }
                launch_multifidelity_compact_records(
                    functions
                        .multifidelity_compact
                        .as_ref()
                        .expect("multifidelity compact PTX function"),
                    &replay_plan,
                    &d_elev,
                    &d_inner,
                    &d_cover,
                    &d_meta,
                    d_seg,
                    d_sp,
                    d_semis,
                    &d_replay_receivers,
                    &d_replay_control,
                    &d_barr,
                    &obst_dev.table,
                    &mut d_replay_out,
                );
                if let Some((_, _, _, _, _, replay_stop, stream)) = stage_events.as_ref() {
                    unsafe {
                        result::event::record(*replay_stop, *stream)
                            .expect("record replay stage stop");
                    }
                    stage_replay_recorded = true;
                }
                if let Some((_, stop, stream)) = kernel_evt {
                    unsafe { result::event::record(stop, stream).expect("record stop") };
                    kernel_stop_recorded = true;
                }
                exact_replay_output = dev
                    .dtoh_sync_copy(&d_replay_out)
                    .expect("multifidelity compact replay dtoh");
                let replay_fault =
                    multifidelity_compact_fault_sum(&exact_replay_output, replay_count)
                        .expect("multifidelity compact replay fault decode");
                exact_fault_this_tile += replay_fault;
            }
            multifidelity_faults_seen += exact_fault_this_tile;
        } else {
            gpu = dev.dtoh_sync_copy(&d_out).expect("dtoh");
        }
        // A multifidelity build can mix candidate rail/dense-road tiles with
        // role-exact binned sparse-road tiles in either CLI layer order. Always combine
        // both cumulative counters so the existing per-tile delta remains
        // monotonic when the next item switches back to the dense output slot.
        if candidate_build_on {
            add_multifidelity_fault_total_to_dense_slot(&mut gpu, multifidelity_faults_seen);
        }
        if !kernel_stop_recorded {
            if let Some((_, stop, stream)) = kernel_evt {
                unsafe { result::event::record(stop, stream).expect("record stop") };
                unsafe { result::stream::synchronize(stream).expect("synchronize stop") };
            }
        }
        let tile_wall_s = tk.elapsed().as_secs_f64();
        let mut tile_kernel_ms = None;
        {
            let st = stats.entry(layer.dir()).or_default();
            st.t_kernel += tile_wall_s;
            if let Some((start, stop, _)) = kernel_evt {
                // Stream is synced by dtoh above ⇒ both events are recorded.
                let ms = unsafe { result::event::elapsed(start, stop).expect("elapsed") } as f64;
                tile_kernel_ms = Some(ms);
                st.kernel_ms += ms;
                st.kernel_calls += 1;
                unsafe {
                    result::event::destroy(start).expect("destroy start");
                    result::event::destroy(stop).expect("destroy stop");
                }
            }
        }

        if let Some((
            dense_start,
            dense_stop,
            anchor_start,
            anchor_stop,
            replay_start,
            replay_stop,
            _,
        )) = stage_events
        {
            let dense_ms = unsafe {
                result::event::elapsed(dense_start, dense_stop).expect("dense stage elapsed")
            } as f64;
            let anchor_ms = unsafe {
                result::event::elapsed(anchor_start, anchor_stop).expect("anchor stage elapsed")
            } as f64;
            let replay_ms = if stage_replay_recorded {
                Some(unsafe {
                    result::event::elapsed(replay_start, replay_stop).expect("replay stage elapsed")
                } as f64)
            } else {
                None
            };
            eprintln!(
                "MULTIFIDELITY_STAGE layer={} z{}/{tx}/{ty} dense_ms={dense_ms:.3} anchor_ms={anchor_ms:.3} replay_ms={replay_ms:?}",
                layer.dir(), cfg.z,
            );
            unsafe {
                result::event::destroy(dense_start).expect("destroy dense stage start");
                result::event::destroy(dense_stop).expect("destroy dense stage stop");
                result::event::destroy(anchor_start).expect("destroy anchor stage start");
                result::event::destroy(anchor_stop).expect("destroy anchor stage stop");
                result::event::destroy(replay_start).expect("destroy replay stage start");
                result::event::destroy(replay_stop).expect("destroy replay stage stop");
            }
        }

        // ARC FAULT: a nonzero delta means THIS tile under-screens somewhere —
        // blocked arcs the merged list had no room for were dropped, so a
        // direction that a building genuinely blocks was painted clear. Stock/W1
        // keep the historical loud-but-nonfatal production policy below. W2's
        // per-call pre-clip union is experimental and must fail closed before
        // reconstruction; its benchmark receipt already requires zero faults.
        let arc_drops_this_tile = gpu[noise_gpu::OUT_FAULT_SLOT] - arc_drops_seen;
        require_zero_w2_dense_arc_fault(
            candidate_build_on && multifidelity_cartesian_unbinned_anchor_enabled(),
            arc_drops_this_tile,
        )
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("{} z{}/{tx}/{ty}", layer.dir(), cfg.z))?;
        if arc_drops_this_tile > 0.0 {
            #[cfg(feature = "v2-h0")]
            bail!(
                "V2 H0 ABI/layout fault {} z{}/{tx}/{ty}: production_fault_slot_delta={:.0}",
                layer.dir(),
                cfg.z,
                arc_drops_this_tile,
            );
            #[cfg(not(feature = "v2-h0"))]
            {
                eprintln!(
                    "!! ARC OVERFLOW {} z{}/{tx}/{ty}: {:.0} blocked arcs DROPPED \
                 (ARC_MAX_MERGED too small for this geometry) — this tile UNDER-screens; \
                 re-measure with NOISE_GPU_DEFINES=\"-DARC_MAX_MERGED=<bigger>\"",
                    layer.dir(),
                    cfg.z,
                    arc_drops_this_tile,
                );
                arc_drops_seen = gpu[noise_gpu::OUT_FAULT_SLOT];
            }
        }
        if require_arcstat {
            let mut current = [0f64; noise_gpu::OUT_ARCSTAT_COUNTERS];
            for px in
                gpu[noise_gpu::OUT_ARCSTAT_BASE..].chunks_exact(noise_gpu::OUT_ARCSTAT_COUNTERS)
            {
                for (acc, value) in current.iter_mut().zip(px.iter()) {
                    *acc += f64::from(*value);
                }
            }
            anyhow::ensure!(
                arc_drops_this_tile == 0.0,
                "rail ARCSTAT census dropped {arc_drops_this_tile:.0} arcs"
            );
            anyhow::ensure!(
                layer == LineLayer::Rail && cfg.z == 12 && tx == 2206 && ty == 1391,
                "rail ARCSTAT census requires exactly rail z12/2206/1391, got {} z{}/{tx}/{ty}",
                layer.dir(),
                cfg.z,
            );
            let mut delta = [0f64; noise_gpu::OUT_ARCSTAT_COUNTERS];
            for i in 0..noise_gpu::OUT_ARCSTAT_COUNTERS {
                delta[i] = current[i] - arcstat_seen[i];
            }
            let census = noise_gpu::validate_rail_port_arcstat_census(&delta)?;
            eprintln!(
                "ARCSTAT_TILE=PASS fixture=rail-2206-1391 quadrature_pairs={:.0} \
                 bucket_rays={:.0} buckets_per_gpu_pair={:.6} \
                 escalating_pairs={:.0} gpu_pair_escalation_frac={:.6} \
                 gpu_escalating_pairs_per_authority_pair={:.6} escalating_buckets={:.0} \
                 gpu_bucket_escalation_frac={:.6} \
                 gpu_escalating_buckets_per_authority_bucket_ray={:.6}",
                delta[0],
                delta[2],
                census.buckets_per_gpu_pair,
                delta[1],
                census.gpu_pair_escalation_frac,
                census.gpu_escalating_pairs_per_authority_pair,
                delta[3],
                census.gpu_bucket_escalation_frac,
                census.gpu_escalating_buckets_per_authority_bucket_ray,
            );
            arcstat_seen = current;
            RAIL_ARCSTAT_CENSUS_PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(feature = "v2-h0")]
        {
            let mut delta = [0_u64; noise_gpu::OUT_H0_COUNTERS];
            for index in 0..noise_gpu::OUT_H0_COUNTERS {
                let current = h0_exact_counter(&gpu, index);
                delta[index] = current
                    .checked_sub(h0_counts_seen[index])
                    .context("V2 H0 exact counter moved backwards")?;
                h0_counts_seen[index] = current;
            }
            let [node_overflow, hard_geometry, abi_layout, guarded_legal, completed_pairs, candidate_visits, generated_nodes, admitted_nodes] =
                delta;
            if node_overflow != 0 || hard_geometry != 0 || abi_layout != 0 {
                bail!(
                    "V2 H0 hard fault {} z{}/{tx}/{ty}: node_overflow={} hard_geometry={} \
                     abi_layout={} guarded_legal={}",
                    layer.dir(),
                    cfg.z,
                    node_overflow,
                    hard_geometry,
                    abi_layout,
                    guarded_legal,
                );
            }
            #[cfg(feature = "v2-h0-counters")]
            eprintln!(
                "H0STAT {} z{}/{tx}/{ty}: pairs={} candidates={} nodes={} admitted={} guarded={}",
                layer.dir(),
                cfg.z,
                completed_pairs,
                candidate_visits,
                generated_nodes,
                admitted_nodes,
                guarded_legal,
            );
            #[cfg(not(feature = "v2-h0-counters"))]
            let _ = (
                completed_pairs,
                candidate_visits,
                generated_nodes,
                admitted_nodes,
                guarded_legal,
            );
        }
        let output_started = Instant::now();
        let mut cells = if candidate_on {
            reconstruct_multifidelity_cells(
                candidate_interpolation
                    .as_ref()
                    .expect("multifidelity interpolation"),
                &exact_replay_output,
            )
        } else {
            let mut accum = TileAccumulator::new();
            accum
                .energy
                .copy_from_slice(&gpu[..noise_gpu::OUT_ENERGY_SLOTS]);
            collapse_lden_surface_u8(&accum)
        };
        // Building interiors: façade donor − ΔL, stamped after the collapse and
        // BEFORE the baseline diff so the comparison is like for like. One
        // estimate per tile (baked with the block), shared by its layers.
        if let Some(interior) = &interiors[batch_slot(batch, tx, ty)] {
            interior.apply(&mut cells);
        }
        let encode_done = Instant::now();
        stats.entry(layer.dir()).or_default().t_encode +=
            encode_done.duration_since(output_started).as_secs_f64();

        if let Some(root) = &cfg.output {
            let out = Path::new(root)
                .join(layer.dir())
                .join(cfg.z.to_string())
                .join(tx.to_string())
                .join(format!("{ty}.bin"));
            let bytes = write_tile(&out, &cells, layer.source_id(), true)?;
            if bytes > 0 {
                let st = stats.entry(layer.dir()).or_default();
                st.n_written += 1;
                st.bytes_written += bytes;
            } else {
                unlink_stale_tile(root, layer, cfg.z, tx, ty)?;
            }
            // `write_tile` includes Brotli encoding. Keep this one honest composite through the
            // stale-output unlink rather than presenting either operation as an isolated write.
            let write_done = Instant::now();
            stats.entry(layer.dir()).or_default().t_write +=
                write_done.duration_since(encode_done).as_secs_f64();
        }
        accumulate_baseline_diff(stats, layer, cfg, tx, ty, &cells)?;
        stats.entry(layer.dir()).or_default().n_tiles += 1;
        if tile_times_enabled() {
            let timing =
                noise_gpu::tile_timing::TileTimingRecord::new(tile_wall_s * 1000.0, tile_kernel_ms)
                    .expect("Instant and CUDA events must yield finite tile timing");
            eprintln!(
                "tile-time {} z{}/{tx}/{ty} {}",
                layer.dir(),
                cfg.z,
                timing.to_json().expect("validated tile timing serializes"),
            );
        }
        prog.tick();
    }
    if arc_drops_seen > 0.0 {
        eprintln!(
            "!! ARC OVERFLOW total for this block: {arc_drops_seen:.0} blocked arcs dropped — \
             the tiles named above under-screen. ARC_MAX_MERGED is sized from a measured \
             demand (kernels/scatter.cu); a nonzero count here means production geometry \
             has outgrown it."
        );
    }
    Ok(())
}

/// Crop the region's halo-blocks in the bounded double-buffered pipeline and
/// hand each ready chunk to `on_chunk`. The GPU sink (serial `process_block`,
/// or enqueue-to-tile-workers) is the caller's; crop order stays sorted so
/// output is byte-identical to the old build-everything-first path.
fn run_block_pipeline(
    region_tiles: &[(u32, u32)],
    cfg: &Cfg,
    prepared: &str,
    obstacle_data: &ObstacleData,
    mut on_chunk: impl FnMut(
        Vec<(
            (u32, u32),
            TileBatch,
            Vec<Option<InteriorEstimate>>,
            Vec<(u32, u32)>,
        )>,
        ChunkPermit,
    ) -> Result<()>,
) -> Result<std::time::Duration> {
    let mut blocks: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(tx, ty) in region_tiles {
        blocks
            .entry((
                (tx / cfg.batch_n) * cfg.batch_n,
                (ty / cfg.batch_n) * cfg.batch_n,
            ))
            .or_default()
            .push((tx, ty));
    }
    let block_keys: Vec<(u32, u32)> = blocks.keys().copied().collect();
    let window: usize = std::env::var("NOISE_GPU_PIPELINE_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&w: &usize| w >= 1)
        .unwrap_or(2);
    let raster_ns = std::sync::atomic::AtomicU64::new(0);
    type Block = ((u32, u32), TileBatch, Vec<Option<InteriorEstimate>>);
    type Chunk = (Vec<Block>, ChunkPermit);
    let build_chunk = |keys: &[(u32, u32)]| -> Chunk {
        let t0 = Instant::now();
        let estimate: u64 = keys
            .iter()
            .map(|&(bx, by)| {
                let (base_x, base_y) = block_batch_origin(bx, by, cfg.batch_n, cfg.z);
                TileBatch::estimate_heap_bytes(cfg.z, base_x, base_y, cfg.batch_n, cfg.halo_m)
            })
            .sum();
        let mut permit = pipeline_gate().acquire(estimate);
        let built: Vec<Block> = keys
            .par_iter()
            .map(|&(bx, by)| {
                RASTERS.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let rasters = slot.get_or_insert_with(|| RealRasters::new(Path::new(prepared)));
                    let (base_x, base_y) = block_batch_origin(bx, by, cfg.batch_n, cfg.z);
                    let mut batch = TileBatch::build_opt_rx_refl(
                        cfg.z,
                        base_x,
                        base_y,
                        cfg.batch_n,
                        cfg.halo_m,
                        rasters,
                    );
                    let mut interiors: Vec<Option<InteriorEstimate>> =
                        (0..batch.tiles.len()).map(|_| None).collect();
                    for &(tx, ty) in &blocks[&(bx, by)] {
                        let slot = batch_slot(&batch, tx, ty);
                        tile_painter::source_loader_obstacle::bake_tile_vector_rx_refl(
                            &mut batch.tiles[slot],
                            obstacle_data.set(),
                        );
                        interiors[slot] = Some(obstacle_data.interior_estimate(&batch.tiles[slot]));
                    }
                    ((bx, by), batch, interiors)
                })
            })
            .collect();
        permit.adjust_to(
            built
                .iter()
                .map(|(_, batch, interiors)| {
                    batch.heap_bytes()
                        + interiors
                            .iter()
                            .flatten()
                            .map(InteriorEstimate::heap_bytes)
                            .sum::<u64>()
                })
                .sum(),
        );
        raster_ns.fetch_add(
            t0.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        (built, permit)
    };
    let mut start = window.min(block_keys.len());
    let mut current: Option<Chunk> = Some(build_chunk(&block_keys[..start]));
    while current.as_ref().is_some_and(|(chunk, _)| !chunk.is_empty()) {
        let next_end = (start + window).min(block_keys.len());
        let next_range = start..next_end;
        let (built_next, gpu_err) = std::thread::scope(|scope| {
            let next_handle = (!next_range.is_empty())
                .then(|| scope.spawn(|| build_chunk(&block_keys[next_range.clone()])));
            let mut err = None;
            if let Some((chunk, permit)) = current.take() {
                let owned: Vec<_> = chunk
                    .into_iter()
                    .map(|(key, batch, interiors)| {
                        let tiles = blocks[&key].clone();
                        (key, batch, interiors, tiles)
                    })
                    .collect();
                // The callback owns the permit: batch path drops it after
                // process_block; the tile-queue path Arc's it onto each job so
                // halo bytes stay charged until the GPU worker drops the job
                // (PipelineByteGate: release when the GPU loop drops them).
                if let Err(e) = on_chunk(owned, permit) {
                    err = Some(e);
                }
            }
            let built = next_handle.map(|h| h.join().expect("chunk builder panicked"));
            (built, err)
        });
        if let Some(e) = gpu_err {
            drop(built_next);
            return Err(e);
        }
        current = built_next;
        start = next_end;
    }
    Ok(std::time::Duration::from_nanos(
        raster_ns.load(std::sync::atomic::Ordering::Relaxed),
    ))
}

/// Build one centre-R4 region's owned tiles for every layer — the shared body of the batch loop
/// and the --stream loop. Loads the region's grid_disk(1) rows + barriers once, uploads sources
/// once, crops blocks in parallel (each rayon worker on its own persistent RASTERS instance —
/// see the thread_local's decision-record comment), then runs the sequential GPU kernel loop.
/// Returns the cell's written/skipped tile-layers plus its shared-raster wall for the event line.
fn process_region(
    r4: u64,
    region_tiles: &[(u32, u32)],
    layers: &[LineLayer],
    cfg: &Cfg,
    dev: &Arc<CudaDevice>,
    functions: &LineFunctions,
    prepared: &str,
    stats: &mut BTreeMap<&'static str, LayerStat>,
    prog: &mut Progress,
) -> Result<RegionResult> {
    let total = region_tiles.len() * layers.len();
    let written0: usize = stats.values().map(|s| s.n_written).sum();
    let Some(host) = load_region_host(r4, layers, cfg, stats)? else {
        if let Some(root) = &cfg.output {
            for l in layers {
                let cleanup_started = Instant::now();
                let mut removed = 0usize;
                for &(tx, ty) in region_tiles {
                    let path = Path::new(root)
                        .join(l.dir())
                        .join(cfg.z.to_string())
                        .join(tx.to_string())
                        .join(format!("{ty}.bin"));
                    match std::fs::remove_file(&path) {
                        Ok(()) => removed += 1,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error)
                                .with_context(|| format!("rm stale {}", path.display()))
                        }
                    }
                }
                let stat = stats.entry(l.dir()).or_default();
                stat.t_cleanup += cleanup_started.elapsed().as_secs_f64();
                stat.n_cleanup_checked += region_tiles.len();
                stat.n_cleanup_removed += removed;
            }
        }
        prog.done += total;
        return Ok(RegionResult {
            written: 0,
            skipped: total,
            raster: std::time::Duration::ZERO,
        });
    };
    let (src_dev, obst_dev) = upload_host_sources(dev, &host, stats)?;
    let region_rows = &host.region_rows;
    let barrier_data = &host.barriers;
    let obstacle_data = &host.obstacles;
    let (census_tiles, census_removed) = drop_unreachable_tiles(
        region_tiles,
        &host.reach,
        layers,
        cfg.output.as_ref(),
        cfg.z,
    )?;
    // Those tile-layers are done: nothing will paint them and nothing will tick for
    // them later, so charge them here or the heartbeat never reaches its total and
    // a quiet-cell rebuild's removals vanish from the cleanup counters.
    prog.done += (region_tiles.len() - census_tiles.len()) * layers.len();
    for layer in layers {
        let stat = stats.entry(layer.dir()).or_default();
        stat.n_cleanup_checked += region_tiles.len() - census_tiles.len();
    }
    if let Some(layer) = layers.first() {
        stats.entry(layer.dir()).or_default().n_cleanup_removed += census_removed;
    }
    let raster = run_block_pipeline(
        &census_tiles,
        cfg,
        prepared,
        obstacle_data,
        |chunk, permit| {
            for (_key, batch, interiors, tiles) in &chunk {
                process_block(
                    dev,
                    functions,
                    batch,
                    interiors,
                    cfg,
                    tiles,
                    region_rows,
                    &host.reach,
                    &src_dev,
                    barrier_data,
                    &obst_dev,
                    stats,
                    prog,
                )?;
            }
            drop(permit);
            Ok(())
        },
    )?;
    let written: usize = stats.values().map(|s| s.n_written).sum::<usize>() - written0;
    Ok(RegionResult {
        written,
        skipped: total.saturating_sub(written),
        raster,
    })
}

struct TileJobPool {
    jobs: Mutex<VecDeque<GpuBlockJob>>,
    cv: Condvar,
    inflight: AtomicUsize,
    closed: Mutex<bool>,
}

impl TileJobPool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            jobs: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            inflight: AtomicUsize::new(0),
            closed: Mutex::new(false),
        })
    }

    fn wait_idle(&self) {
        let mut g = self.jobs.lock().unwrap();
        while self.inflight.load(Ordering::SeqCst) > 0 {
            g = self.cv.wait(g).unwrap();
        }
    }

    /// Decrement inflight under the same mutex `wait_idle` waits on, so a
    /// worker finishing between the idle-check and `cv.wait` cannot lose the
    /// wakeup.
    fn job_finished(&self) {
        let _g = self.jobs.lock().unwrap();
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    fn wait_below(&self, watermark: usize) -> Result<()> {
        let mut g = self.jobs.lock().unwrap();
        while self.inflight.load(Ordering::SeqCst) > watermark {
            g = self.cv.wait(g).unwrap();
        }
        Ok(())
    }

    fn close(&self) {
        let _g = self.jobs.lock().unwrap();
        *self.closed.lock().unwrap() = true;
        self.cv.notify_all();
    }
}

/// Closes the tile pool on every coordinator exit, including `?` and panic.
struct PoolCloseGuard {
    pool: Arc<TileJobPool>,
}

impl Drop for PoolCloseGuard {
    fn drop(&mut self) {
        self.pool.close();
    }
}

fn emit_cell_if_complete(cell: &CellGate, sink: &StreamSink) -> Result<()> {
    if !cell.crop_done.load(Ordering::SeqCst) {
        return Ok(());
    }
    if cell.remaining.load(Ordering::SeqCst) != 0 {
        return Ok(());
    }
    if cell
        .emitted
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    let fail_line = cell.failed.lock().unwrap().clone();
    if let Some(err) = fail_line {
        let wall = cell.t0.elapsed();
        let line = format!("fail {:x} {err}", cell.r4);
        let mut spans = EngineCellSpans::new(cell.r4, "gpu-surface", cell.worker_slot, cell.t0);
        spans.finish_failed(wall, &line);
        sink.evidence
            .region_terminal(
                cell.r4,
                cell.worker_slot,
                cell.interval_id,
                RegionTerminalStatus::Fail,
                0,
                0,
                Some(&line),
            )
            .expect("emit GPU surface region failure");
        if !write_stream_protocol_lines(sink, &spans.line(), &line) {
            return Err(anyhow::anyhow!("stream stdout closed"));
        }
        return Ok(());
    }
    let stats = cell.stats.lock().unwrap().clone();
    let written: usize = stats.values().map(|s| s.n_written).sum();
    let skipped = cell.total.saturating_sub(written);
    let wall = cell.t0.elapsed();
    let raster = *cell.raster.lock().unwrap();
    let mut spans = EngineCellSpans::new(cell.r4, "gpu-surface", cell.worker_slot, cell.t0);
    spans.metric_u64("owned_tiles", cell.tiles.len() as u64);
    spans.metric_bool("cuda_event_timing_enabled", timing_enabled());
    spans.metric_str(
        "effective_layers",
        &cell
            .effective
            .iter()
            .map(|l| l.dir())
            .collect::<Vec<_>>()
            .join(","),
    );
    if !raster.is_zero() {
        spans.push_aggregate_span("raster", raster, None, None, Some("surface-halo"));
    }
    let mut output_bytes = 0usize;
    for layer in &cell.effective {
        let name = layer.dir();
        let delta = stats.get(name).cloned().unwrap_or_default();
        if delta.t_load > 0.0 {
            spans.push_aggregate_span(
                "source_load",
                std::time::Duration::from_secs_f64(delta.t_load),
                Some(1),
                None,
                Some(name),
            );
        }
        if delta.t_bins > 0.0 {
            spans.push_aggregate_span(
                "cpu_prepare",
                std::time::Duration::from_secs_f64(delta.t_bins),
                None,
                None,
                Some(name),
            );
        }
        if delta.t_h2d > 0.0 {
            spans.push_aggregate_span(
                "h2d",
                std::time::Duration::from_secs_f64(delta.t_h2d),
                None,
                None,
                Some(name),
            );
        }
        if delta.kernel_calls > 0 {
            spans.push_cuda_span(
                "gpu_kernel",
                std::time::Duration::from_secs_f64((delta.kernel_ms / 1_000.0).max(0.0)),
                Some(delta.kernel_calls as u64),
                Some(name),
            );
        }
        if delta.n_tiles > 0 {
            spans.push_aggregate_span(
                "gpu_pipeline_composite",
                std::time::Duration::from_secs_f64(delta.t_kernel.max(0.0)),
                Some(delta.n_tiles as u64),
                None,
                Some(name),
            );
            spans.push_aggregate_span(
                "encode",
                std::time::Duration::from_secs_f64(delta.t_encode.max(0.0)),
                Some(delta.n_tiles as u64),
                None,
                Some(name),
            );
        }
        if delta.t_write > 0.0 {
            spans.push_aggregate_span(
                "encode_write_composite",
                std::time::Duration::from_secs_f64(delta.t_write),
                Some(delta.n_tiles as u64),
                Some(delta.bytes_written as u64),
                Some(name),
            );
        }
        output_bytes += delta.bytes_written;
    }
    spans.finish_done(wall, written, skipped, Some(output_bytes));
    if sink.evidence.is_enabled() {
        let output_root = sink
            .cfg
            .output
            .as_deref()
            .map(Path::new)
            .expect("GPU surface evidence requires --output");
        for &(x, y) in &cell.tiles {
            for layer in &cell.effective {
                let output = output_root
                    .join(layer.dir())
                    .join(sink.z.to_string())
                    .join(x.to_string())
                    .join(format!("{y}.bin"));
                sink.evidence
                    .tile_terminal(
                        cell.r4,
                        layer.dir(),
                        sink.z,
                        x,
                        y,
                        output_root,
                        &output,
                        "all-periods-silent",
                    )
                    .expect("emit GPU surface tile terminal");
            }
        }
    }
    sink.evidence
        .region_terminal(
            cell.r4,
            cell.worker_slot,
            cell.interval_id,
            RegionTerminalStatus::Done,
            written,
            skipped,
            None,
        )
        .expect("emit GPU surface region terminal");
    let line = format!(
        "done {:x} {} {} {}",
        cell.r4,
        written,
        skipped,
        wall.as_millis()
    );
    if !write_stream_protocol_lines(sink, &spans.line(), &line) {
        return Err(anyhow::anyhow!("stream stdout closed"));
    }
    Ok(())
}

fn process_region_via_tile_pool(
    region_tiles: &[(u32, u32)],
    cfg: &Cfg,
    prepared: &str,
    host: Arc<HostRegion>,
    cell: &Arc<CellGate>,
    pool: &TileJobPool,
    watermark: usize,
) -> Result<std::time::Duration> {
    let layers: Vec<LineLayer> = host.region_rows.iter().map(|(l, _)| *l).collect();
    let (census_tiles, _removed) = drop_unreachable_tiles(
        region_tiles,
        &host.reach,
        &layers,
        cfg.output.as_ref(),
        cfg.z,
    )?;
    run_block_pipeline(
        &census_tiles,
        cfg,
        prepared,
        &host.obstacles,
        |chunk, permit| {
            if chunk.is_empty() {
                drop(permit);
                return Ok(());
            }
            let permit = Arc::new(permit);
            cell.remaining.fetch_add(chunk.len(), Ordering::SeqCst);
            {
                let mut q = pool.jobs.lock().unwrap();
                pool.inflight.fetch_add(chunk.len(), Ordering::SeqCst);
                for (_key, batch, interiors, tiles) in chunk {
                    q.push_back(GpuBlockJob {
                        host: Arc::clone(&host),
                        cell: Arc::clone(cell),
                        batch,
                        interiors,
                        block_tiles: tiles,
                        permit: Arc::clone(&permit),
                    });
                }
            }
            pool.cv.notify_all();
            pool.wait_below(watermark)
        },
    )
}

fn finish_block_job(
    cell: &CellGate,
    local_stats: &mut BTreeMap<&'static str, LayerStat>,
    pool: &TileJobPool,
    sink: &StreamSink,
) {
    let taken = std::mem::take(local_stats);
    {
        let mut cell_stats = cell.stats.lock().unwrap();
        for (k, v) in taken {
            cell_stats.entry(k).or_default().absorb(v);
        }
    }
    cell.remaining.fetch_sub(1, Ordering::SeqCst);
    pool.job_finished();
    let _ = emit_cell_if_complete(cell, sink);
}

fn tile_worker_loop(pool: &TileJobPool, cfg: &Cfg, sink: &StreamSink) {
    let (dev, functions) = warm_device_on(true);
    let mut src_dev: Vec<LayerSrc> = Vec::new();
    let mut obst_dev: Option<ObstDev> = None;
    let mut cached_r4 = None;
    let mut local_stats: BTreeMap<&'static str, LayerStat> = BTreeMap::new();
    let mut prog = Progress {
        done: 0,
        total: 0,
        last_beat: Instant::now(),
    };
    loop {
        let job = {
            let mut g = pool.jobs.lock().unwrap();
            loop {
                if let Some(job) = g.pop_front() {
                    break Some(job);
                }
                if *pool.closed.lock().unwrap() {
                    break None;
                }
                g = pool.cv.wait(g).unwrap();
            }
        };
        let Some(job) = job else { break };
        let mut account = JobAccountGuard {
            cell: Arc::clone(&job.cell),
            pool,
            sink,
            armed: true,
        };
        if job.cell.failed.lock().unwrap().is_some() {
            account.armed = false;
            finish_block_job(&job.cell, &mut local_stats, pool, sink);
            continue;
        }
        if cached_r4 != Some(job.host.r4) {
            match upload_host_sources(&dev, &job.host, &mut local_stats) {
                Ok((src, obst)) => {
                    src_dev = src;
                    obst_dev = Some(obst);
                    cached_r4 = Some(job.host.r4);
                }
                Err(e) => {
                    *job.cell.failed.lock().unwrap() = Some(format!("{e:#}"));
                    account.armed = false;
                    finish_block_job(&job.cell, &mut local_stats, pool, sink);
                    continue;
                }
            }
        }
        let obst = obst_dev.as_ref().expect("uploaded obstacles");
        if let Err(e) = process_block(
            &dev,
            &functions,
            &job.batch,
            &job.interiors,
            cfg,
            &job.block_tiles,
            &job.host.region_rows,
            &job.host.reach,
            &src_dev,
            &job.host.barriers,
            obst,
            &mut local_stats,
            &mut prog,
        ) {
            *job.cell.failed.lock().unwrap() = Some(format!("{e:#}"));
        }
        account.armed = false;
        finish_block_job(&job.cell, &mut local_stats, pool, sink);
    }
}

/// STREAM mode (`--stream`): the persistent warm surface worker the cluster orchestrator feeds.
/// CUDA context + scatter PTX + admin table resident. Each R4 cell is loaded once
/// (rows, packed sources, barriers, obstacles, cropped halo) into a shared
/// halo-block queue; `QM_GPU_STREAM_WORKERS` GPU threads pull those blocks.
/// A region's shared load is independent of which stream runs which of its
/// owned tiles — true for every cell. Cells overlap in the queue so a dense
/// cell's remaining tiles keep every worker busy after lighter cells drain.
/// Reads output R4 cell IDs (one hex/line), prints `start <r4hex> <unix_ms>` before work, builds
/// each cell's owned tiles, prints one `engine-spans-v1 {json}` evidence line, then `done <r4hex>
/// <written> <skipped> <ms>` (or `fail <r4hex> <err>`) — the same protocol as gpu-airborne, so the
/// agent drives either identically.
fn run_stream(
    z: u8,
    layers: &[LineLayer],
    halo_m: f64,
    batch_n: u32,
    h3r4: PathBuf,
    baseline: String,
    output: Option<String>,
    prepared: &str,
) -> Result<()> {
    let barriers_enabled = gpu_barriers_enabled();
    let cfg = Cfg {
        z,
        batch_n,
        halo_m,
        h3r4,
        baseline,
        output,
        barriers_enabled,
    };
    // FIXED N (default 2, NOT rayon thread count): each worker holds per-tile GPU
    // scratch on its own stream. The cell itself is loaded once and its owned
    // tiles are independent jobs over that shared payload — true for every
    // cell on Earth, not a per-run rebalance. 2 fits the 12 GB cards;
    // QM_GPU_STREAM_WORKERS overrides.
    let n_workers: usize = env("QM_GPU_STREAM_WORKERS", "2")
        .parse()
        .unwrap_or(2)
        .max(1);
    let names: Vec<&str> = layers.iter().map(|l| l.dir()).collect();
    let evidence = RendererEvidence::from_env(
        "gpu-surface",
        RuntimeParameters {
            zoom: z,
            batch_size: batch_n,
            n_days: None,
            rayon_threads: rayon::current_num_threads(),
            stream_workers: n_workers,
            region_concurrency_configured: 1,
            region_concurrency_effective: 1,
            max_regions_per_claim: 1,
            layers: names.iter().map(|name| (*name).to_string()).collect(),
        },
    )?;
    eprintln!(
        "stream: layers={names:?}, halo={halo_m:.0}m, batch={batch_n}, {n_workers} tile-worker(s) — cells share a halo-block queue; any worker may run any tile of a loaded region"
    );

    // Stdin is a cell queue (Morton order from the orchestrator). The coordinator
    // loads each cell once and enqueues its halo-blocks; GPU workers pull from
    // that shared queue (cells overlap up to the inflight watermark). The
    // optional `Vec<String>` is the stdin line's `layers=` token
    // (paint-pipeline-v4 PR#1 §3) — `None` = build every configured layer.
    let work: StreamCellQueue = Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
    let out = Arc::new(Mutex::new(std::io::stdout()));

    let reader_work = Arc::clone(&work);
    // DETACHED (not joined): on a broken-pipe abort the workers exit while this thread may still be
    // blocked in stdin.lines() with stdin open — joining it would deadlock main.
    // On normal EOF it sets closed + returns; either way the OS reaps it when main returns.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let s = line.trim();
            if s.is_empty() {
                continue;
            }
            let (hex, req_layers) = split_stream_line(s);
            match u64::from_str_radix(hex, 16) {
                Ok(r4) => {
                    let (lock, cv) = &*reader_work;
                    lock.lock().unwrap().0.push_back((
                        r4,
                        req_layers.map(|v| v.into_iter().map(str::to_string).collect()),
                    ));
                    cv.notify_one();
                }
                Err(_) => eprintln!("stream: skip non-hex line: {s}"),
            }
        }
        let (lock, cv) = &*reader_work;
        lock.lock().unwrap().1 = true; // EOF → wake workers to drain the tail + exit
        cv.notify_all();
    });

    let cfg = Arc::new(cfg);
    let pool = TileJobPool::new();
    let sink = Arc::new(StreamSink {
        out: Arc::clone(&out),
        evidence: evidence.clone(),
        cfg: Arc::clone(&cfg),
        z,
        work: Arc::clone(&work),
    });
    let watermark = n_workers.saturating_mul(2).max(2);
    std::thread::scope(|scope| -> Result<()> {
        let _close = PoolCloseGuard {
            pool: Arc::clone(&pool),
        };
        for _ in 0..n_workers {
            let pool = Arc::clone(&pool);
            let cfg = Arc::clone(&cfg);
            let sink = Arc::clone(&sink);
            scope.spawn(move || tile_worker_loop(&pool, &cfg, &sink));
        }

        let worker_slot = 0usize; // one coordinator claims every cell; n_workers is tile-block concurrency
        loop {
            let cell: Option<(u64, Option<Vec<String>>)> = {
                let (lock, cv) = &*work;
                let mut g = lock.lock().unwrap();
                loop {
                    if let Some(cell) = g.0.pop_front() {
                        break Some(cell);
                    }
                    if g.1 {
                        break None;
                    }
                    g = cv.wait(g).unwrap();
                }
            };
            let Some((r4, req_layers)) = cell else { break };
            let interval_id = evidence
                .region_claim(r4, worker_slot)
                .expect("emit GPU surface region claim");
            announce_stream_cell_started(r4);
            let t = Instant::now();
            let tiles = region_tiles(r4, z);
            let (effective, skipped) =
                split_configured_layers(layers, req_layers.as_deref(), |l| l.dir());
            if effective.is_empty() {
                let line = format!(
                    "fail {r4:x} layers-request matches none of configured [{}]",
                    skipped.join(",")
                );
                if !emit_stream_fail(&sink, r4, worker_slot, interval_id, t, &line) {
                    break;
                }
                continue;
            }
            let effective_names: Vec<&str> = effective.iter().map(|layer| layer.dir()).collect();
            if let Err(e) = evidence.region_dependencies(
                r4,
                Path::new(prepared),
                &cfg.h3r4,
                &tiles,
                z,
                cfg.halo_m,
                &effective_names,
                DependencyProfile::Surface,
            ) {
                let line = format!("fail {r4:x} {e:#}");
                if !emit_stream_fail(&sink, r4, worker_slot, interval_id, t, &line) {
                    break;
                }
                continue;
            }
            let mut load_stats = BTreeMap::new();
            let host = match load_region_host(r4, &effective, &cfg, &mut load_stats) {
                Ok(host) => host,
                Err(e) => {
                    let line = format!("fail {r4:x} {e:#}");
                    if !emit_stream_fail(&sink, r4, worker_slot, interval_id, t, &line) {
                        break;
                    }
                    continue;
                }
            };
            let cell = Arc::new(CellGate {
                r4,
                remaining: AtomicUsize::new(0),
                crop_done: AtomicBool::new(false),
                emitted: AtomicBool::new(false),
                failed: Mutex::new(None),
                stats: Mutex::new(BTreeMap::new()),
                total: tiles.len() * effective.len(),
                t0: t,
                raster: Mutex::new(std::time::Duration::ZERO),
                interval_id,
                worker_slot,
                tiles: tiles.clone(),
                effective: effective.clone(),
            });
            {
                let mut cell_stats = cell.stats.lock().unwrap();
                for (k, v) in load_stats {
                    cell_stats.entry(k).or_default().absorb(v);
                }
            }
            let Some(host) = host else {
                if let Some(root) = &cfg.output {
                    for l in &effective {
                        let cleanup_started = Instant::now();
                        let mut removed = 0usize;
                        for &(tx, ty) in &tiles {
                            let path = Path::new(root)
                                .join(l.dir())
                                .join(cfg.z.to_string())
                                .join(tx.to_string())
                                .join(format!("{ty}.bin"));
                            match std::fs::remove_file(&path) {
                                Ok(()) => removed += 1,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => {
                                    *cell.failed.lock().unwrap() =
                                        Some(format!("rm stale {}: {error}", path.display()));
                                    break;
                                }
                            }
                        }
                        let mut cell_stats = cell.stats.lock().unwrap();
                        let stat = cell_stats.entry(l.dir()).or_default();
                        stat.t_cleanup += cleanup_started.elapsed().as_secs_f64();
                        stat.n_cleanup_checked += tiles.len();
                        stat.n_cleanup_removed += removed;
                    }
                }
                cell.crop_done.store(true, Ordering::SeqCst);
                if emit_cell_if_complete(&cell, &sink).is_err() {
                    break;
                }
                continue;
            };
            let raster = match process_region_via_tile_pool(
                &tiles,
                &cfg,
                prepared,
                Arc::new(host),
                &cell,
                &pool,
                watermark,
            ) {
                Ok(raster) => raster,
                Err(e) => {
                    *cell.failed.lock().unwrap() = Some(format!("{e:#}"));
                    std::time::Duration::ZERO
                }
            };
            *cell.raster.lock().unwrap() = raster;
            cell.crop_done.store(true, Ordering::SeqCst);
            if emit_cell_if_complete(&cell, &sink).is_err() {
                break;
            }
        }
        pool.wait_idle();
        Ok(())
    })?;
    Ok(())
}

/// `QM_GPU_BARRIERS` — vector noise-wall screening on the GPU lanes.
/// DEFAULT ON in the engine itself (owner directive 2026-06-13: every GPU
/// surface build screens its own barriers): the ON default used to live only
/// in the v1 cluster wrapper, the v2 orchestrator rewrite lost the env, and
/// every fleet GPU road/rail paint until 2026-08-02 ran wall-blind (caught at
/// Voznice: D4 walls in barriers.arrow, absent from the tiles). `=0` stays
/// the explicit barrier-blind baseline (tests pass the flag programmatically).
fn gpu_barriers_enabled() -> bool {
    let enabled = env("QM_GPU_BARRIERS", "1") == "1";
    if enabled {
        eprintln!("QM_GPU_BARRIERS=1 — kernel vector-barrier screening ENABLED");
    }
    enabled
}

fn main() -> Result<()> {
    // The PRODUCTION painter. A CPU-only lever left in a worker's environment
    // would paint the world with the GPU's shipped rule while every CPU-side
    // check ran under a different one — silently, for as long as it was set.
    noise_gpu::ensure_no_cpu_only_arc_levers()?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Index-based parse: each known flag consumes the NEXT token (which must exist
    // and not itself be a flag); everything else is a positional. Tracking by
    // position (not value) avoids dropping a positional that equals a flag's value.
    let (mut output, mut bbox, mut layers_s, mut batch_s, mut regions_file) =
        (None, None, None, None, None);
    let mut zoom_s: Option<String> = None;
    let mut stream = false;
    let mut pos: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "--stream" => {
                stream = true;
                i += 1;
            }
            "--output" | "--bbox" | "--layers" | "--batch" | "--regions-file" | "--zoom" => {
                let v = argv
                    .get(i + 1)
                    .filter(|s| !s.starts_with("--"))
                    .cloned()
                    .with_context(|| format!("{a} needs a value"))?;
                match a {
                    "--output" => output = Some(v),
                    "--bbox" => bbox = Some(v),
                    "--layers" => layers_s = Some(v),
                    "--regions-file" => regions_file = Some(v),
                    "--zoom" => zoom_s = Some(v),
                    _ => batch_s = Some(v),
                }
                i += 2;
            }
            s if s.starts_with("--") => bail!("unknown flag {s}"),
            _ => {
                pos.push(argv[i].clone());
                i += 1;
            }
        }
    }
    let layers: Vec<LineLayer> = layers_s
        .as_deref()
        .unwrap_or("rail")
        .split(',')
        .map(LineLayer::parse)
        .collect::<Result<_>>()?;

    // 512px tiles: z12 is the world base (same lattice as the old z13@256);
    // higher zooms build refinement tiers (city-z13 plan). Block/tile math
    // downstream is zoom-parametric already; the bound matches tile-painter's.
    let z: u8 = match zoom_s.as_deref() {
        Some(s) => {
            let z: u8 = s.parse().context("--zoom must be an integer")?;
            if !(6..=18).contains(&z) {
                bail!("--zoom {z} out of range 6..=18");
            }
            z
        }
        None => 12,
    };
    let z13_profile = multifidelity_z13_profile()?;
    let stock_cartesian_anchor = multifidelity_cartesian_unbinned_anchor_enabled();
    anyhow::ensure!(
        stock_cartesian_anchor
            == z13_profile.is_some_and(|profile| profile.stride == MultifidelityStride::Stride4),
        "compiled Cartesian unbinned marker does not match the stride4 z13 profile"
    );
    if let Some(profile) = z13_profile {
        if !multifidelity_line_enabled() {
            bail!("z13 profile marker exists without the reviewed W1 multifidelity marker");
        }
        if z != 13 {
            bail!(
                "z13 profile stride={} requires --zoom 13, got --zoom {z}",
                profile.stride.pixels()
            );
        }
        if stock_cartesian_anchor {
            eprintln!(
                "MULTIFIDELITY_PROFILE zoom={z} stride={} adaptive=0 cpu_reference_profile=0 presence=cheap exact_backend=candidate_cartesian_unbinned module=candidate_aot_only anchor_launches={} anchor_blocks={} computed_receivers={} anchors={}",
                profile.stride.pixels(),
                MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS,
                MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS
                    * MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS,
                MULTIFIDELITY_STOCK_CARTESIAN_COMPUTED_RECEIVERS,
                profile.stride.anchor_record_count(),
            );
        } else {
            eprintln!(
                "MULTIFIDELITY_PROFILE zoom={z} stride={} adaptive=0 cpu_reference_profile=0 presence=cheap exact_backend=compact_packed packed_warps={}",
                profile.stride.pixels(),
                MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK,
            );
        }
    }
    let require_arcstat = rail_arcstat_census_required();
    if require_arcstat {
        anyhow::ensure!(
            !cfg!(feature = "v2-h0"),
            "rail ARCSTAT census is only defined for the stock surface role"
        );
        anyhow::ensure!(!stream, "rail ARCSTAT census refuses --stream");
        anyhow::ensure!(
            layers.as_slice() == [LineLayer::Rail] && z == 12,
            "rail ARCSTAT census requires --layers rail --zoom 12"
        );
        anyhow::ensure!(
            std::env::var_os(RENDERER_STATIC_ATTEST_FLAG).is_none(),
            "rail ARCSTAT census refuses static renderer attestation"
        );
    }
    let prepared = env("NOISE_GPU_PREPARED", "/dev/shm/qmap/prepared");
    let baseline = env("NOISE_GPU_BASELINE", ""); // empty ⇒ no diff (production)
    let year = env("DATA_YEAR", "2026");
    let h3r4 = PathBuf::from(format!("{prepared}/{year}/h3r4"));
    // Shared halo = the widest reach among the requested layers (road 10 km),
    // overridable for benchmarking; a shorter-reach layer culls at its own reach.
    let halo_m: f64 = match std::env::var("NOISE_GPU_HALO_M") {
        Ok(v) => v.parse()?,
        Err(_) => layers.iter().map(|l| l.halo_m()).fold(0.0, f64::max),
    };
    let batch_n: u32 = match batch_s {
        Some(s) => s.parse()?,
        None => default_batch_size(),
    };
    if batch_n == 0 {
        bail!("--batch / block size must be >= 1");
    }
    let static_workers: usize = env("QM_GPU_STREAM_WORKERS", "2")
        .parse()
        .unwrap_or(2)
        .max(1);
    let static_layers: Vec<String> = layers.iter().map(|layer| layer.dir().to_string()).collect();
    if maybe_run_static_attestation(
        "gpu-surface",
        StaticAttestationParameters {
            runtime: RuntimeParameters {
                zoom: z,
                batch_size: batch_n,
                n_days: None,
                rayon_threads: rayon::current_num_threads(),
                stream_workers: static_workers,
                region_concurrency_configured: static_workers,
                region_concurrency_effective: static_workers,
                max_regions_per_claim: 1,
                layers: static_layers.clone(),
            },
            accepted_options: [
                "--batch/1",
                "--bbox/1",
                "--layers/1",
                "--output/1",
                "--regions-file/1",
                "--stream/0",
                "--zoom/1",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            prepared_root: PathBuf::from(&prepared),
            h3r4_dir: h3r4.clone(),
            halo_m,
            layers: static_layers,
            profile: DependencyProfile::Surface,
        },
    )? {
        return Ok(());
    }

    // Init when EITHER line layer is built: road needs default-AADT, rail needs
    // the C1 per-region period split — a rail-only run on Admin::UNKNOWN would
    // take the world split and break popup parity.
    if layers.contains(&LineLayer::Road) || layers.contains(&LineLayer::Rail) {
        let result = admin::init_admin_table(&admin::default_admin_path(&h3r4));
        if std::env::var(RENDERER_EVIDENCE_FLAG).as_deref() == Ok("1") {
            result.context("renderer evidence requires the physical admin table")?;
        }
    }

    if stream {
        return run_stream(
            z, &layers, halo_m, batch_n, h3r4, baseline, output, &prepared,
        );
    }

    // Target tiles → grouped by centre R4 so each region's rows load ONCE
    // (~36 tiles share an R4), not re-read per tile; each region then batches into
    // grid-aligned halo blocks. Build `regions` directly in both modes so
    // n_targets counts exactly the valid tiles that get built.
    let mut regions: BTreeMap<u64, Vec<(u32, u32)>> = BTreeMap::new();
    let (blk_n, mode) = if let Some(rf) = &regions_file {
        // Cluster per-chunk unit: build exactly the listed output R4s' owned tiles
        // (centre-R4 ownership, same contract as build_heatmap_surface --regions-file).
        for r4 in read_r4_file(Path::new(rf))? {
            regions.insert(r4, region_tiles(r4, z));
        }
        (
            batch_n,
            format!("regions-file {rf} ({} R4s)", regions.len()),
        )
    } else if let Some(b) = &bbox {
        let v: Vec<f64> = b.split(',').map(|s| s.parse()).collect::<Result<_, _>>()?;
        if v.len() != 4 || v[0] >= v[2] || v[1] >= v[3] {
            bail!("--bbox needs south,west,north,east with south<north and west<east");
        }
        let (xr, yr) = tile_range(z, v[0], v[1], v[2], v[3]);
        for ty in yr {
            for tx in xr.clone() {
                if let Some(r4) = tile_centre_r4(z, tx, ty) {
                    regions.entry(r4).or_default().push((tx, ty));
                }
            }
        }
        (batch_n, format!("bbox {b}"))
    } else {
        let (bx_in, by_in): (u32, u32) = (
            pos.first().context("need <base_x> or --bbox")?.parse()?,
            pos.get(1).context("need <base_y>")?.parse()?,
        );
        let bn: u32 = match pos.get(2) {
            Some(s) => s.parse()?,
            None => 4,
        };
        if bn == 0 {
            bail!("block size must be >= 1");
        }
        // Snap to the grid the bbox/CPU runners batch on, so a dev block's shared
        // halo matches theirs (else diffing vs an aligned baseline drifts spuriously).
        let (base_x, base_y) = ((bx_in / bn) * bn, (by_in / bn) * bn);
        if (base_x, base_y) != (bx_in, by_in) {
            eprintln!(
                "note: snapped block origin {bx_in}/{by_in} → {base_x}/{base_y} (grid-aligned)"
            );
        }
        for dy in 0..bn {
            for dx in 0..bn {
                if let Some(r4) = tile_centre_r4(z, base_x + dx, base_y + dy) {
                    regions
                        .entry(r4)
                        .or_default()
                        .push((base_x + dx, base_y + dy));
                }
            }
        }
        (bn, format!("block {base_x}/{base_y} n={bn}"))
    };
    if regions.is_empty() {
        bail!("no tiles to build (no valid z{z} tiles in range)");
    }
    let n_targets: usize = regions.values().map(Vec::len).sum();
    if require_arcstat {
        let targets: Vec<(u32, u32)> = regions.values().flatten().copied().collect();
        anyhow::ensure!(
            targets.as_slice() == [(2206, 1391)],
            "rail ARCSTAT census requires exactly tile 2206/1391, got {targets:?}"
        );
    }
    let layer_names: Vec<&str> = layers.iter().map(|l| l.dir()).collect();
    eprintln!(
        "{mode} | {} region(s), {n_targets} tile(s), layers={:?}, halo={halo_m:.0} m, batch={blk_n}",
        regions.len(),
        layer_names,
    );

    let (dev, functions) = warm_device();

    let barriers_enabled = gpu_barriers_enabled();
    let cfg = Cfg {
        z,
        batch_n: blk_n,
        halo_m,
        h3r4,
        baseline,
        output: output.clone(),
        barriers_enabled,
    };
    let mut stats: BTreeMap<&'static str, LayerStat> = BTreeMap::new();
    let mut prog = Progress {
        done: 0,
        total: n_targets * layers.len(),
        last_beat: Instant::now(),
    };
    let t_all = Instant::now();
    for (&r4, region_tiles) in &regions {
        process_region(
            r4,
            region_tiles,
            &layers,
            &cfg,
            &dev,
            &functions,
            &prepared,
            &mut stats,
            &mut prog,
        )?;
    }
    let wall = t_all.elapsed().as_secs_f64();

    eprintln!(
        "=== {n_targets} tile(s) × {} layer(s) in {wall:.2}s ===",
        layers.len()
    );
    for (name, s) in &stats {
        // gpu = the GPU-phase wall (upload + launch + sync); prep = bin + pack. The
        // pipeline OVERLAPS prep(N+1) with gpu(N), so wall < gpu + prep — the
        // top-line wall is the real cost, these are diagnostic, not additive.
        // kernel = the CUDA-event isolated launch time (NOISE_GPU_TIMING=1 only).
        let kernel_ms = if timing_enabled() {
            format!(" | kernel {:.1} ms", s.kernel_ms)
        } else {
            String::new()
        };
        eprintln!(
            "  [{name}] {} tiles | gpu {:.0} ms | prep {:.0} ms | load {:.0} ms | written {} (gpu∥prep){kernel_ms}",
            s.n_tiles,
            s.t_kernel * 1e3,
            s.t_bins * 1e3,
            s.t_load * 1e3,
            s.n_written,
        );
        if s.n_baseline > 0 {
            let denom = s.n_cmp.max(1) as f64;
            eprintln!(
                "          vs baseline: {} tiles, max {}B ({:.1} dB), {} cells differ | ≤1B {:.3}% ≤3B {:.3}% of {} cmp",
                s.n_baseline,
                s.max_diff,
                s.max_diff as f64 * 0.5,
                s.n_diff,
                100.0 * s.n_le1 as f64 / denom,
                100.0 * s.n_le3 as f64 / denom,
                s.n_cmp,
            );
        }
    }
    // Machine-readable total kernel time (CUDA events, all layers) for the --micro
    // harness's median-of-N: one `KERNEL_MS=<total>` token it greps per run. Only
    // when timing is on, so a normal build emits nothing extra.
    if timing_enabled() {
        let total_kernel_ms: f64 = stats.values().map(|s| s.kernel_ms).sum();
        eprintln!("KERNEL_MS={total_kernel_ms:.3}");
    }
    if let Some(root) = &output {
        eprintln!("  → HM3 under {root}/{{{}}}/{z}", layer_names.join(","));
    }
    if require_arcstat {
        let passes = RAIL_ARCSTAT_CENSUS_PASSES.load(std::sync::atomic::Ordering::Relaxed);
        anyhow::ensure!(
            passes == 1,
            "rail ARCSTAT census requires exactly one completed fixture, got {passes}"
        );
        eprintln!("ARCSTAT_GATE=PASS fixture=rail-2206-1391 passes=1");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod multifidelity_tests {
    use super::*;

    fn compact_anchor_output(
        axis: &[usize],
        energy: impl Fn(usize, usize, usize) -> f32,
    ) -> Vec<f32> {
        let stride = noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE;
        let mut output = vec![0.0f32; axis.len() * axis.len() * stride];
        for (ay, &py) in axis.iter().enumerate() {
            for (ax, &px) in axis.iter().enumerate() {
                let record = ay * axis.len() + ax;
                let offset = record * stride;
                output[offset + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT] =
                    (py * TILE_PX + px) as f32;
                for period in 0..NUM_PERIODS {
                    output[offset + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE + period] =
                        energy(py, px, period);
                }
            }
        }
        output
    }

    fn complete_stock_cartesian_dense_group(
        plan: &MultifidelityStockCartesianPlan,
        group: usize,
    ) -> Vec<f32> {
        let mut output = multifidelity_stock_cartesian_output_prefix_sentinel();
        let group_range = plan.latitude_group_range(group).expect("group range");
        let launched_columns = MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS;
        for synthetic_y in 0..BIN_W {
            for synthetic_x in 0..launched_columns {
                let source = (synthetic_y * TILE_PX + synthetic_x) * NUM_PERIODS;
                for period in 0..NUM_PERIODS {
                    output[source + period] = (source + period + 1) as f32;
                }
            }
        }
        for (synthetic_y, axis_y) in group_range.enumerate() {
            for axis_x in 0..plan.axis.len() {
                let source = (synthetic_y * TILE_PX + axis_x) * NUM_PERIODS;
                let record = axis_y * plan.axis.len() + axis_x;
                for period in 0..NUM_PERIODS {
                    output[source + period] = (record * NUM_PERIODS + period + 1) as f32;
                }
            }
        }
        output
    }

    #[test]
    fn stride_anchor_axis_carries_the_final_boundary() {
        let stride = MultifidelityStride::Stride16;
        let axis = multifidelity_anchor_axis(stride);
        assert_eq!(stride.pixels(), 16);
        assert_eq!(axis.len(), stride.anchor_count());
        assert_eq!(axis.len(), 33);
        assert_eq!(axis[axis.len() - 2], 496);
        assert_eq!(axis.last().copied(), Some(TILE_PX - 1));
        assert_eq!(axis, (0..=496).step_by(16).chain([511]).collect::<Vec<_>>());
        assert!(multifidelity_is_anchor(stride, 496, 496));
        assert!(multifidelity_is_anchor(stride, TILE_PX - 1, TILE_PX - 1));
        assert!(!multifidelity_is_anchor(stride, 495, 495));
    }

    #[test]
    fn runtime_strides_cover_boundary_and_expected_record_counts() {
        for (pixels, axis_count, record_count, penultimate) in [
            (4, 129, 16_641, 508),
            (8, 65, 4_225, 504),
            (16, 33, 1_089, 496),
            (32, 17, 289, 480),
        ] {
            let stride = MultifidelityStride::from_pixels(pixels).expect("supported stride");
            let axis = multifidelity_anchor_axis(stride);
            assert_eq!(stride.pixels(), pixels);
            assert_eq!(axis.len(), axis_count);
            assert_eq!(stride.anchor_record_count(), record_count);
            assert_eq!(axis[axis.len() - 2], penultimate);
            assert_eq!(axis.last().copied(), Some(TILE_PX - 1));
            assert!(multifidelity_is_anchor(stride, TILE_PX - 1, TILE_PX - 1));
            assert!(multifidelity_is_anchor(stride, penultimate, penultimate));
            assert!(!multifidelity_is_anchor(
                stride,
                penultimate - 1,
                penultimate - 1
            ));
            assert_eq!(
                stride.compact_output_len(),
                record_count * noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE
            );
        }
        for unsupported in [0, 1, 2, 3, 5, 12, 24, 511] {
            assert_eq!(MultifidelityStride::from_pixels(unsupported), None);
        }
    }

    #[test]
    fn cartesian_unbinned_stride4_plan_is_nine_waves_and_covers_every_anchor() {
        let plan = MultifidelityStockCartesianPlan::stride4().expect("stride4 Cartesian plan");
        assert_eq!(plan.axis.len(), 129);
        assert_eq!(plan.anchor_record_count(), 16_641);
        assert_eq!(MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS, 9);
        assert_eq!(MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS, 9);
        assert_eq!(MULTIFIDELITY_STOCK_CARTESIAN_COMPUTED_RECEIVERS, 20_736);

        let ranges = (0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS)
            .map(|group| plan.latitude_group_range(group).expect("group range"))
            .collect::<Vec<_>>();
        assert_eq!(ranges.first().cloned(), Some(0..16));
        assert_eq!(ranges.last().cloned(), Some(128..129));
        assert!(ranges.windows(2).all(|pair| pair[0].end == pair[1].start));

        let mut covered = Vec::with_capacity(plan.anchor_record_count());
        for range in &ranges {
            for axis_y in range.clone() {
                for axis_x in 0..plan.axis.len() {
                    covered.push(plan.axis[axis_y] * TILE_PX + plan.axis[axis_x]);
                }
            }
        }
        assert_eq!(covered.len(), 16_641);
        covered.sort_unstable();
        covered.dedup();
        assert_eq!(covered.len(), 16_641);
        assert_eq!(covered.first().copied(), Some(0));
        assert_eq!(covered.last().copied(), Some(TILE_PX * TILE_PX - 1));
        assert!(covered.contains(&511));
        assert!(covered.contains(&(511 * TILE_PX)));
        assert_eq!(
            plan.source_dense_index(8, BIN_W - 1, 9 * BIN_W - 1),
            Ok(TILE_PX * TILE_PX - 1),
            "the final wave/block padding must duplicate receiver 511,511"
        );

        // Candidate `line` maps one-dimensional pix through meta[10]. With TW=16,
        // the nine 256-thread blocks cover exactly the packed 16x144 rectangle.
        let tile_width = BIN_W;
        let tiles_per_row = TILE_PX / tile_width;
        for block in 0..MULTIFIDELITY_STOCK_CARTESIAN_LONGITUDE_BLOCKS {
            for lane in 0..BIN_W * BIN_W {
                let pix = block * BIN_W * BIN_W + lane;
                let tile = pix / (tile_width * tile_width);
                let in_tile = pix % (tile_width * tile_width);
                let synthetic_y = (tile / tiles_per_row) * tile_width + in_tile / tile_width;
                let synthetic_x = (tile % tiles_per_row) * tile_width + in_tile % tile_width;
                assert_eq!(synthetic_y, lane / BIN_W);
                assert_eq!(synthetic_x, block * BIN_W + lane % BIN_W);
            }
        }
    }

    #[test]
    fn stock_cartesian_receiver_pack_is_bit_exact_and_pads_with_511() {
        let plan = MultifidelityStockCartesianPlan::stride4().expect("stride4 Cartesian plan");
        let mut rxll = vec![0.0f64; TILE_PX * 2];
        for coordinate in 0..TILE_PX {
            rxll[coordinate] = 40.0 + coordinate as f64 / 4096.0;
            rxll[TILE_PX + coordinate] = -9.0 + coordinate as f64 / 8192.0;
        }
        let mut rxar = vec![0.0f32; TILE_PX * TILE_PX * 2];
        for dense_index in 0..TILE_PX * TILE_PX {
            rxar[dense_index * 2] = dense_index as f32 + 0.25;
            rxar[dense_index * 2 + 1] = (dense_index % 251) as f32 + 0.5;
        }

        let mut inspected = 0;
        for group in 0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS {
            let (packed_rxll, packed_rxar) =
                pack_multifidelity_stock_cartesian_group(&plan, &rxll, &rxar, group)
                    .expect("stock Cartesian receiver pack");
            assert_eq!(packed_rxll.len(), TILE_PX * 2);
            assert_eq!(packed_rxar.len(), MULTIFIDELITY_STOCK_CARTESIAN_RXAR_VALUES);
            for synthetic_y in 0..BIN_W {
                for synthetic_x in 0..MULTIFIDELITY_STOCK_CARTESIAN_LAUNCHED_COLUMNS {
                    let dense_index = plan
                        .source_dense_index(group, synthetic_y, synthetic_x)
                        .expect("mapped dense index");
                    let packed_index = synthetic_y * TILE_PX + synthetic_x;
                    assert_eq!(
                        packed_rxll[synthetic_y].to_bits(),
                        rxll[dense_index / TILE_PX].to_bits()
                    );
                    assert_eq!(
                        packed_rxll[TILE_PX + synthetic_x].to_bits(),
                        rxll[TILE_PX + dense_index % TILE_PX].to_bits()
                    );
                    assert_eq!(
                        packed_rxar[packed_index * 2].to_bits(),
                        rxar[dense_index * 2].to_bits()
                    );
                    assert_eq!(
                        packed_rxar[packed_index * 2 + 1].to_bits(),
                        rxar[dense_index * 2 + 1].to_bits()
                    );
                    inspected += 1;
                }
            }
        }
        assert_eq!(inspected, MULTIFIDELITY_STOCK_CARTESIAN_COMPUTED_RECEIVERS);
        assert!(
            pack_multifidelity_stock_cartesian_group(&plan, &rxll[..TILE_PX], &rxar, 0).is_err()
        );
        rxar[0] = f32::NAN;
        assert!(pack_multifidelity_stock_cartesian_group(&plan, &rxll, &rxar, 0).is_err());
    }

    #[test]
    fn stock_cartesian_extract_requires_unique_complete_group_outputs() {
        let plan = MultifidelityStockCartesianPlan::stride4().expect("stride4 Cartesian plan");
        let mut compact = multifidelity_stock_cartesian_compact_output_sentinel(&plan);
        let mut fault_total = 0.0;
        for group in 0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS {
            fault_total += extract_multifidelity_stock_cartesian_group(
                &plan,
                group,
                &complete_stock_cartesian_dense_group(&plan, group),
                0.0,
                &mut compact,
            )
            .expect("extract complete group");
        }
        assert_eq!(fault_total, 0.0);
        validate_multifidelity_stock_cartesian_output(&plan, &compact)
            .expect("complete Cartesian output");
        let decoded = decode_multifidelity_compact_output(&compact, plan.anchor_record_count())
            .expect("decode complete Cartesian output");
        assert_eq!(decoded.len(), 16_641);
        assert_eq!(decoded.first().map(|record| record.0), Some(0));
        assert_eq!(
            decoded.last().map(|record| record.0),
            Some(TILE_PX * TILE_PX - 1)
        );

        let mut missing_group = multifidelity_stock_cartesian_compact_output_sentinel(&plan);
        for group in 0..MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS - 1 {
            extract_multifidelity_stock_cartesian_group(
                &plan,
                group,
                &complete_stock_cartesian_dense_group(&plan, group),
                0.0,
                &mut missing_group,
            )
            .expect("extract present group");
        }
        assert!(validate_multifidelity_stock_cartesian_output(&plan, &missing_group).is_err());

        let first_group = complete_stock_cartesian_dense_group(&plan, 0);
        let mut duplicate_group = multifidelity_stock_cartesian_compact_output_sentinel(&plan);
        extract_multifidelity_stock_cartesian_group(
            &plan,
            0,
            &first_group,
            0.0,
            &mut duplicate_group,
        )
        .expect("extract first group once");
        assert!(extract_multifidelity_stock_cartesian_group(
            &plan,
            0,
            &first_group,
            0.0,
            &mut duplicate_group,
        )
        .is_err());

        let mut unwritten_lane = complete_stock_cartesian_dense_group(&plan, 0);
        unwritten_lane[0] = f32::NAN;
        assert!(extract_multifidelity_stock_cartesian_group(
            &plan,
            0,
            &unwritten_lane,
            0.0,
            &mut multifidelity_stock_cartesian_compact_output_sentinel(&plan),
        )
        .is_err());

        let mut invalid_longitude_padding = complete_stock_cartesian_dense_group(&plan, 0);
        let longitude_padding = plan.axis.len();
        invalid_longitude_padding[longitude_padding * NUM_PERIODS] = f32::NAN;
        assert!(extract_multifidelity_stock_cartesian_group(
            &plan,
            0,
            &invalid_longitude_padding,
            0.0,
            &mut multifidelity_stock_cartesian_compact_output_sentinel(&plan),
        )
        .is_err());

        let mut invalid_final_group_latitude_padding = complete_stock_cartesian_dense_group(
            &plan,
            MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS - 1,
        );
        let final_group_padding_row = BIN_W - 1;
        let final_group_padding_source = final_group_padding_row * TILE_PX * NUM_PERIODS;
        invalid_final_group_latitude_padding[final_group_padding_source] = f32::NAN;
        assert!(extract_multifidelity_stock_cartesian_group(
            &plan,
            MULTIFIDELITY_STOCK_CARTESIAN_LATITUDE_GROUPS - 1,
            &invalid_final_group_latitude_padding,
            0.0,
            &mut multifidelity_stock_cartesian_compact_output_sentinel(&plan),
        )
        .is_err());

        assert!(extract_multifidelity_stock_cartesian_group(
            &plan,
            0,
            &first_group,
            f32::NAN,
            &mut multifidelity_stock_cartesian_compact_output_sentinel(&plan),
        )
        .is_err());

        assert!(require_zero_multifidelity_stock_cartesian_fault(0.0).is_ok());
        let fault_error = require_zero_multifidelity_stock_cartesian_fault(1.0)
            .expect_err("nonzero Cartesian aggregate must fail closed");
        assert!(fault_error.contains("duplicate padding lanes"));
    }

    #[test]
    fn w2_dense_arc_fault_policy_fails_closed_without_changing_stock_or_w1() {
        assert!(require_zero_w2_dense_arc_fault(true, 0.0).is_ok());
        for fault in [1.0, f32::INFINITY, f32::NAN] {
            let error = require_zero_w2_dense_arc_fault(true, fault)
                .expect_err("W2 must reject every nonzero or invalid dense ARC fault delta");
            assert!(error.contains("refusing to reconstruct an under-screened tile"));
            assert!(require_zero_w2_dense_arc_fault(false, fault).is_ok());
        }
    }

    #[test]
    fn cartesian_unbinned_exact_meta_sets_stop_and_tile_width_only() {
        let mut source = (0..noise_gpu::SURFACE_META_SLOTS)
            .map(|slot| slot as f64 + 0.25)
            .collect::<Vec<_>>();
        source[MULTIFIDELITY_STOCK_CARTESIAN_META_OUTPUT_SLOTS_SLOT] =
            noise_gpu::OUT_SLOTS_PROD as f64;
        let exact = multifidelity_stock_cartesian_exact_meta(&source).expect("exact metadata");
        for slot in 0..noise_gpu::SURFACE_META_SLOTS {
            match slot {
                MULTIFIDELITY_STOCK_CARTESIAN_META_BYTE_STOP_SLOT => {
                    assert_eq!(exact[slot], 0.0)
                }
                MULTIFIDELITY_STOCK_CARTESIAN_META_TILE_WIDTH_SLOT => {
                    assert_eq!(exact[slot], BIN_W as f64)
                }
                _ => assert_eq!(exact[slot].to_bits(), source[slot].to_bits()),
            }
        }
        assert!(multifidelity_stock_cartesian_exact_meta(&source[..13]).is_err());
        source[MULTIFIDELITY_STOCK_CARTESIAN_META_OUTPUT_SLOTS_SLOT] += 1.0;
        assert!(multifidelity_stock_cartesian_exact_meta(&source).is_err());
    }

    #[test]
    fn candidate_selector_uses_role_exact_binned_fallback_only_for_sparse_roads() {
        for nsrc in [0, 1, 256, ROAD_SPARSE_STOCK_MAX_SOURCES] {
            assert_eq!(
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer: LineLayer::Road,
                    nsrc,
                    requested_stride: None,
                }),
                None
            );
            assert_eq!(
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer: LineLayer::Rail,
                    nsrc,
                    requested_stride: None,
                }),
                // Rail takes the denser lattice regardless of source count, and so
                // does dense road below -- the sparse-road arm above is the only
                // W1 case that still resolves to the exact binned fallback.
                Some(MultifidelityStride::Stride8)
            );
        }
        assert_eq!(
            select_multifidelity_stride(MultifidelitySelectionInputs {
                layer: LineLayer::Road,
                nsrc: ROAD_SPARSE_STOCK_MAX_SOURCES + 1,
                requested_stride: None,
            }),
            Some(MultifidelityStride::Stride8)
        );
        assert_eq!(
            select_multifidelity_stride(MultifidelitySelectionInputs {
                layer: LineLayer::Road,
                nsrc: usize::MAX,
                requested_stride: None,
            }),
            Some(MultifidelityStride::Stride8)
        );
    }

    #[test]
    fn requested_z13_stride_replaces_only_the_dense_candidate_arm() {
        for requested in [
            MultifidelityStride::Stride32,
            MultifidelityStride::Stride16,
            MultifidelityStride::Stride8,
            MultifidelityStride::Stride4,
        ] {
            assert_eq!(
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer: LineLayer::Road,
                    nsrc: ROAD_SPARSE_STOCK_MAX_SOURCES + 1,
                    requested_stride: Some(requested),
                }),
                Some(requested)
            );
            assert_eq!(
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer: LineLayer::Rail,
                    nsrc: 1,
                    requested_stride: Some(requested),
                }),
                Some(requested)
            );
            assert_eq!(
                select_multifidelity_stride(MultifidelitySelectionInputs {
                    layer: LineLayer::Road,
                    nsrc: ROAD_SPARSE_STOCK_MAX_SOURCES,
                    requested_stride: Some(requested),
                }),
                None
            );
        }
    }

    #[test]
    fn every_runtime_stride_decodes_allocates_and_reconstructs() {
        let mut cheap_gpu = vec![0.0f32; noise_gpu::OUT_SLOTS_MULTIFIDELITY];
        for index in 0..TILE_PX * TILE_PX {
            cheap_gpu[index * NUM_PERIODS] = 10.0;
        }
        for (pixels, expected_records) in [(4, 16_641), (8, 4_225), (16, 1_089), (32, 289)] {
            let stride = MultifidelityStride::from_pixels(pixels).expect("supported stride");
            let axis = multifidelity_anchor_axis(stride);
            let exact =
                compact_anchor_output(&axis, |_, _, period| if period == 0 { 40.0 } else { 0.0 });
            assert_eq!(exact.len(), stride.compact_output_len());
            let decoded = decode_multifidelity_compact_output(&exact, expected_records)
                .expect("runtime compact output decodes");
            assert_eq!(decoded.len(), expected_records);
            let data = multifidelity_interpolation(stride, &cheap_gpu, &exact);
            assert_eq!(data.stride, stride);
            assert_eq!(data.axis.len() * data.axis.len(), expected_records);
            let cells = reconstruct_multifidelity_cells(&data, &[]);
            assert_eq!(cells[0], 26);
            assert_eq!(cells[TILE_PX * TILE_PX - 1], 26);
        }
    }

    #[test]
    fn selector_covers_the_final_stride_axis_window() {
        let mut cheap_gpu = vec![0.0f32; noise_gpu::OUT_SLOTS_MULTIFIDELITY];
        for index in 0..TILE_PX * TILE_PX {
            cheap_gpu[index * NUM_PERIODS] = 10.0;
        }
        let stride = MultifidelityStride::Stride16;
        let axis = multifidelity_anchor_axis(stride);
        let exact = compact_anchor_output(&axis, |py, px, period| {
            if period == 0 && py == 496 && px == 496 {
                1_000_000.0
            } else if period == 0 {
                10.0
            } else {
                0.0
            }
        });
        let data = multifidelity_interpolation(stride, &cheap_gpu, &exact);
        let mask = multifidelity_receiver_mask_with_replay(LineLayer::Rail, &data, true);

        // The last real axis window is [496, 511); its interior must be
        // selectable while both boundary anchors remain authoritative.
        assert_eq!(mask[497 * TILE_PX + 497], 1.0);
        assert_eq!(mask[496 * TILE_PX + 496], 0.0);
        assert_eq!(mask[511 * TILE_PX + 511], 0.0);

        // The production entry point must reach the same block through the layer
        // authority, not only the test seam.
        assert_eq!(multifidelity_receiver_mask(LineLayer::Rail, &data), mask);

        let anchors_only = multifidelity_receiver_mask_with_replay(LineLayer::Rail, &data, false);
        assert!(anchors_only.iter().all(|&value| value == 0.0));

        // Road never reaches launch C, so its mask stays empty however steep the tile.
        assert!(!multifidelity_layer_replays(LineLayer::Road));
        let road = multifidelity_receiver_mask(LineLayer::Road, &data);
        assert!(road.iter().all(|&value| value == 0.0));
    }

    #[test]
    fn compact_output_rejects_short_fractional_and_out_of_bounds_records() {
        let valid = vec![3.0, 1.0, 2.0, 3.0, 0.0];
        let decoded = decode_multifidelity_compact_output(&valid, 1).expect("valid compact output");
        assert_eq!(decoded, vec![(3, [1.0, 2.0, 3.0], 0.0)]);
        assert!(decode_multifidelity_compact_output(&valid[..4], 1).is_err());

        let mut fractional = valid.clone();
        fractional[0] = 3.5;
        assert!(decode_multifidelity_compact_output(&fractional, 1).is_err());

        let mut out_of_bounds = valid;
        out_of_bounds[0] = (TILE_PX * TILE_PX) as f32;
        assert!(decode_multifidelity_compact_output(&out_of_bounds, 1).is_err());

        let duplicate = vec![3.0, 1.0, 2.0, 3.0, 0.0, 3.0, 4.0, 5.0, 6.0, 0.0];
        assert!(decode_multifidelity_compact_output(&duplicate, 2).is_err());
    }

    #[test]
    fn compact_receiver_pack_rejects_out_of_bounds_dense_index() {
        let rxll = vec![0.0f64; TILE_PX * 2];
        let rxar = vec![0.0f32; TILE_PX * TILE_PX * 2];
        let last = TILE_PX * TILE_PX - 1;
        let packed = pack_multifidelity_compact_receivers(&rxll, &rxar, &[0, last])
            .expect("valid compact receiver pack");
        assert_eq!(
            packed.len(),
            2 * noise_gpu::MULTIFIDELITY_COMPACT_RECEIVER_RECORD_WORDS
        );
        assert_eq!(packed[3], 0);
        assert_eq!(packed[7], last as u64);
        assert!(pack_multifidelity_compact_receivers(&rxll, &rxar, &[TILE_PX * TILE_PX]).is_err());
        let mut nonfinite = rxar;
        nonfinite[0] = f32::NAN;
        assert!(pack_multifidelity_compact_receivers(&rxll, &nonfinite, &[0]).is_err());
    }

    #[test]
    fn compact_bucket_plan_preserves_set_and_reconstruction() {
        let multifidelity_stride = MultifidelityStride::Stride16;
        let axis = multifidelity_anchor_axis(multifidelity_stride);
        let row_major: Vec<usize> = axis
            .iter()
            .flat_map(|&py| axis.iter().map(move |&px| py * TILE_PX + px))
            .collect();
        let check_plan = |label: &str,
                          dense_indices: &[usize],
                          expected_launches: usize,
                          expected_global_blocks: usize| {
            let plan = multifidelity_compact_plan(dense_indices).expect(label);
            assert_eq!(
                plan.indices.len(),
                dense_indices.len(),
                "{label}: record count"
            );
            assert_eq!(
                plan.launches.len(),
                expected_launches,
                "{label}: block count"
            );
            assert_eq!(
                dense_indices
                    .len()
                    .div_ceil(MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK),
                expected_global_blocks,
                "{label}: global one-grid baseline",
            );
            let mut expected = dense_indices.to_vec();
            expected.sort_unstable();
            let mut actual = plan.indices.clone();
            actual.sort_unstable();
            assert_eq!(actual, expected, "{label}: sorted index set changed");
            actual.dedup();
            assert_eq!(actual.len(), plan.indices.len(), "{label}: duplicate index");

            let controls = multifidelity_compact_plan_controls(&plan);
            assert_eq!(
                controls.len(),
                noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                    + plan.launches.len() * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS,
                "{label}: control length",
            );
            assert_eq!(controls[0], dense_indices.len() as u64);
            assert_eq!(
                controls[1],
                noise_gpu::MULTIFIDELITY_COMPACT_ABI_VERSION as u64
            );
            assert_eq!(
                controls[2],
                noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE as u64
            );
            let mut covered = Vec::with_capacity(plan.indices.len());
            for (block_index, launch) in plan.launches.iter().enumerate() {
                assert!(launch.record_count > 0);
                assert!(launch.record_count <= MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK);
                assert_eq!(
                    launch.record_offset,
                    covered.len(),
                    "{label}: gap at block {block_index}"
                );
                let control = noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                    + block_index * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS;
                assert_eq!(controls[control], launch.record_offset as u64);
                assert_eq!(controls[control + 1], launch.record_count as u64);
                let end = launch.record_offset + launch.record_count;
                let chunk = &plan.indices[launch.record_offset..end];
                let bucket = multifidelity_compact_bucket(chunk[0]);
                assert!(chunk
                    .iter()
                    .all(|&index| multifidelity_compact_bucket(index) == bucket));
                let min_x = chunk.iter().map(|&index| index % TILE_PX).min().unwrap();
                let max_x = chunk.iter().map(|&index| index % TILE_PX).max().unwrap();
                let min_y = chunk.iter().map(|&index| index / TILE_PX).min().unwrap();
                let max_y = chunk.iter().map(|&index| index / TILE_PX).max().unwrap();
                assert!(
                    max_x - min_x < MULTIFIDELITY_COMPACT_BUCKET_PX,
                    "{label}: x span at block {block_index}"
                );
                assert!(
                    max_y - min_y < MULTIFIDELITY_COMPACT_BUCKET_PX,
                    "{label}: y span at block {block_index}"
                );
                covered.extend_from_slice(chunk);
            }
            assert_eq!(
                covered, plan.indices,
                "{label}: block ranges are not contiguous"
            );
            eprintln!(
                "COMPACT_PLAN_CENSUS label={label} active={} global_blocks={} bucket_blocks={} overhead={}",
                dense_indices.len(),
                expected_global_blocks,
                expected_launches,
                expected_launches - expected_global_blocks,
            );
            plan
        };
        // Sixteen 32-pixel buckets per axis; the final bucket carries the
        // explicit 511 boundary alongside the 16-pixel lattice, so the plan
        // emits 256 bucket blocks.
        let anchor_plan = check_plan("anchors", &row_major, 256, 5);
        let selector_indices: Vec<usize> = (0..TILE_PX * TILE_PX)
            .filter(|index| index % 12 == 0)
            .collect();
        let _selector_plan = check_plan("selector", &selector_indices, 256, 86);
        let all_indices: Vec<usize> = (0..TILE_PX * TILE_PX).collect();
        let _all_plan = check_plan("all", &all_indices, 1024, 1024);
        let boundary = TILE_PX * TILE_PX - 1;
        let boundary_block = anchor_plan
            .launches
            .iter()
            .find(|launch| {
                let end = launch.record_offset + launch.record_count;
                anchor_plan.indices[launch.record_offset..end].contains(&boundary)
            })
            .expect("anchor boundary block");
        assert_eq!(multifidelity_compact_bucket(boundary), (15, 15));
        assert!(anchor_plan.indices[boundary_block.record_offset
            ..boundary_block.record_offset + boundary_block.record_count]
            .contains(&boundary));
        assert!(multifidelity_compact_plan(&[0, 0]).is_err());
        assert!(multifidelity_compact_plan(&[TILE_PX * TILE_PX]).is_err());

        let mut cheap_gpu = vec![0.0f32; noise_gpu::OUT_SLOTS_MULTIFIDELITY];
        for index in 0..TILE_PX * TILE_PX {
            cheap_gpu[index * 3] = 10.0;
        }
        let stride = noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE;
        let mut row_output = vec![0.0f32; row_major.len() * stride];
        let mut row_record_for_dense = vec![usize::MAX; TILE_PX * TILE_PX];
        for (record, &dense_index) in row_major.iter().enumerate() {
            row_record_for_dense[dense_index] = record;
            let output = record * stride;
            row_output[output + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_INDEX_SLOT] =
                dense_index as f32;
            row_output[output + noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_ENERGY_BASE] =
                if record % 7 == 0 { 100.0 } else { 0.0 };
        }
        let mut bucket_output = Vec::with_capacity(row_output.len());
        for &dense_index in &anchor_plan.indices {
            let record = row_record_for_dense[dense_index];
            bucket_output.extend_from_slice(&row_output[record * stride..(record + 1) * stride]);
        }
        let interpolation =
            multifidelity_interpolation(multifidelity_stride, &cheap_gpu, &row_output);
        assert_eq!(
            reconstruct_multifidelity_cells(&interpolation, &row_output),
            reconstruct_multifidelity_cells(&interpolation, &bucket_output),
            "explicit output indices must make compact block order irrelevant"
        );
    }

    #[test]
    fn packed_compact_plan_preserves_each_warp_bucket_and_record_bounds() {
        let stride = MultifidelityStride::Stride32;
        let axis = multifidelity_anchor_axis(stride);
        let dense_indices: Vec<usize> = axis
            .iter()
            .flat_map(|&py| axis.iter().map(move |&px| py * TILE_PX + px))
            .collect();
        let plan = multifidelity_compact_packed_plan(&dense_indices).expect("packed anchor plan");
        assert_eq!(plan.indices.len(), stride.anchor_record_count());
        assert_eq!(plan.indices.len(), 289);
        assert!(plan
            .launches
            .iter()
            .all(|block| block.len() == MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK));

        let controls = multifidelity_compact_packed_plan_controls(&plan);
        assert_eq!(controls[0], dense_indices.len() as u64);
        assert_eq!(
            controls[1],
            noise_gpu::MULTIFIDELITY_COMPACT_ABI_VERSION as u64
        );
        assert_eq!(
            controls[2],
            noise_gpu::MULTIFIDELITY_COMPACT_OUTPUT_STRIDE as u64
        );
        assert_eq!(
            controls.len(),
            noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                + plan.launches.len()
                    * MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK
                    * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS
        );

        let mut covered = Vec::with_capacity(plan.indices.len());
        for (block_index, block) in plan.launches.iter().enumerate() {
            for (warp_index, descriptor) in block.iter().enumerate() {
                let control = noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                    + (block_index * MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK + warp_index)
                        * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS;
                assert_eq!(controls[control], descriptor.record_offset as u64);
                assert_eq!(controls[control + 1], descriptor.record_count as u64);
                if descriptor.record_count == 0 {
                    assert!(covered.len() == plan.indices.len());
                    continue;
                }
                assert!(descriptor.record_count <= MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_WARP);
                assert_eq!(descriptor.record_offset, covered.len());
                let end = descriptor.record_offset + descriptor.record_count;
                let chunk = &plan.indices[descriptor.record_offset..end];
                let bucket = multifidelity_compact_bucket(chunk[0]);
                assert!(chunk
                    .iter()
                    .all(|&index| multifidelity_compact_bucket(index) == bucket));
                let min_x = chunk.iter().map(|&index| index % TILE_PX).min().unwrap();
                let max_x = chunk.iter().map(|&index| index % TILE_PX).max().unwrap();
                let min_y = chunk.iter().map(|&index| index / TILE_PX).min().unwrap();
                let max_y = chunk.iter().map(|&index| index / TILE_PX).max().unwrap();
                assert!(max_x - min_x < MULTIFIDELITY_COMPACT_BUCKET_PX);
                assert!(max_y - min_y < MULTIFIDELITY_COMPACT_BUCKET_PX);
                covered.extend_from_slice(chunk);
            }
        }
        assert_eq!(covered, plan.indices);
    }

    #[test]
    fn packed_stride16_plan_covers_the_production_anchor_lattice() {
        let stride = MultifidelityStride::Stride16;
        let axis = multifidelity_anchor_axis(stride);
        let dense_indices: Vec<usize> = axis
            .iter()
            .flat_map(|&py| axis.iter().map(move |&px| py * TILE_PX + px))
            .collect();
        let plan = multifidelity_compact_packed_plan(&dense_indices)
            .expect("production packed anchor plan");

        assert_eq!(plan.indices.len(), 1_089);
        assert_eq!(plan.indices.len(), stride.anchor_record_count());
        assert_eq!(plan.launches.len(), 32);
        assert_eq!(
            multifidelity_compact_packed_plan_controls(&plan).len(),
            noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                + plan.launches.len()
                    * MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK
                    * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS
        );

        let mut covered = plan.indices.clone();
        covered.sort_unstable();
        let mut expected = dense_indices;
        expected.sort_unstable();
        assert_eq!(covered, expected);
    }

    #[test]
    fn multifidelity_fault_total_stays_monotonic_across_mixed_layer_order() {
        let mut previous = 0.0f32;
        for (dense_total, sampled_total, expected_delta) in
            [(0.0, 3.0, 3.0), (2.0, 3.0, 2.0), (2.0, 4.0, 1.0)]
        {
            let mut output = vec![0.0; noise_gpu::OUT_SLOTS_MULTIFIDELITY];
            output[noise_gpu::OUT_FAULT_SLOT] = dense_total;
            add_multifidelity_fault_total_to_dense_slot(&mut output, sampled_total);
            let current = output[noise_gpu::OUT_FAULT_SLOT];
            assert_eq!(current - previous, expected_delta);
            previous = current;
        }
    }

    #[test]
    fn packed_compact_plan_rejects_bad_indices_and_splits_a_full_bucket() {
        let bucket_indices: Vec<usize> = (0..32)
            .flat_map(|py| (0..32).map(move |px| py * TILE_PX + px))
            .collect();
        let plan = multifidelity_compact_packed_plan(&bucket_indices).expect("full bucket plan");
        assert_eq!(plan.indices.len(), 1024);
        assert_eq!(plan.launches.len(), 4);
        assert!(plan
            .launches
            .iter()
            .flat_map(|block| block.iter())
            .all(|descriptor| descriptor.record_count == 32));
        let controls = multifidelity_compact_packed_plan_controls(&plan);
        assert_eq!(controls[0], 1024);
        assert_eq!(
            controls.len(),
            noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_WORDS
                + 4 * MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK
                    * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS
        );
        assert!(multifidelity_compact_packed_plan(&[0, 0]).is_err());
        assert!(multifidelity_compact_packed_plan(&[TILE_PX * TILE_PX]).is_err());
    }

    #[test]
    fn packed_compact_launch_geometry_keeps_the_existing_cuda_abi() {
        let indices = multifidelity_anchor_axis(MultifidelityStride::Stride32)
            .iter()
            .map(|&coordinate| coordinate * TILE_PX + coordinate)
            .collect::<Vec<_>>();
        let plan = multifidelity_compact_packed_plan(&indices).expect("diagonal plan");
        let (grid_x, block_x, shared_mem_bytes) =
            multifidelity_compact_packed_launch_geometry(&plan).expect("launch geometry");
        assert_eq!(grid_x as usize, plan.launches.len());
        assert_eq!(block_x as usize, BIN_W * BIN_W);
        assert_eq!(
            block_x as usize,
            MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK
                * MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_WARP
        );
        assert_eq!(shared_mem_bytes, 0);
        assert!(multifidelity_compact_packed_launch_geometry(
            &MultifidelityCompactPackedPlan::default()
        )
        .is_err());
    }

    #[test]
    fn energy_reconstruction_handles_log_ratio_zero_fallback_and_silence() {
        let mut cheap_gpu = vec![0.0f32; noise_gpu::OUT_SLOTS_MULTIFIDELITY];
        for index in 0..TILE_PX * TILE_PX {
            cheap_gpu[index * NUM_PERIODS] = 10.0;
        }
        let multifidelity_stride = MultifidelityStride::Stride16;
        let axis = multifidelity_anchor_axis(multifidelity_stride);
        // All-positive corners use log(exact/cheap): 40/10 = 4×, applied to
        // the raw period energy before Lden.  40 linear energy is 13.01 dB
        // Lden, hence HM3 byte 26.
        let exact =
            compact_anchor_output(&axis, |_, _, period| if period == 0 { 40.0 } else { 0.0 });
        let data = multifidelity_interpolation(multifidelity_stride, &cheap_gpu, &exact);
        let cells = reconstruct_multifidelity_cells(&data, &[]);
        assert_eq!(cells[TILE_PX + 1], 26);
        assert_eq!(cells[(TILE_PX - 1) * TILE_PX + (TILE_PX - 1)], 26);

        // A zero exact corner must not enter log(0). At one quarter of the
        // 16-pixel window, the silent (0,0) corner has weight 9/16, so the
        // linear fallback yields 40 × 7/16 = 17.5. The dense cheap field is
        // present at this receiver, so reconstruction is admitted.
        let exact_with_silent_corner = compact_anchor_output(&axis, |py, px, period| {
            if period == 0 && (py != 0 || px != 0) {
                40.0
            } else {
                0.0
            }
        });
        let quarter_stride = multifidelity_stride.pixels() / 4;
        let target = quarter_stride * TILE_PX + quarter_stride;
        let data = multifidelity_interpolation(
            multifidelity_stride,
            &cheap_gpu,
            &exact_with_silent_corner,
        );
        let cells = reconstruct_multifidelity_cells(&data, &[]);
        assert_eq!(cells[target], 19);

        // The same exact-anchor interpolation is suppressed when launch A's
        // dense cheap field says the interior receiver is silent. This is the
        // contour-preserving gate used by the W1 candidate.
        cheap_gpu[target * NUM_PERIODS] = 0.0;
        let data = multifidelity_interpolation(
            multifidelity_stride,
            &cheap_gpu,
            &exact_with_silent_corner,
        );
        let cells = reconstruct_multifidelity_cells(&data, &[]);
        assert_eq!(cells[target], tile_painter::wire_hm3::NO_DATA);

        // Stride4 keeps the same cheap presence authority as every other arm;
        // the ladder must vary only physical anchor spacing.
        let sparse_stride = MultifidelityStride::Stride4;
        let sparse_axis = multifidelity_anchor_axis(sparse_stride);
        let sparse_exact = compact_anchor_output(&sparse_axis, |py, px, period| {
            if period == 0 && (py != 0 || px != 0) {
                40.0
            } else {
                0.0
            }
        });
        let sparse_target = TILE_PX + 1;
        cheap_gpu[sparse_target * NUM_PERIODS] = 0.0;
        let data = multifidelity_interpolation(sparse_stride, &cheap_gpu, &sparse_exact);
        let cells = reconstruct_multifidelity_cells(&data, &[]);
        assert_eq!(cells[sparse_target], tile_painter::wire_hm3::NO_DATA);

        // Exact silence wins over a cheap false positive: no period gets an
        // invented log correction and every receiver remains NO_DATA.
        let silent = compact_anchor_output(&axis, |_, _, _| 0.0);
        let data = multifidelity_interpolation(multifidelity_stride, &cheap_gpu, &silent);
        let cells = reconstruct_multifidelity_cells(&data, &[]);
        assert!(cells
            .iter()
            .all(|&cell| cell == tile_painter::wire_hm3::NO_DATA));
    }
}

fn multifidelity_compact_plan_controls(plan: &MultifidelityCompactPlan) -> Vec<u64> {
    let mut controls = multifidelity_compact_control(plan.indices.len());
    controls.reserve(
        plan.launches
            .len()
            .checked_mul(noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS)
            .expect("compact control length overflow"),
    );
    for launch in &plan.launches {
        controls.extend([launch.record_offset as u64, launch.record_count as u64]);
    }
    controls
}

/// Serialize a packed plan using the unchanged compact control ABI. The eight
/// descriptors per block are always emitted, including zero-count tail entries,
/// because the device kernel has no separate control-length argument.
fn multifidelity_compact_packed_plan_controls(plan: &MultifidelityCompactPackedPlan) -> Vec<u64> {
    let mut controls = multifidelity_compact_control(plan.indices.len());
    controls.reserve(
        plan.launches
            .len()
            .checked_mul(
                MULTIFIDELITY_COMPACT_PACKED_WARPS_PER_BLOCK
                    * noise_gpu::MULTIFIDELITY_COMPACT_CONTROL_BLOCK_WORDS,
            )
            .expect("packed compact control length overflow"),
    );
    for block in &plan.launches {
        for descriptor in block {
            controls.extend([
                descriptor.record_offset as u64,
                descriptor.record_count as u64,
            ]);
        }
    }
    controls
}

fn multifidelity_compact_packed_launch_geometry(
    plan: &MultifidelityCompactPackedPlan,
) -> Result<(u32, u32, u32), String> {
    if plan.launches.is_empty() {
        return Err("packed compact plan must have one block descriptor".to_string());
    }
    let grid_x = u32::try_from(plan.launches.len())
        .map_err(|_| "packed compact launch grid overflow".to_string())?;
    Ok((
        grid_x,
        MULTIFIDELITY_COMPACT_PACKED_RECORDS_PER_BLOCK as u32,
        0,
    ))
}

/// Submit all compact records in one grid. Each CUDA block reads its
/// offset/count descriptor from the flat control plan; the receiver/output
/// allocations retain the explicit dense-index ABI, with no padding record that
/// could be mistaken for an exact receiver by the host decoder.
#[allow(clippy::too_many_arguments)]
fn launch_multifidelity_compact_records(
    function: &CudaFunction,
    plan: &MultifidelityCompactPlan,
    d_elev: &CudaSlice<f32>,
    d_inner: &CudaSlice<f32>,
    d_cover: &CudaSlice<u8>,
    d_meta: &CudaSlice<f64>,
    d_seg: &CudaSlice<f64>,
    d_sp: &CudaSlice<f64>,
    d_semis: &CudaSlice<f32>,
    d_receivers: &CudaSlice<u64>,
    d_controls: &CudaSlice<u64>,
    d_barr: &CudaSlice<f64>,
    d_obstacles: &CudaSlice<u64>,
    d_out: &mut CudaSlice<f32>,
) {
    let block_count = plan.launches.len();
    assert!(
        block_count > 0,
        "compact plan must have one block descriptor"
    );
    debug_assert!(plan.launches.iter().all(|launch| launch.record_count > 0
        && launch.record_count <= MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK));
    let grid_x = u32::try_from(block_count).expect("compact launch grid overflow");
    let launch_cfg = LaunchConfig {
        grid_dim: (grid_x, 1, 1),
        block_dim: (MULTIFIDELITY_COMPACT_RECORDS_PER_BLOCK as u32, 1, 1),
        shared_mem_bytes: 0,
    };
    unsafe {
        function
            .clone()
            .launch(
                launch_cfg,
                noise_gpu::line_kernel_arguments!(
                    d_elev,
                    d_inner,
                    d_cover,
                    d_meta,
                    d_seg,
                    d_sp,
                    d_semis,
                    d_receivers,
                    d_controls,
                    d_barr,
                    d_obstacles,
                    d_out,
                ),
            )
            .expect("multifidelity compact grid launch");
    }
}

/// Submit the bounded warp-packed prototype. Its 256-thread launch is split
/// into eight 32-lane subgroups; every subgroup consumes one descriptor and
/// therefore performs source culling against only its own 32×32 bucket.
#[allow(clippy::too_many_arguments)]
fn launch_multifidelity_compact_records_packed(
    function: &CudaFunction,
    plan: &MultifidelityCompactPackedPlan,
    d_elev: &CudaSlice<f32>,
    d_inner: &CudaSlice<f32>,
    d_cover: &CudaSlice<u8>,
    d_meta: &CudaSlice<f64>,
    d_seg: &CudaSlice<f64>,
    d_sp: &CudaSlice<f64>,
    d_semis: &CudaSlice<f32>,
    d_receivers: &CudaSlice<u64>,
    d_controls: &CudaSlice<u64>,
    d_barr: &CudaSlice<f64>,
    d_obstacles: &CudaSlice<u64>,
    d_out: &mut CudaSlice<f32>,
) {
    let (grid_x, block_x, shared_mem_bytes) = multifidelity_compact_packed_launch_geometry(plan)
        .expect("packed compact plan must have one block descriptor");
    let launch_cfg = LaunchConfig {
        grid_dim: (grid_x, 1, 1),
        block_dim: (block_x, 1, 1),
        shared_mem_bytes,
    };
    unsafe {
        function
            .clone()
            .launch(
                launch_cfg,
                noise_gpu::line_kernel_arguments!(
                    d_elev,
                    d_inner,
                    d_cover,
                    d_meta,
                    d_seg,
                    d_sp,
                    d_semis,
                    d_receivers,
                    d_controls,
                    d_barr,
                    d_obstacles,
                    d_out,
                ),
            )
            .expect("multifidelity packed compact grid launch");
    }
}
