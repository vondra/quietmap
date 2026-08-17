//! STREAM mode (`--stream`) for the gpu-airborne bin: the persistent warm worker the cluster
//! orchestrator feeds — parallel CPU prep ahead of a VRAM-gated two-stream GPU pool over R4 cells
//! read from stdin, reusing CUDA contexts + LUTs + rasters across cells (no process churn).

use anyhow::{bail, Context, Result};
use noise_gpu::airborne::{is_cell_unbuildable, AirborneGpu};
use raster_reader::fused_tile_z13::default_batch_size;
use raster_reader::RealRasters;
use tile_painter::engine_spans::EngineCellSpans;
use tile_painter::r4_source_cache::R4SourceCache;
use tile_painter::region_runner::{announce_stream_cell_started, region_tiles};
use tile_painter::renderer_evidence::{
    DependencyProfile, RegionTerminalStatus, RendererEvidence, RuntimeParameters,
};
use tile_painter::worklist::{any_source_arrow, resolve_n_days};

use crate::build::{
    gpu_build_cell_chunked, gpu_build_cell_one_pass, max_candidates_per_chunk, BuildTimings,
};
use crate::prep::{prep_cell, PreparedCell};
use crate::{ring_union, Args, SEL};

/// Shared streaming work queue: (pending Morton-ordered cells, stream-closed flag) under a mutex,
/// plus a condvar to park idle workers. Workers drain a contiguous run off the front; the stdin
/// reader pushes to the back and wakes one. (Factored to a type alias so the `let` below isn't a
/// clippy `type_complexity` lint.)
type StreamQueue = std::sync::Arc<(
    std::sync::Mutex<(std::collections::VecDeque<u64>, bool)>,
    std::sync::Condvar,
)>;

/// Weighted device-memory admission for the two CUDA streams. Ordinary cells take one permit;
/// a cell above its per-stream candidate share (or the bounded megahub path) takes every permit
/// and therefore runs alone. This preserves the old one-cell VRAM safety for large cells while
/// allowing two small kernels to overlap their launch/sync gaps.
struct VramGate {
    state: std::sync::Mutex<VramState>,
    changed: std::sync::Condvar,
}

struct VramState {
    available: usize,
    total: usize,
    exclusive_waiters: usize,
}

impl VramGate {
    fn new(permits: usize) -> Self {
        Self {
            state: std::sync::Mutex::new(VramState {
                available: permits,
                total: permits,
                exclusive_waiters: 0,
            }),
            changed: std::sync::Condvar::new(),
        }
    }

    fn acquire(&self, permits: usize) -> VramLease<'_> {
        let mut state = self.state.lock().unwrap();
        let exclusive = permits == state.total;
        if exclusive {
            state.exclusive_waiters += 1;
            // Wake any ordinary waiter so it observes writer priority before consuming a newly
            // released permit. This also makes the state transition directly observable in tests.
            self.changed.notify_all();
        }
        while state.available < permits || (!exclusive && state.exclusive_waiters > 0) {
            state = self.changed.wait(state).unwrap();
        }
        if exclusive {
            state.exclusive_waiters -= 1;
        }
        state.available -= permits;
        VramLease {
            gate: self,
            permits,
        }
    }
}

struct VramLease<'a> {
    gate: &'a VramGate,
    permits: usize,
}

impl Drop for VramLease<'_> {
    fn drop(&mut self) {
        self.gate.state.lock().unwrap().available += self.permits;
        self.gate.changed.notify_all();
    }
}

#[derive(Default)]
struct PerfWindow {
    cells: u128,
    detailed_cells: u128,
    candidates_ms: u128,
    pack_ms: u128,
    dem_ms: u128,
    queue_ms: u128,
    vram_wait_ms: u128,
    build_ms: u128,
    accumulator_init_ms: u128,
    upload_ms: u128,
    scatter_ms: u128,
    seal_ms: u128,
    candidates: u128,
    blocks: u128,
    tiles: u128,
    chunked: u128,
}

#[derive(Clone, Copy)]
struct DetailedCellPerf {
    prep: crate::prep::PrepTimings,
    build: BuildTimings,
    candidates: usize,
    blocks: usize,
    tiles: usize,
}

impl PerfWindow {
    const CELLS: u128 = 64;

