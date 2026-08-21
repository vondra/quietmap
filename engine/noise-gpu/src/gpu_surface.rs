//! GPU surface heatmap batch runner for the LINE layers (road + rail) — the
//! production wiring of the binned scatter kernel. Road and rail are both
//! `LineRow` sources feeding the identical CNOSSOS line-source physics, so one
//! kernel (`line_binned`) serves both; only the loader, halo reach, and HM3
//! source_id differ. Builds one tile block's shared 10 km halo once, then per
//! tile per layer: load rows, bin sources per 8×8 block, run the kernel,
//! collapse to Lden u8, write `{output}/{layer}/13/x/y.bin` and (if a baseline
//! exists) diff it. Reports per-layer throughput.
//!
//!   # one grid-aligned block (dev/bench), diff vs baseline:
//!   NOISE_GPU_BASELINE=/root/baseline gpu-surface --layers rail 4510 2786 4
//!   # a whole region (production), road+rail → HM3:
//!   NOISE_GPU_PREPARED=/dev/shm/qmap/prepared DATA_YEAR=2026 \
//!     gpu-surface --layers road,rail --bbox 38.27,-9.78,39.17,-8.50 --output OUT
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use cudarc::driver::sys::CUevent_flags;
use cudarc::driver::{result, CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use h3o::CellIndex;
use noise_compute::admin;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_gpu::{
    pack_sources, pack_tile, upload_obstacles, ObstDev, SurfaceKernelTileParameters, TileBuffers,
    BIN_W, N_BINS,
};
use raster_reader::fused_tile_z13::{default_batch_size, TileBatch};
use raster_reader::RealRasters;
use rayon::prelude::*;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::engine_spans::EngineCellSpans;
use tile_painter::grid::tile_range;
use tile_painter::region_runner::{
    announce_stream_cell_started, batch_slot, read_r4_file, region_tiles, split_configured_layers,
    split_stream_line, tile_batch_window, tile_centre_r4, tile_halo_window,
};
use tile_painter::renderer_evidence::{
    maybe_run_static_attestation, DependencyProfile, RegionTerminalStatus, RendererEvidence,
    RuntimeParameters, StaticAttestationParameters, RENDERER_EVIDENCE_FLAG,
    RENDERER_STATIC_ATTEST_FLAG,
};
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::ObstacleData;
use tile_painter::wire_hm3::{collapse_lden_surface_u8, read_tile, write_tile};

// One-time GPU/layer setup lives in the sibling `gpu_init` module; the hot
// kernel-launch path (process_block/region, run_stream, main) stays here.
#[path = "gpu_init.rs"]
mod gpu_init;
use gpu_init::{timing_enabled, warm_device, warm_device_on, LineLayer, Progress};

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

/// Process-wide byte budget for host-resident tile blocks (E1, gg z13 v2
/// review): bounds building + ready blocks across ALL stream workers and
/// both halves of each worker's double buffer — a per-worker block-count
/// window is not a memory bound (2 workers × current+next × window ⇒ up to
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
/// waits, and nothing releases until the whole collect returns; gg z13 impl
/// review, Codex CRITICAL). Dropping the permit releases — panic-safe.
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
    /// one chunk always makes progress. Deadlock-freedom rests on TWO rules
    /// (both violated by the first draft of this gate): (1) only BUILDER
    /// threads ever wait here; (2) the GPU loop drops its chunk's permit
    /// BEFORE joining the next builder, so a builder waiting for space can
    /// never be waited ON by the thread that owns the space.
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
    fn delta(&self, before: Option<&Self>) -> Self {
        let before = before.cloned().unwrap_or_default();
        Self {
            t_kernel: self.t_kernel - before.t_kernel,
            kernel_ms: self.kernel_ms - before.kernel_ms,
            kernel_calls: self.kernel_calls - before.kernel_calls,
            t_bins: self.t_bins - before.t_bins,
            t_load: self.t_load - before.t_load,
            t_h2d: self.t_h2d - before.t_h2d,
            t_encode: self.t_encode - before.t_encode,
            t_write: self.t_write - before.t_write,
            t_cleanup: self.t_cleanup - before.t_cleanup,
            max_diff: self.max_diff,
            n_diff: self.n_diff - before.n_diff,
            n_cmp: self.n_cmp - before.n_cmp,
            n_le1: self.n_le1 - before.n_le1,
            n_le3: self.n_le3 - before.n_le3,
            n_baseline: self.n_baseline - before.n_baseline,
            n_written: self.n_written - before.n_written,
            bytes_written: self.bytes_written - before.bytes_written,
            n_cleanup_checked: self.n_cleanup_checked - before.n_cleanup_checked,
            n_cleanup_removed: self.n_cleanup_removed - before.n_cleanup_removed,
            n_tiles: self.n_tiles - before.n_tiles,
        }
    }
}

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
    /// flag). See the spike record (.claude/plans/heatmap-orchestrator-audit/).
    barriers_enabled: bool,
}