    fn add(
        &mut self,
        queue_ms: u128,
        vram_wait_ms: u128,
        build_ms: u128,
        build: BuildTimings,
        detail: Option<DetailedCellPerf>,
        chunked: bool,
    ) -> Option<String> {
        self.cells += 1;
        self.queue_ms += queue_ms;
        self.vram_wait_ms += vram_wait_ms;
        self.build_ms += build_ms;
        // BuildTimings exists for both the ordinary and chunked paths. Never hide the densest
        // cells from the rolling diagnostic merely because their candidate shape is unmeasured.
        self.accumulator_init_ms += build.accumulator_init.as_millis();
        self.upload_ms += build.upload.as_millis();
        self.scatter_ms += build.scatter.as_millis();
        self.seal_ms += build.seal.as_millis();
        if let Some(detail) = detail {
            self.detailed_cells += 1;
            self.candidates_ms += detail.prep.candidates.as_millis();
            self.pack_ms += detail.prep.pack.as_millis();
            self.dem_ms += detail.prep.dem.as_millis();
            self.candidates += detail.candidates as u128;
            self.blocks += detail.blocks as u128;
            self.tiles += detail.tiles as u128;
        }
        self.chunked += u128::from(chunked);
        if self.cells < Self::CELLS {
            return None;
        }
        let n = Self::CELLS;
        let mut line = format!(
            "{} cells avg: queue={}ms vram-wait={}ms build={}ms; all-build \
             avg[accum={}ms upload={}ms scatter={}ms seal={}ms]",
            Self::CELLS,
            self.queue_ms / n,
            self.vram_wait_ms / n,
            self.build_ms / n,
            self.accumulator_init_ms / n,
            self.upload_ms / n,
            self.scatter_ms / n,
            self.seal_ms / n,
        );
        if self.detailed_cells > 0 {
            let measured = self.detailed_cells;
            line.push_str(&format!(
                "; one-pass={measured} avg[candidates={}ms pack={}ms dem={}ms \
                 shape={}cand/{}blocks/{}tiles]",
                self.candidates_ms / measured,
                self.pack_ms / measured,
                self.dem_ms / measured,
                self.candidates / measured,
                self.blocks / measured,
                self.tiles / measured,
            ));
        } else {
            line.push_str("; one-pass=0 detailed=unmeasured");
        }
        if self.chunked > 0 {
            line.push_str(&format!(
                "; shape=chunked/unmeasured cells={}",
                self.chunked
            ));
        }
        *self = Self::default();
        Some(line)
    }
}