/// A layer's GPU-resident source buffers (`seg`, `sp`, `semis`), uploaded once per
/// region (by the caller) and shared across every block/tile of that region.
type LayerSrc = (LineLayer, (CudaSlice<f64>, CudaSlice<f64>, CudaSlice<f32>));

/// Compute every `(tile, layer)` in `block_tiles` on the GPU, using the caller-built
/// shared halo (`batch`, cropped in parallel across the region's blocks), the region's
/// pre-loaded rows, and pre-uploaded sources (`src_dev`) — all built/uploaded once per
/// centre-R4 region by the caller, not re-read or re-uploaded per block or tile.
#[allow(clippy::too_many_arguments)]
fn process_block(
    dev: &Arc<CudaDevice>,
    f: &CudaFunction,
    batch: &TileBatch,
    cfg: &Cfg,
    bx: u32,
    by: u32,
    block_tiles: &[(u32, u32)],
    region_rows: &[(LineLayer, Vec<LineRow>)],
    src_dev: &[LayerSrc],
    barriers: &BarrierData,
    obst_dev: &ObstDev,
    obstacles: Option<&ObstacleSet>,
    interior_pass: &mut tile_painter::source_loader_obstacle::InteriorPass,
    tile_timing: &mut BTreeMap<((u32, u32), u8), (f64, Option<f64>)>,
    stats: &mut BTreeMap<&'static str, LayerStat>,
    prog: &mut Progress,
) -> Result<()> {
    let halo = &batch.tiles[0].halo;
    let halo_geom = halo.geom();
    let (_, _, _, rows, cols) = halo_geom;

    let elev: Vec<f32> = halo.pixels().iter().map(|p| p.elevation).collect();
    // Noise barriers reach the kernel as the VECTOR per-tile `for_tile` slice
    // (exact ray×segment crossings in `barrier_best_candidate`), never as a
    // raster burn —
    // `FusedGrid::burn_building_max` was measured acoustically unsound (the ray
    // cadence steps over a one-cell-thin wall on most paths; mean +3.7 / max
    // +13.8 dB under-screening; decision record: tile-painter
    // tests/barrier_screening.rs).
    let mut cover = Vec::with_capacity(rows * cols * 3);
    for p in halo.pixels() {
        cover.push(p.building);
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
    if require_arcstat && cfg!(feature = "v2-h0") {
        bail!("rail ARCSTAT census is only defined for the stock surface role");
    }
    let out_slots = if require_arcstat {
        noise_gpu::OUT_SLOTS_PROF
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
    // kernel. Same per-(tile,layer) work in the same order ⇒ identical output.
    // Order by LAYER first (all road, then all rail), not interleaved: the pipeline
    // overlaps tile N+1's prep with tile N's kernel, so consecutive same-layer items
    // (similar kernel ≈ similar prep cost) overlap far better than road↔rail swings.
    let pass_a_tiles = tile_halo_window(block_tiles, cfg.z);
    let items: Vec<(u32, u32, LineLayer)> = region_rows
        .iter()
        .flat_map(|(l, _)| pass_a_tiles.iter().map(move |&(tx, ty)| (tx, ty, *l)))
        .collect();
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

    // Pass A stores every collapsed tile/layer, including the 8-neighbour
    // halo. Pass B below applies one geometric donor raster to every layer and
    // writes only the centre tiles owned by this block.
    if let Some(set) = obstacles {
        for &(tx, ty) in &pass_a_tiles {
            if !interior_pass.has_class_raster((tx, ty)) {
                let tile = &batch.tiles[batch_slot(batch, tx, ty)];
                interior_pass.insert_class_raster(
                    (tx, ty),
                    tile_painter::source_loader_obstacle::bake_tile_envelope_classes(tile, set),
                );
            }
        }
    }
    let mut iter = items.into_iter();
    let mut pending = iter
        .find(|(tx, ty, layer)| !interior_pass.has_collapsed((*tx, *ty), layer.source_id()))
        .map(|it| prep_timed(it, stats));
    while let Some(((tx, ty, layer), bufs)) = pending {
        let tk = Instant::now();
        let d_inner = dev.htod_copy(bufs.inner).expect("inner");
        let d_meta = dev.htod_copy(bufs.meta).expect("meta");
        // Region-resident sources (uploaded once per layer above) — not re-uploaded per tile.
        // (nsrc rides in meta[12] — pack_tile; the freed launch slot carries the
        // obstacle pointer table.)
        let (d_seg, d_sp, d_semis) = &src_dev
            .iter()
            .find(|(l, _)| *l == layer)
            .expect("layer src")
            .1;
        let d_rxll = dev.htod_copy(bufs.rxll).expect("rxll");
        let d_rxar = dev.htod_copy(bufs.rxar).expect("rxar");
        let d_barr = dev.htod_copy(bufs.barr).expect("barr");
        let h2d_done = Instant::now();
        stats.entry(layer.dir()).or_default().t_h2d += h2d_done.duration_since(tk).as_secs_f64();
        // CUDA-event bracket (timing only): record on the SAME stream the kernel
        // launches on (`f.launch` → `dev.cu_stream()`), so `start`/`stop` straddle
        // exactly the kernel — not the htod copies above or the dtoh join below. The
        // `elapsed` read happens after dtoh_sync_copy synchronises the stream, so
        // both events are complete. Off ⇒ no events created at all.
        let kernel_evt = timing_enabled().then(|| {
            let stream = *dev.cu_stream();
            let start = result::event::create(CUevent_flags::CU_EVENT_DEFAULT).expect("evt start");
            let stop = result::event::create(CUevent_flags::CU_EVENT_DEFAULT).expect("evt stop");
            unsafe { result::event::record(start, stream).expect("record start") };
            (start, stop, stream)
        });
        unsafe {
            f.clone()
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
        if let Some((_, stop, stream)) = kernel_evt {
            unsafe { result::event::record(stop, stream).expect("record stop") };
        }
        // Overlap: prep the NEXT item on the CPU while this kernel runs on the GPU.
        pending = iter
            .find(|(next_tx, next_ty, next_layer)| {
                !interior_pass.has_collapsed((*next_tx, *next_ty), next_layer.source_id())
            })
            .map(|it| prep_timed(it, stats));
        // Join: dtoh_sync_copy waits for the kernel, then reads the result back.
        let gpu = dev.dtoh_sync_copy(&d_out).expect("dtoh");
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

        // ARC FAULT: a nonzero delta means THIS tile under-screens somewhere —
        // blocked arcs the merged list had no room for were dropped, so a
        // direction that a building genuinely blocks was painted clear. Loud, not
        // fatal: the tile is still the best this kernel can produce, and a world
        // build should not die on it — but it must never again be invisible.
        let arc_drops_this_tile = gpu[noise_gpu::OUT_FAULT_SLOT] - arc_drops_seen;
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
            if block_tiles.contains(&(tx, ty)) {
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
                RAIL_ARCSTAT_CENSUS_PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            arcstat_seen = current;
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
        let mut accum = TileAccumulator::new();
        accum
            .energy
            .copy_from_slice(&gpu[..noise_gpu::OUT_ENERGY_SLOTS]);
        interior_pass.insert_collapsed(
            (tx, ty),
            layer.source_id(),
            collapse_lden_surface_u8(&accum),
        );
        tile_timing.insert(((tx, ty), layer.source_id()), (tile_wall_s, tile_kernel_ms));
    }

    // Pass B is deliberately outside the CUDA loop. It gives every line
    // layer the same geometric donor offsets and ensures halo tiles never
    // become output ownership conflicts.
    for &(tx, ty) in block_tiles {
        if obstacles.is_some() {
            interior_pass.ensure_donors((tx, ty));
        }
        for layer in region_rows.iter().map(|(layer, _)| *layer) {
            let output_started = Instant::now();
            let mut cells = interior_pass
                .collapsed((tx, ty), layer.source_id())
                .map(|cells| cells.to_vec())
                .expect("Pass A collapsed every requested GPU layer");
            interior_pass.apply((tx, ty), layer.source_id(), &mut cells);
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
                    stats.entry(layer.dir()).or_default().n_written += 1;
                } else if out.exists() {
                    std::fs::remove_file(&out)
                        .with_context(|| format!("rm stale {}", out.display()))?;
                }
                let write_done = Instant::now();
                stats.entry(layer.dir()).or_default().t_write +=
                    write_done.duration_since(encode_done).as_secs_f64();
            }
            if !cfg.baseline.is_empty() {
                let bp = Path::new(&cfg.baseline)
                    .join(layer.dir())
                    .join(cfg.z.to_string())
                    .join(tx.to_string())
                    .join(format!("{ty}.bin"));
                if bp.exists() {
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
                }
            }
            let st = stats.entry(layer.dir()).or_default();
            st.n_tiles += 1;
            if let Some(&(tile_wall_s, tile_kernel_ms)) =
                tile_timing.get(&((tx, ty), layer.source_id()))
            {
                if tile_times_enabled() {
                    let timing = noise_gpu::tile_timing::TileTimingRecord::new(
                        tile_wall_s * 1000.0,
                        tile_kernel_ms,
                    )
                    .expect("Instant and CUDA events must yield finite tile timing");
                    eprintln!(
                        "tile-time {} z{}/{tx}/{ty} {}",
                        layer.dir(),
                        cfg.z,
                        timing.to_json().expect("validated tile timing serializes"),
                    );
                }
            }
            prog.tick();
        }
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
    f: &CudaFunction,
    prepared: &str,
    stats: &mut BTreeMap<&'static str, LayerStat>,
    prog: &mut Progress,
) -> Result<RegionResult> {
    let total = region_tiles.len() * layers.len();
    let written0: usize = stats.values().map(|s| s.n_written).sum();
    // Load every requested layer's rows ONCE for this region (grid_disk(1)).
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
    // Skip a region whose grid_disk(1) ring holds NO line sources (every tile all-silent);
    // preserve the all-silent cleanup so a direct-to-OUTPUT rebuild drops a prior build's tiles.
    if region_rows.iter().all(|(_, rows)| rows.is_empty()) {
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
    }
    let barrier_data = if cfg.barriers_enabled {
        BarrierData::load_for_r4s(&cfg.h3r4, &ring).context("load barriers")?
    } else {
        BarrierData::from_segments(Vec::new())
    };
    // Vector obstacles (geodata-v2 1.6, QM_VECTOR_BUILDINGS=1): the same
    // loader + policy as the CPU builder (all-or-raster, region cell required
    // under partial, shard errors hard). Uploaded ONCE per region; the kernel
    // reads it through the complete 14-slot table owned by
    // `noise_gpu::upload_obstacles` (obst[0]==0 ⇒ raster mode; slot 5 == 0 ⇒
    // E2 pruning disabled). There is deliberately no reserved pointer slot.
    let obstacle_data = ObstacleData::load_for_r4s(&cfg.h3r4, r4, &ring)
        .with_context(|| format!("load obstacles R4 {r4:015x}"))?;
    let obst_dev = upload_obstacles(dev, obstacle_data.set())?;
    // Upload each layer's sources to the GPU ONCE for this region.
    let mut src_dev: Vec<LayerSrc> = Vec::with_capacity(region_rows.len());
    for (layer, rows) in &region_rows {
        let tp = Instant::now();
        let s = pack_sources(rows);
        stats.entry(layer.dir()).or_default().t_bins += tp.elapsed().as_secs_f64();
        let th = Instant::now();
        let uploaded = (
            dev.htod_copy(s.seg).expect("seg"),
            dev.htod_copy(s.sp).expect("sp"),
            dev.htod_copy(s.semis).expect("semis"),
        );
        stats.entry(layer.dir()).or_default().t_h2d += th.elapsed().as_secs_f64();
        src_dev.push((*layer, uploaded));
    }
    // Batch the region's tiles into grid-aligned blocks (one shared halo each).
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
    // Crop block halos in a BOUNDED double-buffered pipeline: build window k+1
    // on a scoped thread (internally a rayon par_iter — each worker keeps its
    // persistent RASTERS thread_local, the twice-decided zero-sync design)
    // while the main thread runs window k's GPU work. The GPU loop consumes
    // the SAME sorted block order, so output is byte-identical to the old
    // build-everything-first path (which materialised EVERY block at once —
    // ~2.8 GiB/region measured at 10 m fields; gg z13 review).
    //
    // RESIDENCY CONTRACT (gg z13 impl review, Codex CRITICAL — two prior
    // drafts deadlocked): host block bytes are bounded PROCESS-WIDE by the
    // byte gate. ONE RAII permit per CHUNK, acquired by the builder for the
    // chunk's summed pre-build estimate BEFORE any block is built (per-block
    // permits inside the rayon collect deadlock once a chunk's aggregate
    // exceeds the budget), corrected to the measured total after the build,
    // and dropped by the GPU loop BEFORE it joins the next builder (a
    // builder blocked on the gate must never be waited ON by the thread
    // holding the bytes it needs). Only builder threads ever wait on the
    // gate ⇒ no circular wait. The gate bounds RESIDENT chunk bytes; the
    // per-block d_inner/H2D staging inside process_block is a transient of
    // at most one block's halo (documented slack, covered by the default
    // budget's headroom). `NOISE_GPU_PIPELINE_BLOCKS` (default 2, ≥1) is
    // only the chunk granularity of the double buffer, not the memory
    // contract.
    let block_keys: Vec<(u32, u32)> = blocks.keys().copied().collect();
    let mut interior_pass = tile_painter::source_loader_obstacle::InteriorPass::new();
    let mut tile_timing: BTreeMap<((u32, u32), u8), (f64, Option<f64>)> = BTreeMap::new();
    let window: usize = std::env::var("NOISE_GPU_PIPELINE_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&w: &usize| w >= 1)
        .unwrap_or(2);
    let raster_ns = std::sync::atomic::AtomicU64::new(0);
    type Chunk = (Vec<((u32, u32), TileBatch)>, ChunkPermit);
    let build_chunk = |keys: &[(u32, u32)]| -> Chunk {
        let t0 = Instant::now();
        let estimate: u64 = keys
            .iter()
            .map(|&(bx, by)| {
                let pass_a_tiles = tile_halo_window(&blocks[&(bx, by)], cfg.z);
                let (batch_base_x, batch_base_y, batch_n) = tile_batch_window(&pass_a_tiles, cfg.z);
                TileBatch::estimate_heap_bytes(
                    cfg.z,
                    batch_base_x,
                    batch_base_y,
                    batch_n,
                    cfg.halo_m,
                )
            })
            .sum();
        let mut permit = pipeline_gate().acquire(estimate);
        let built: Vec<((u32, u32), TileBatch)> = keys
            .par_iter()
            .map(|&(bx, by)| {
                RASTERS.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let rasters = slot.get_or_insert_with(|| RealRasters::new(Path::new(prepared)));
                    let pass_a_tiles = tile_halo_window(&blocks[&(bx, by)], cfg.z);
                    let (batch_base_x, batch_base_y, batch_n) =
                        tile_batch_window(&pass_a_tiles, cfg.z);
                    let mut batch = TileBatch::build_opt_rx_refl(
                        cfg.z,
                        batch_base_x,
                        batch_base_y,
                        batch_n,
                        cfg.halo_m,
                        rasters,
                        obstacle_data.set().is_none(),
                    );
                    // Vector mode: pre-bake vector reflection into rx_refl —
                    // the rxar upload then carries it to the kernel unchanged
                    // (the one shared helper, SPEC §3.8); paint the Pass-A
                    // halo as well as the block's owned tiles.
                    if let Some(set) = obstacle_data.set() {
                        for &(tx, ty) in &pass_a_tiles {
                            let slot = batch_slot(&batch, tx, ty);
                            let tile = &mut batch.tiles[slot];
                            tile_painter::source_loader_obstacle::bake_tile_vector_rx_refl(
                                tile, set,
                            );
                        }
                    }
                    ((bx, by), batch)
                })
            })
            .collect();
        permit.adjust_to(built.iter().map(|(_, batch)| batch.heap_bytes()).sum());
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
        // The builder always gets joined and its chunk collected (RAII permit)
        // even when a block fails — a stream worker survives a failed region,
        // so a leaked reservation would permanently shrink the shared budget.
        let (built_next, gpu_err) = std::thread::scope(|scope| {
            let next_handle = (!next_range.is_empty())
                .then(|| scope.spawn(|| build_chunk(&block_keys[next_range.clone()])));
            let mut err = None;
            for (key, batch) in &current.as_ref().expect("loop guard").0 {
                let (bx, by) = *key;
                if let Err(e) = process_block(
                    dev,
                    f,
                    batch,
                    cfg,
                    bx,
                    by,
                    &blocks[key],
                    &region_rows,
                    &src_dev,
                    &barrier_data,
                    &obst_dev,
                    obstacle_data.set(),
                    &mut interior_pass,
                    &mut tile_timing,
                    stats,
                    prog,
                ) {
                    err = Some(e);
                    break;
                }
            }
            // Drop the consumed chunk (and its permit) BEFORE joining: the
            // builder may be blocked on the gate waiting for exactly these
            // bytes — joining first is the deadlock the review caught.
            drop(current.take());
            let built = next_handle.map(|h| h.join().expect("chunk builder panicked"));
            (built, err)
        });
        if let Some(e) = gpu_err {
            drop(built_next); // RAII releases the unconsumed next chunk
            return Err(e);
        }
        current = built_next;
        start = next_end;
    }
    let raster =
        std::time::Duration::from_nanos(raster_ns.load(std::sync::atomic::Ordering::Relaxed));
    let written: usize = stats.values().map(|s| s.n_written).sum::<usize>() - written0;
    Ok(RegionResult {
        written,
        skipped: total.saturating_sub(written),
        raster,
    })
}

/// STREAM mode (`--stream`): the persistent warm surface worker the cluster orchestrator feeds.
/// CUDA context + scatter PTX + admin table resident, and a PER-THREAD RealRasters instance
/// (thread_local RASTERS — zero cross-thread sync; see the decision record above: the shared
/// store was tried and reverted, cache contention gutted crop throughput) reused across the
/// whole cell stream, so each thread's mmap-LRU stays warm across regions.
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
    use std::collections::VecDeque;
    use std::sync::{Condvar, Mutex};
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
    // FIXED N (default 2, NOT rayon thread count): each worker holds a whole region's source uploads +
    // per-tile GPU scratch on its own stream, so N is bounded by VRAM/RAM, not cores (codex: a
    // halo-only cap is unsafe). 2 fits the 12 GB cards; QM_GPU_STREAM_WORKERS overrides.
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
            region_concurrency_configured: n_workers,
            region_concurrency_effective: n_workers,
            max_regions_per_claim: 1,
            layers: names.iter().map(|name| (*name).to_string()).collect(),
        },
    )?;
    eprintln!(
        "stream: layers={names:?}, halo={halo_m:.0}m, batch={batch_n}, {n_workers} worker(s) — reading R4 cells from stdin"
    );

    // Morton-locality streaming pool (mirrors gpu_airborne run_stream): a reader thread fills a shared
    // queue in arrival (= the orchestrator's Morton) order; each warm worker pops ONE cell per lock
    // acquire, so its serial-crop RealRasters keeps the grid_disk(1) ring-cache warm across the cells it
    // builds while every CUDA stream stays fed (no worker monopolizes a batch).
    // (queue of pending (cell, per-cell stale-layers-request) pairs, stream-closed flag) + a
    // condvar — same shape as gpu_airborne::StreamQueue. The optional `Vec<String>` is the
    // stdin line's `layers=` token (paint-pipeline-v4 PR#1 §3) — `None` = build every
    // configured layer, today's behavior.
    type Work = Arc<(Mutex<(VecDeque<(u64, Option<Vec<String>>)>, bool)>, Condvar)>;
    let work: Work = Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
    let out = Arc::new(Mutex::new(std::io::stdout()));

    let reader_work = Arc::clone(&work);
    // DETACHED (not joined): on a broken-pipe abort the workers exit while this thread may still be
    // blocked in stdin.lines() with stdin open — joining it would deadlock main (gg-gemini CRITICAL).
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

    let cfg = &cfg;
    std::thread::scope(|scope| {
        for worker_slot in 0..n_workers {
            let work = Arc::clone(&work);
            let out = Arc::clone(&out);
            let evidence = evidence.clone();
            scope.spawn(move || {
                // Warm per-worker state: own CUDA stream (overlaps on the GPU) + own stats/prog
                // (worker-local, gg); raster access via the per-rayon-thread RASTERS instances.
                // Safe under UNIQUE centre-R4 ownership (the scheduler leases each cell once per stream):
                // each cell's output tiles are disjoint, so two workers never write the same .bin (gg-codex).
                let (dev, f) = warm_device_on(true);
                let mut stats: BTreeMap<&'static str, LayerStat> = BTreeMap::new();
                let mut prog = Progress {
                    done: 0,
                    total: 0,
                    last_beat: Instant::now(),
                };
                loop {
                    let cell: Option<(u64, Option<Vec<String>>)> = {
                        let (lock, cv) = &*work;
                        let mut g = lock.lock().unwrap();
                        loop {
                            if let Some(cell) = g.0.pop_front() {
                                break Some(cell);
                            }
                            if g.1 {
                                break None; // stream closed + drained → exit
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
                    let mut spans = EngineCellSpans::new(r4, "gpu-surface", worker_slot, t);
                    let tiles = region_tiles(r4, z);
                    spans.metric_u64("owned_tiles", tiles.len() as u64);
                    spans.metric_bool("cuda_event_timing_enabled", timing_enabled());
                    // Narrow this process's configured layers down to the requested (stale)
                    // subset for THIS cell — absent request = build every configured layer,
                    // today's behavior (paint-pipeline-v4 PR#1 §3). The agent only sends
                    // `layers=` for a strict subset of the group, so an EMPTY effective set
                    // means worker-config↔plan drift — fail LOUD (/fail → parked), never a
                    // hollow `done` that would let the hub seal an unbuilt stale layer.
                    let (effective, skipped) =
                        split_configured_layers(layers, req_layers.as_deref(), |l| l.dir());
                    spans.metric_str(
                        "effective_layers",
                        &effective
                            .iter()
                            .map(|l| l.dir())
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    spans.metric_str(
                        "h2d_coverage",
                        "source-soa-and-per-tile; shared-block-halo unmeasured",
                    );
                    spans.metric_str(
                        "cpu_prepare_coverage",
                        "source-pack-and-per-tile-pack; shared-block-vector-pack unmeasured",
                    );
                    let stats_before = stats.clone();
                    let line = if effective.is_empty() {
                        let line = format!(
                            "fail {r4:x} layers-request matches none of configured [{}]",
                            skipped.join(",")
                        );
                        spans.finish_failed(t.elapsed(), &line);
                        evidence
                            .region_terminal(
                                r4,
                                worker_slot,
                                interval_id,
                                RegionTerminalStatus::Fail,
                                0,
                                0,
                                Some(&line),
                            )
                            .expect("emit GPU surface region failure");
                        line
                    } else {
                        let effective_names: Vec<&str> =
                            effective.iter().map(|layer| layer.dir()).collect();
                        let dependencies = evidence.region_dependencies(
                            r4,
                            Path::new(prepared),
                            &cfg.h3r4,
                            &tiles,
                            z,
                            cfg.halo_m,
                            &effective_names,
                            DependencyProfile::Surface,
                        );
                        match dependencies.and_then(|()| {
                            process_region(
                                r4, &tiles, &effective, cfg, &dev, &f, prepared, &mut stats,
                                &mut prog,
                            )
                        }) {
                            Ok(result) => {
                                let mut output_bytes = 0usize;
                                if !result.raster.is_zero() {
                                    spans.push_aggregate_span(
                                        "raster",
                                        result.raster,
                                        None,
                                        None,
                                        Some("surface-halo"),
                                    );
                                }
                                for layer in &effective {
                                    let name = layer.dir();
                                    let delta = stats
                                        .get(name)
                                        .expect("effective layer has stats")
                                        .delta(stats_before.get(name));
                                    spans.push_aggregate_span(
                                        "source_load",
                                        std::time::Duration::from_secs_f64(delta.t_load.max(0.0)),
                                        Some(1),
                                        None,
                                        Some(name),
                                    );
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
                                            std::time::Duration::from_secs_f64(
                                                (delta.kernel_ms / 1_000.0).max(0.0),
                                            ),
                                            Some(delta.kernel_calls as u64),
                                            Some(name),
                                        );
                                    }
                                    // This existing host timer deliberately includes H2D, the
                                    // overlapped prep of the next tile, kernel wait and D2H. It is
                                    // useful evidence, but not mislabeled as isolated GPU time.
                                    if delta.n_tiles > 0 {
                                        spans.push_aggregate_span(
                                            "gpu_pipeline_composite",
                                            std::time::Duration::from_secs_f64(
                                                delta.t_kernel.max(0.0),
                                            ),
                                            Some(delta.n_tiles as u64),
                                            None,
                                            Some(name),
                                        );
                                        spans.push_aggregate_span(
                                            "encode",
                                            std::time::Duration::from_secs_f64(
                                                delta.t_encode.max(0.0),
                                            ),
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
                                    if delta.n_cleanup_checked > 0 {
                                        let cleanup_component =
                                            format!("{name}:stale-output-cleanup");
                                        spans.push_aggregate_span(
                                            "write",
                                            std::time::Duration::from_secs_f64(
                                                delta.t_cleanup.max(0.0),
                                            ),
                                            Some(delta.n_cleanup_checked as u64),
                                            None,
                                            Some(&cleanup_component),
                                        );
                                        spans.metric_u64(
                                            format!("stale_outputs_removed_{name}"),
                                            delta.n_cleanup_removed as u64,
                                        );
                                    }
                                    output_bytes += delta.bytes_written;
                                }
                                let wall = t.elapsed();
                                spans.finish_done(
                                    wall,
                                    result.written,
                                    result.skipped,
                                    Some(output_bytes),
                                );
                                if evidence.is_enabled() {
                                    let output_root = cfg
                                        .output
                                        .as_deref()
                                        .map(Path::new)
                                        .expect("GPU surface evidence requires --output");
                                    for &(x, y) in &tiles {
                                        for &layer in &effective_names {
                                            let output = output_root
                                                .join(layer)
                                                .join(z.to_string())
                                                .join(x.to_string())
                                                .join(format!("{y}.bin"));
                                            evidence
                                                .tile_terminal(
                                                    r4,
                                                    layer,
                                                    z,
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
                                evidence
                                    .region_terminal(
                                        r4,
                                        worker_slot,
                                        interval_id,
                                        RegionTerminalStatus::Done,
                                        result.written,
                                        result.skipped,
                                        None,
                                    )
                                    .expect("emit GPU surface region terminal");
                                format!(
                                    "done {r4:x} {} {} {}",
                                    result.written,
                                    result.skipped,
                                    wall.as_millis()
                                )
                            }
                            Err(e) => {
                                let line = format!("fail {r4:x} {e}");
                                spans.finish_failed(t.elapsed(), &line);
                                evidence
                                    .region_terminal(
                                        r4,
                                        worker_slot,
                                        interval_id,
                                        RegionTerminalStatus::Fail,
                                        0,
                                        0,
                                        Some(&line),
                                    )
                                    .expect("emit GPU surface region failure");
                                line
                            }
                        }
                    };
                    let mut o = out.lock().unwrap();
                    let ok = writeln!(o, "{}", spans.line()).is_ok()
                        && writeln!(o, "{line}").is_ok()
                        && o.flush().is_ok();
                    drop(o);
                    if !ok {
                        // downstream (the box-agent) closed its read end → stop the build, exactly
                        // as the old serial path's `writeln!(…)?` did: signal EOF so every worker drains
                        // and exits, instead of spinning on a dead pipe.
                        let (lock, cv) = &*work;
                        let mut g = lock.lock().unwrap();
                        g.1 = true;
                        g.0.clear(); // drop pending cells so peers exit NOW, not after wasted builds
                        drop(g);
                        cv.notify_all();
                        break;
                    }
                }
            });
        }
    });
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
    // take the world split and break popup parity (Codex delta 1).
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

    let (dev, f) = warm_device();

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
            &f,
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