fn cell_perf_suffix(
    queue_ms: u128,
    vram_wait_ms: u128,
    build_ms: u128,
    build: BuildTimings,
    detail: Option<DetailedCellPerf>,
) -> String {
    let Some(detail) = detail else {
        return format!(
            "prep_ms=unmeasured queue_ms={queue_ms} \
             vram_wait_ms={vram_wait_ms} build_ms={build_ms} \
             accum_ms={} upload_ms={} scatter_ms={} seal_ms={} shape=chunked/unmeasured",
            build.accumulator_init.as_millis(),
            build.upload.as_millis(),
            build.scatter.as_millis(),
            build.seal.as_millis(),
        );
    };
    let prep_ms = detail.prep.total().as_millis();
    format!(
        "prep_ms={prep_ms} candidates_ms={} pack_ms={} dem_ms={} \
         queue_ms={queue_ms} vram_wait_ms={vram_wait_ms} build_ms={build_ms} accum_ms={} \
         upload_ms={} scatter_ms={} seal_ms={} shape=one-pass:{}cand/{}blocks/{}tiles",
        detail.prep.candidates.as_millis(),
        detail.prep.pack.as_millis(),
        detail.prep.dem.as_millis(),
        detail.build.accumulator_init.as_millis(),
        detail.build.upload.as_millis(),
        detail.build.scatter.as_millis(),
        detail.build.seal.as_millis(),
        detail.candidates,
        detail.blocks,
        detail.tiles,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellFailureDisposition {
    ReportCellFailure,
    FatalProcess,
}

fn cell_failure_disposition(
    preparation_failed: bool,
    error: &anyhow::Error,
) -> CellFailureDisposition {
    if preparation_failed || is_cell_unbuildable(error) {
        CellFailureDisposition::ReportCellFailure
    } else {
        CellFailureDisposition::FatalProcess
    }
}

fn stream_cell_failure_line(r4: u64, error: &anyhow::Error) -> String {
    format!("fail {r4:x} {error}")
}

type PreparedReceiver = std::sync::Arc<
    std::sync::Mutex<
        std::sync::mpsc::Receiver<(
            u64,
            std::time::Instant,
            std::time::Instant,
            Result<PreparedCell>,
        )>,
    >,
>;

/// One warm CUDA stream. The shared receiver hands each prepared cell to exactly one worker; the
/// weighted gate keeps large/fallback builds exclusive and lets only VRAM-small cells overlap.
#[allow(clippy::too_many_arguments)]
fn run_gpu_worker(
    worker_id: usize,
    n_workers: usize,
    receiver: &PreparedReceiver,
    vram_gate: &VramGate,
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
    args: &Args,
    n_days: u16,
    z: u8,
    bn: u32,
    evidence: RendererEvidence,
) {
    use std::io::Write;
    use std::time::Instant;

    let gpu = AirborneGpu::new(class_weights);
    let concurrent_candidate_limit = max_candidates_per_chunk(gpu.vram_total_bytes()) / n_workers;
    eprintln!(
        "stream: gpu={worker_id} ready; concurrent-candidate-limit={concurrent_candidate_limit}"
    );
    // Own cache + rasters for the M2 chunked fallback. Ordinary cells were fully prepared by the
    // coordinator and never touch these; only a host/VRAM-large cell re-loads its seven-cell ring.
    let mut gpu_cache = R4SourceCache::new(&args.h3r4_dir, args.r4_cache.max(7), SEL);
    let gpu_rasters = RealRasters::new(&args.prepared_dir);
    let mut perf = PerfWindow::default();

    loop {
        // Hold the receiver mutex only for `recv`: once a worker owns a message it releases the
        // lock and computes, so the peer can receive the next prepared cell concurrently.
        let message = receiver.lock().unwrap().recv();
        let Ok((r4, cell_started, prep_finished, prepared)) = message else {
            break;
        };
        let interval_id = evidence
            .region_claim(r4, worker_id)
            .expect("emit GPU airborne region claim");
        let dequeued_at = Instant::now();
        let queue_duration = dequeued_at.duration_since(prep_finished);
        let prep_meta = prepared.as_ref().ok().map(|p| {
            (
                p.t_start,
                p.timings,
                p.nreg,
                p.blocks.len(),
                p.blocks
                    .iter()
                    .map(|block| block.btiles.len())
                    .sum::<usize>(),
            )
        });
        let initial_permits = prepared
            .as_ref()
            .ok()
            .map(|p| {
                if p.too_big || p.nreg > concurrent_candidate_limit {
                    n_workers
                } else {
                    1
                }
            })
            .unwrap_or(1);

        let wait_start = Instant::now();
        let mut lease = Some(vram_gate.acquire(initial_permits));
        let mut vram_wait_ms = wait_start.elapsed().as_millis();
        let mut build_ms = 0u128;
        let mut used_chunked = prepared.as_ref().is_ok_and(|p| p.too_big);
        let mut chunked_reason = used_chunked.then_some("host-budget");
        let mut abandoned_one_pass_wall = std::time::Duration::ZERO;
        let tiles = region_tiles(r4, z);
        let dependencies = evidence.region_dependencies(
            r4,
            &args.prepared_dir,
            &args.h3r4_dir,
            &tiles,
            z,
            0.0,
            &["aircraft-airborne"],
            DependencyProfile::Aircraft,
        );
        let preparation_failed = prepared.is_err() || dependencies.is_err();
        let built = match dependencies {
            Err(error) => Err(error),
            Ok(()) => match prepared {
                Err(e) => Err(e),
                Ok(p) if p.too_big => {
                    let started = Instant::now();
                    let result = gpu_build_cell_chunked(
                        &gpu,
                        &mut gpu_cache,
                        &gpu_rasters,
                        args,
                        n_days,
                        z,
                        bn,
                        r4,
                        &tiles,
                    );
                    build_ms += started.elapsed().as_millis();
                    result
                }
                Ok(p) => {
                    let started = Instant::now();
                    let first = gpu_build_cell_one_pass(&gpu, args, n_days, p);
                    let first_wall = started.elapsed();
                    build_ms += first_wall.as_millis();
                    match first {
                        Err(e) if is_cell_unbuildable(&e) => {
                            // The estimate was deliberately conservative, but fragmentation or an
                            // unusually large classify list can still reject a shared one-pass build.
                            // Rebuild it chunked only after upgrading to exclusive VRAM ownership.
                            used_chunked = true;
                            chunked_reason = Some("vram-fallback");
                            abandoned_one_pass_wall = first_wall;
                            if initial_permits < n_workers {
                                drop(lease.take());
                                let upgrade_start = Instant::now();
                                lease = Some(vram_gate.acquire(n_workers));
                                vram_wait_ms += upgrade_start.elapsed().as_millis();
                            }
                            let started = Instant::now();
                            let result = gpu_build_cell_chunked(
                                &gpu,
                                &mut gpu_cache,
                                &gpu_rasters,
                                args,
                                n_days,
                                z,
                                bn,
                                r4,
                                &tiles,
                            );
                            build_ms += started.elapsed().as_millis();
                            result
                        }
                        other => other,
                    }
                }
            },
        };
        drop(lease);

        let mut spans = EngineCellSpans::new(r4, "gpu-airborne", worker_id, cell_started);
        spans.push_host_span(
            "queue",
            prep_finished,
            dequeued_at,
            Some(1),
            None,
            Some("prepared-channel"),
        );
        spans.metric_u64("owned_tiles", tiles.len() as u64);
        spans.metric_bool("chunked", used_chunked);
        spans.metric_str("chunked_reason", chunked_reason.unwrap_or("not-chunked"));
        // Production does not enable CUDA events yet. The named host composite below is the
        // truthful boundary; null gpu_kernel/d2h fields must never imply a missing event.
        spans.metric_bool("cuda_event_timing_enabled", false);
        let line = match built {
            Ok(built) => {
                let (t_start, timings, candidates, blocks, prepared_tiles) =
                    prep_meta.expect("successful prep metadata");
                let pipeline_wall = t_start.elapsed();
                let protocol_wall = cell_started.elapsed();
                let total_ms = pipeline_wall.as_millis();
                let queue_ms = queue_duration.as_millis();
                let detail = (!used_chunked).then_some(DetailedCellPerf {
                    prep: timings,
                    build: built.timings,
                    candidates,
                    blocks,
                    tiles: prepared_tiles,
                });
                if let Some(summary) = perf.add(
                    queue_ms,
                    vram_wait_ms,
                    build_ms,
                    built.timings,
                    detail,
                    used_chunked,
                ) {
                    eprintln!("[perf gpu={worker_id}] {summary}");
                }
                if let Some(detail) = detail {
                    spans.push_aggregate_span(
                        "cpu_prepare",
                        detail.prep.candidates,
                        Some(1),
                        None,
                        Some("source-load-and-candidate-prepare-composite"),
                    );
                    spans.push_aggregate_span(
                        "pack",
                        detail.prep.pack,
                        Some(1),
                        None,
                        Some("candidate-soa"),
                    );
                    spans.push_aggregate_span(
                        "raster",
                        detail.prep.dem,
                        Some(detail.blocks as u64),
                        None,
                        Some("dem"),
                    );
                    spans.metric_u64("candidates", detail.candidates as u64);
                    spans.metric_u64("blocks", detail.blocks as u64);
                    spans.metric_u64("prepared_tiles", detail.tiles as u64);
                } else {
                    // A chunked cell first passed through the ordinary prep path. A host-budget
                    // route performs only a source/admission probe; a VRAM fallback completed the
                    // full one-pass CPU prep before its device attempt failed. Preserve both as
                    // explicitly named host composites rather than hiding the largest cells.
                    if !timings.candidates.is_zero() {
                        spans.push_aggregate_span(
                            "cpu_prepare",
                            timings.candidates,
                            Some(1),
                            None,
                            Some(match chunked_reason {
                                Some("host-budget") => {
                                    "initial-source-load-and-host-budget-probe-composite"
                                }
                                _ => "initial-source-load-and-candidate-prepare-composite",
                            }),
                        );
                    }
                    if !timings.pack.is_zero() {
                        spans.push_aggregate_span(
                            "pack",
                            timings.pack,
                            Some(1),
                            None,
                            Some("initial-one-pass-candidate-soa"),
                        );
                    }
                    if !timings.dem.is_zero() {
                        spans.push_aggregate_span(
                            "raster",
                            timings.dem,
                            Some(blocks as u64),
                            None,
                            Some("initial-one-pass-dem"),
                        );
                    }
                    spans.push_aggregate_span(
                        "source_load",
                        built.timings.source_load,
                        Some(1),
                        None,
                        Some("chunk-source-ring-load-and-view-build-composite"),
                    );
                    spans.push_aggregate_span(
                        "cpu_prepare",
                        built.timings.candidate_prepare_composite,
                        None,
                        None,
                        Some("chunk-candidate-enumeration-composite"),
                    );
                    spans.push_aggregate_span(
                        "pack",
                        built.timings.pack,
                        None,
                        None,
                        Some("chunk-candidate-soa"),
                    );
                    spans.push_aggregate_span(
                        "raster",
                        built.timings.raster,
                        None,
                        None,
                        Some("chunk-dem"),
                    );
                    if !abandoned_one_pass_wall.is_zero() {
                        spans.push_aggregate_span(
                            "gpu_pipeline_composite",
                            abandoned_one_pass_wall,
                            Some(1),
                            None,
                            Some("abandoned-one-pass-attempt-host-wall"),
                        );
                    }
                }
                spans.push_aggregate_span(
                    "vram_gate",
                    std::time::Duration::from_millis(vram_wait_ms.min(u64::MAX as u128) as u64),
                    Some(1),
                    None,
                    None,
                );
                spans.push_aggregate_span(
                    "accumulator_init",
                    built.timings.accumulator_init,
                    Some(1),
                    None,
                    None,
                );
                spans.push_aggregate_span(
                    "h2d",
                    built.timings.upload,
                    None,
                    None,
                    Some("region-soa-copy-and-sync-composite"),
                );
                // AirborneGpu::scatter_region currently combines receiver/meta H2D,
                // classify+physics kernels, D2H and host expansion. Keep the existing safe
                // boundary explicit; isolated gpu_kernel/d2h stay null until CUDA spans land.
                spans.push_aggregate_span(
                    "gpu_pipeline_composite",
                    built.timings.scatter,
                    None,
                    None,
                    Some("classify-scatter-copyback-expand"),
                );
                spans.push_aggregate_span(
                    "encode_write_composite",
                    built.timings.seal,
                    Some((built.written + built.skipped) as u64),
                    Some(built.output_bytes as u64),
                    None,
                );
                spans.finish_done(
                    protocol_wall,
                    built.written,
                    built.skipped,
                    Some(built.output_bytes),
                );
                for &(x, y) in &tiles {
                    let path = args
                        .output
                        .join(z.to_string())
                        .join(x.to_string())
                        .join(format!("{y}.bin"));
                    evidence
                        .tile_terminal(
                            r4,
                            "aircraft-airborne",
                            z,
                            x,
                            y,
                            &args.output,
                            &path,
                            "all-periods-silent",
                        )
                        .expect("emit GPU airborne tile terminal");
                }
                evidence
                    .region_terminal(
                        r4,
                        worker_id,
                        interval_id,
                        RegionTerminalStatus::Done,
                        built.written,
                        built.skipped,
                        None,
                    )
                    .expect("emit GPU airborne region terminal");
                let perf_suffix =
                    cell_perf_suffix(queue_ms, vram_wait_ms, build_ms, built.timings, detail);
                format!(
                    "done {r4:x} {} {} {total_ms} {perf_suffix}",
                    built.written, built.skipped,
                )
            }
            // Preparation/input failures and a cell too large even for exclusive chunking are
            // deterministic per-cell failures: report them so Hub attempts/parking can converge.
            // Only a build-stage error that may have poisoned the CUDA context kills the process.
            Err(e) => match cell_failure_disposition(preparation_failed, &e) {
                CellFailureDisposition::ReportCellFailure => {
                    let line = stream_cell_failure_line(r4, &e);
                    spans.finish_failed(cell_started.elapsed(), &line);
                    evidence
                        .region_terminal(
                            r4,
                            worker_id,
                            interval_id,
                            RegionTerminalStatus::Fail,
                            0,
                            0,
                            Some(&line),
                        )
                        .expect("emit GPU airborne region failure");
                    line
                }
                CellFailureDisposition::FatalProcess => {
                    spans.finish_failed(cell_started.elapsed(), &format!("{e:#}"));
                    evidence
                        .region_terminal(
                            r4,
                            worker_id,
                            interval_id,
                            RegionTerminalStatus::Fail,
                            0,
                            0,
                            Some(&format!("{e:#}")),
                        )
                        .expect("emit fatal GPU airborne region failure");
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{}", spans.line());
                    let _ = out.flush();
                    eprintln!("airborne: FATAL unrecoverable error on {r4:x}: {e:?}");
                    std::process::exit(1);
                }
            },
        };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", spans.line());
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// STREAM mode (`--stream`): the persistent warm worker the cluster orchestrator feeds — the
/// answer to the per-chunk process spawn + inter-chunk staging stall (~39% of box wall-time)
/// that capped the cluster's GPU at ~61% effective (a warm process sustains ~88-100%, STEP 1).
/// One process: CUDA context + NPD LUTs + class-weights + RealRasters all resident; R4 cell IDs
/// stream in on stdin and each is built by one prep coordinator (its own `RealRasters` + R4 LRU;
/// candidate preparation fans across Rayon) ahead of TWO persistent CUDA streams. Two is the
/// fleet-safe maximum: small cells overlap launch/sync gaps; a weighted VRAM gate makes a large or
/// chunked cell acquire both permits and run alone, preserving the old one-cell OOM safety. The
/// stages are joined by a depth-1 channel, so prep cannot build an unbounded host-RAM backlog.
/// A `start <r4hex> <unix_ms>` line opens each cell before CPU prep; one
/// `engine-spans-v1 {json}` line records the engine-local facts, then its unchanged result closes
/// it after GPU work. Stdout locking prevents interleave and the orchestrator may ACK out of order.
/// n_days + class_weights resolve once from `--seed-regions`.
///
/// Termination / deadlock-freedom: the reader (main scope thread) parses stdin onto the Morton
/// work queue; on EOF it sets the closed flag + `notify_all`. The prep thread drains contiguous
/// runs (PULL_BATCH) and on "closed + empty" returns and drops the only Sender. Both GPU workers
/// then observe receiver disconnect and exit. The receiver mutex is held only across `recv`, never
/// GPU work; the depth-1 `send` has a live consumer until every GPU worker has exited.
pub(crate) fn run_stream(args: &Args, z: u8) -> Result<()> {
    use std::collections::VecDeque;
    use std::io::BufRead;
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Condvar, Mutex};

    let seed = args.seed_regions.as_ref().context(
        "--stream requires --seed-regions (resolves the build-wide n_days + class_weights)",
    )?;
    let seed_r4s = tile_painter::region_runner::read_r4_file(seed)?;
    let source_r4s = ring_union(seed_r4s.iter().copied());
    if !any_source_arrow(&args.h3r4_dir, &source_r4s, SEL)? {
        bail!("--seed-regions has no airborne source — cannot resolve class_weights");
    }
    let resolved = resolve_n_days(&args.h3r4_dir, &source_r4s, SEL)?;
    let n_days = match args.n_days {
        Some(cli) if cli != resolved => {
            bail!("--n-days {cli} disagrees with arrow metadata ({resolved})")
        }
        _ => resolved,
    };
    let class_weights =
        tile_painter::worklist::resolve_class_weights(&args.h3r4_dir, &source_r4s, SEL, n_days)?;
    let bn = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    // One leaves small-cell launch gaps visible; the former rayon-thread-count pool opened 16-32
    // whole-region contexts and OOMed dense hubs. Keep 1 as a diagnostic override and clamp every
    // larger value to the reviewed two-stream ceiling.
    let n_workers = std::env::var("QM_GPU_STREAM_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 2);
    let evidence = RendererEvidence::from_env(
        "gpu-airborne",
        RuntimeParameters {
            zoom: z,
            batch_size: bn,
            n_days: Some(n_days),
            rayon_threads: rayon::current_num_threads(),
            stream_workers: n_workers,
            region_concurrency_configured: n_workers,
            region_concurrency_effective: n_workers,
            max_regions_per_claim: PULL_BATCH,
            layers: vec!["aircraft-airborne".to_string()],
        },
    )?;
    eprintln!(
        "stream: n_days={n_days}, batch={bn}, parallel-prep + {n_workers} VRAM-gated GPU stream(s) — reading R4 cells from stdin"
    );

    // Morton-locality work queue. The single prep thread pulls a CONTIGUOUS run of up to
    // PULL_BATCH cells from the front of a shared queue the reader fills in arrival (= the
    // orchestrator's Morton) order, so its grid_disk(1) ring-cache stays warm across the run —
    // even better than the old per-worker pool, which split the Morton stream across K caches.
    // The mutex is held only to splice a run off the front (cheap); prep_cell runs unlocked.
    const PULL_BATCH: usize = 4;
    let work: StreamQueue = Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
    // Depth-1: one prepared cell may wait behind the two device workers and the one being prepared.
    // Host RAM therefore stays bounded at roughly four ordinary cells.
    let (gpu_tx, gpu_rx) = sync_channel::<(
        u64,
        std::time::Instant,
        std::time::Instant,
        Result<PreparedCell>,
    )>(1);
    let gpu_rx: PreparedReceiver = Arc::new(Mutex::new(gpu_rx));
    let vram_gate = VramGate::new(n_workers);

    std::thread::scope(|scope| {
        // PREP THREAD (CPU only — no device touch). Owns its own RealRasters + R4 LRU (PER-prep,
        // not shared: it then locks only its OWN tile-store mutexes, the fix that broke the flat
        // 41%-CPU airborne ceiling — see 7746b452). Pulls Morton-contiguous runs, prep_cells each,
        // and sends the cell identity, pipeline clocks, and `Result<PreparedCell>` (the depth-1
        // `send` blocks when the channel is full — the desired backpressure). Owns the ONLY
        // Sender, so on exit the channel closes.
        let prep_work = Arc::clone(&work);
        scope.spawn(move || {
            let rasters = RealRasters::new(&args.prepared_dir);
            let mut cache = R4SourceCache::new(&args.h3r4_dir, args.r4_cache.max(7), SEL);
            loop {
                let batch: Vec<u64> = {
                    let (lock, cv) = &*prep_work;
                    let mut g = lock.lock().unwrap();
                    loop {
                        if !g.0.is_empty() {
                            let take = g.0.len().min(PULL_BATCH);
                            break g.0.drain(..take).collect();
                        }
                        if g.1 {
                            break Vec::new(); // stream closed + drained → exit
                        }
                        g = cv.wait(g).unwrap();
                    }
                };
                if batch.is_empty() {
                    break; // → return → drop gpu_tx → channel closes → GPU thread's `for` ends
                }
                for r4 in batch {
                    // A GPU-airborne cell is active from CPU preparation through its final GPU
                    // result; the depth-1 handoff is part of that one bounded pipeline lifetime.
                    let cell_started = std::time::Instant::now();
                    announce_stream_cell_started(r4);
                    let tiles = region_tiles(r4, z);
                    let prepared = prep_cell(&rasters, &mut cache, z, bn, r4, &tiles);
                    let prep_finished = std::time::Instant::now();
                    // A prep error (CPU/IO/source-load) is forwarded with the cell identity so the
                    // GPU thread emits `fail` and continues; otherwise a deterministic corrupt input
                    // could process-restart/TTL-requeue forever without ever reaching Hub parking.
                    // If the GPU thread has already gone (rx dropped), exit gracefully.
                    if gpu_tx
                        .send((r4, cell_started, prep_finished, prepared))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });

        for worker_id in 0..n_workers {
            let receiver = Arc::clone(&gpu_rx);
            let class_weights = &class_weights;
            let gate = &vram_gate;
            let evidence = evidence.clone();
            scope.spawn(move || {
                run_gpu_worker(
                    worker_id,
                    n_workers,
                    &receiver,
                    gate,
                    class_weights,
                    args,
                    n_days,
                    z,
                    bn,
                    evidence,
                )
            });
        }

        // Reader on the main scope thread (StdinLock is !Send): parse hex R4s onto the queue tail in
        // arrival order, waking the prep thread per cell. On EOF flag done + wake it so it drains + exits.
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let s = line.trim();
            if s.is_empty() {
                continue;
            }
            match u64::from_str_radix(s, 16) {
                Ok(r4) => {
                    let (lock, cv) = &*work;
                    lock.lock().unwrap().0.push_back(r4);
                    cv.notify_one();
                }
                Err(_) => eprintln!("stream: skip non-hex line: {s}"),
            }
        }
        let (lock, cv) = &*work;
        lock.lock().unwrap().1 = true;
        cv.notify_all();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use crate::build::BuildTimings;
    use crate::prep::PrepTimings;

    use super::{
        cell_failure_disposition, cell_perf_suffix, stream_cell_failure_line,
        CellFailureDisposition, DetailedCellPerf, PerfWindow, VramGate,
    };

    fn measured_detail() -> DetailedCellPerf {
        DetailedCellPerf {
            prep: PrepTimings {
                candidates: Duration::from_millis(1),
                pack: Duration::from_millis(2),
                dem: Duration::from_millis(3),
            },
            build: BuildTimings {
                accumulator_init: Duration::from_millis(6),
                upload: Duration::from_millis(7),
                scatter: Duration::from_millis(8),
                seal: Duration::from_millis(9),
                ..BuildTimings::default()
            },
            candidates: 10,
            blocks: 2,
            tiles: 20,
        }
    }

    #[test]
    fn perf_window_emits_one_low_volume_phase_summary() {
        let mut window = PerfWindow::default();
        for _ in 0..PerfWindow::CELLS - 1 {
            assert!(window
                .add(
                    4,
                    5,
                    30,
                    measured_detail().build,
                    Some(measured_detail()),
                    false,
                )
                .is_none());
        }
        let summary = window
            .add(
                4,
                5,
                30,
                measured_detail().build,
                Some(measured_detail()),
                false,
            )
            .expect("64th cell emits the aggregate");
        assert_eq!(
            summary,
            "64 cells avg: queue=4ms vram-wait=5ms build=30ms; all-build \
             avg[accum=6ms upload=7ms scatter=8ms seal=9ms]; one-pass=64 \
             avg[candidates=1ms pack=2ms dem=3ms shape=10cand/2blocks/20tiles]"
        );
        assert_eq!(window.cells, 0, "the next aggregate starts a fresh window");
    }

    #[test]
    fn chunked_perf_is_explicitly_unmeasured_never_zero_shaped() {
        let build = measured_detail().build;
        let suffix = cell_perf_suffix(4, 5, 30, build, None);
        assert_eq!(
            suffix,
            "prep_ms=unmeasured queue_ms=4 vram_wait_ms=5 build_ms=30 \
             accum_ms=6 upload_ms=7 scatter_ms=8 seal_ms=9 shape=chunked/unmeasured"
        );

        let mut window = PerfWindow::default();
        for _ in 0..PerfWindow::CELLS - 1 {
            assert!(window
                .add(
                    4,
                    5,
                    30,
                    measured_detail().build,
                    Some(measured_detail()),
                    false,
                )
                .is_none());
        }
        let summary = window
            .add(4, 5, 30, build, None, true)
            .expect("64th cell emits the aggregate");
        assert!(summary.contains("one-pass=63"));
        assert!(summary.contains("shape=chunked/unmeasured cells=1"));
        assert!(!summary.contains("shape=0cand"));
    }

    #[test]
    fn deterministic_preparation_error_reports_one_cell_without_killing_stream() {
        let error = anyhow::anyhow!("corrupt airborne.arrow");
        assert_eq!(
            cell_failure_disposition(true, &error),
            CellFailureDisposition::ReportCellFailure
        );
        assert_eq!(
            cell_failure_disposition(false, &error),
            CellFailureDisposition::FatalProcess
        );
        assert_eq!(
            stream_cell_failure_line(0x841e309ffffffff, &error),
            "fail 841e309ffffffff corrupt airborne.arrow"
        );
    }

    #[test]
    fn waiting_exclusive_cell_precedes_new_small_cells() {
        let gate = Arc::new(VramGate::new(2));
        let first_small = gate.acquire(1);

        let (exclusive_acquired_tx, exclusive_acquired_rx) = mpsc::channel();
        let (release_exclusive_tx, release_exclusive_rx) = mpsc::channel();
        let exclusive_gate = Arc::clone(&gate);
        let exclusive = std::thread::spawn(move || {
            let _lease = exclusive_gate.acquire(2);
            exclusive_acquired_tx.send(()).unwrap();
            release_exclusive_rx.recv().unwrap();
        });

        // Wait for the exclusive request to enter the gate. The state inspection makes the test
        // deterministic; sleeping and hoping the spawned thread ran would make this race-prone.
        {
            let mut state = gate.state.lock().unwrap();
            while state.exclusive_waiters == 0 {
                state = gate.changed.wait(state).unwrap();
            }
        }

        let (second_small_tx, second_small_rx) = mpsc::channel();
        let second_gate = Arc::clone(&gate);
        let second_small = std::thread::spawn(move || {
            let _lease = second_gate.acquire(1);
            second_small_tx.send(()).unwrap();
        });

        assert!(second_small_rx
            .recv_timeout(Duration::from_millis(20))
            .is_err());
        drop(first_small);
        exclusive_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exclusive cell should acquire both permits");
        assert!(second_small_rx
            .recv_timeout(Duration::from_millis(20))
            .is_err());

        release_exclusive_tx.send(()).unwrap();
        second_small_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("small cell should resume after the exclusive cell");
        exclusive.join().unwrap();
        second_small.join().unwrap();
    }
}
