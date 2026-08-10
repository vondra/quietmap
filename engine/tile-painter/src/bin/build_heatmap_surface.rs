//! Build surface HM3 heatmap tiles — the GROUND family: road/rail line sources,
//! industrial/building point sources, and airport ground-ops, all sharing ONE
//! terrain halo per batch. `--source ground` builds all five in a single pass:
//! the 10 km halo is built ONCE per batch and every layer scatters onto the hot
//! `Arc<FusedGrid>`, emitting five separate `{layer}/` trees. A single
//! `--source road|rail|industrial|building|aircraft-ground` keeps the per-layer
//! path (parity checks).
//!
//! Region by region (one centre-R4): load the region's `grid_disk(1)` sources
//! for every requested layer, batch its base-zoom tiles sharing one halo, scatter
//! (line / point / ground-ops kernel), collapse to the Lden byte (surface layers
//! steady-power; ground-ops event energy ÷ n_days), write `{layer}/{z}/{x}/{y}.bin`.
//! Regions run on an outer rayon over a Morton curve (axis 2, like the aircraft
//! builder); the per-region pipeline + telemetry live in `surface_region`.
//!
//!   build-heatmap-surface --source ground --bbox 49.9,14.3,50.1,14.6 --output …/build
//!   build-heatmap-surface --source road   --tile-x 2211 --tile-y 1386  --output …/build
//!
//! `--world` / `--shard` and the per-layer combine live in build-heatmap.sh.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use h3o::CellIndex;
use rayon::prelude::*;

use raster_reader::fused_tile_z13::{default_batch_size, tile_pixel_size_m, TILE_PX};
use raster_reader::{FusedPixel, RealRasters};

use noise_compute::admin;
use tile_painter::engine_spans::EngineCellSpans;
use tile_painter::grid::tile_range;
use tile_painter::r4_source_cache::SourceSel;
use tile_painter::region_runner::{
    announce_stream_cell_started, morton_order, read_r4_file, region_tiles,
    split_configured_layers, split_stream_line, tile_centre_r4,
};
use tile_painter::surface_region::{
    layer_meta, process_surface_region, Heartbeat, Source, SurfaceCtx, SurfaceStats,
};
use tile_painter::worklist::resolve_n_days;

/// Region concurrency when neither `--region-concurrency`/the env knob is set
/// nor `/proc/meminfo` is readable — never serialise just because RAM is unknown.
const FALLBACK_REGION_CONCURRENCY: usize = 16;

/// Per-region peak-RAM budget for the concurrency cap — the MEASURED peak RSS of one
/// cell's rest build (source `Vec` + terrain halo + scatter scratch), which
/// `region_concurrency` copies of run at once. Measured single-cell (concurrency 1):
/// Osaka `842e611`, the world's densest building R4 (4.1 M `PointRow`s), peaks at
/// **1.16 GB**; a typical/empty cell 0.3–0.4 GB. Budget 2 GB (≈1.7× the worst) so N
/// concurrent cells fit the box's memcap. The old halo-only estimate greenlit 16 dense
/// cells ≈ 18 GB and OOM-SIGKILLed the ~12 GB-memcap workers.
const PER_REGION_PEAK_BYTES: f64 = 2.0 * 1024.0 * 1024.0 * 1024.0;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, value_enum)]
    source: Source,
    #[arg(long)]
    tile_x: Option<u32>,
    #[arg(long)]
    tile_y: Option<u32>,
    /// `south_lat,west_lon,north_lat,east_lon`.
    #[arg(long, value_parser = parse_bbox)]
    bbox: Option<[f64; 4]>,
    #[arg(long, default_value_t = 12)]
    zoom: u8,
    #[arg(long)]
    h3r4_dir: PathBuf,
    #[arg(long)]
    prepared_dir: PathBuf,
    /// staging root — every layer writes `{output}/{layer}/{z}/{x}/{y}.bin`
    /// (ground mode emits all five; a single `--source` writes just that one).
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 0)]
    batch_size: u32,
    #[arg(long, default_value_t = false)]
    write_empty: bool,
    /// Layers to drop from the expanded set (repeat: `--exclude road --exclude
    /// rail`). The GPU hybrid uses this so the CPU ground pass builds only the
    /// point/ground layers while a GPU runner takes the line layers in parallel.
    #[arg(long, value_enum)]
    exclude: Vec<Source>,
    /// Build EXACTLY the output R4s listed in this file (one 15-hex cell/line)
    /// instead of --tile/--bbox — the cluster orchestrator's per-chunk work unit.
    #[arg(long)]
    regions_file: Option<PathBuf>,
    /// Ground-ops `n_days` divisor, resolved ONCE by the cluster master and
    /// passed to every chunk so they can't diverge. Omit for single-host runs
    /// (then it's resolved from the build's own airport_traffic arrows).
    #[arg(long)]
    n_days: Option<u16>,
    /// Max regions building concurrently (axis-2 region parallelism). 0 = the
    /// RAM-derived default. Each concurrent region holds one shared ~10 km halo
    /// (~10s–100s MB), so this caps peak halo memory; the inner 16×16 receiver
    /// blocks still steal across the whole rayon pool. Force `1` for an A/B
    /// byte-diff against the old sequential build.
    #[arg(long, default_value_t = 0)]
    region_concurrency: usize,
    /// STREAM mode: read output R4 cell IDs (one hex/line) from stdin and build each on a warm
    /// pool — n_days + class_weights + ONE shared RealRasters resident, at most region_concurrency
    /// halos in flight. Prints `start <r4hex> <unix_ms>` before work, one
    /// `engine-spans-v1 {json}` evidence line, then `done <r4hex> <written> <skipped> <ms>` (or
    /// `fail …`) per cell. The persistent CPU surface worker the cell-stream orchestrator feeds.
    #[arg(long, default_value_t = false)]
    stream: bool,
    /// STREAM mode: resolve the build-wide ground-ops n_days + class_weights ONCE from this seed
    /// regions-file (the orchestrator's representative source set); streamed cells inherit it.
    #[arg(long)]
    seed_regions: Option<PathBuf>,
}

/// Shared streaming work queue: (pending Morton-ordered (cell, per-cell stale-layers-request)
/// pairs, stream-closed flag) under a mutex + a condvar to park idle workers — identical to
/// gpu-airborne's, so the box agent feeds either. The optional `Vec<String>` is the stdin
/// line's `layers=` token (paint-pipeline-v4 PR#1 §3) — `None` = build every configured layer.
type StreamQueue = std::sync::Arc<(
    std::sync::Mutex<(std::collections::VecDeque<(u64, Option<Vec<String>>)>, bool)>,
    std::sync::Condvar,
)>;

fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.parse())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bbox: {e}"))?;
    if v.len() != 4 {
        return Err("expected south,west,north,east".into());
    }
    if v[0] >= v[2] || v[1] >= v[3] {
        return Err("need south<north and west<east".into());
    }
    Ok([v[0], v[1], v[2], v[3]])
}

/// How many regions may build at once. Capped because each concurrent region
/// holds its OWN loaded `grid_disk(1)` source set + scatter scratch for the
/// lifetime of its batches, and that drives peak RAM: the world's densest building
/// R4 (Osaka `842e611`, 4.1 M `PointRow`s) peaks at 1.16 GB per cell (MEASURED
/// single-cell); most cells 0.3–0.5 GB. Aircraft fans out one region per thread
/// (0-halo, bounded emitters); surface can't, or 16 dense megacity cells at once ≈
/// 18 GB → OOM-SIGKILL on a ~12 GB-memcap worker (32 GB box, 20 GB memcap reserve).
///
/// Priority: `--region-concurrency` > `QUIETMAP_SURFACE_REGION_CONCURRENCY` >
/// RAM-derived > a fallback constant. Always clamped to the pool size — more
/// in-flight regions than threads can advance buys nothing but memory. The
/// RAM-derived cap = budget × 0.8 ÷ `PER_REGION_PEAK_BYTES`, where `budget` is
/// the cgroup v2 `memory.max` (the actual `scripts/memcap` ceiling the engine
/// SIGKILLs at) when set, else physical `MemTotal` — so it respects the container
/// limit, not the host total. The old estimate budgeted only the halo and OOMed.
///
/// Realised as the CHUNK COUNT of `par_chunks` (chunk = one worker building its
/// regions sequentially), so at most `cap` source sets are resident while the
/// inner receiver-block `par_iter` still steals across the full pool — both axes
/// compose via rayon work-stealing.
fn region_concurrency(cli: usize, zoom: u8, halo_m: f64, batch_n: u32) -> usize {
    let pool = rayon::current_num_threads().max(1);
    if cli > 0 {
        return cli.min(pool).max(1);
    }
    if let Some(env) = std::env::var("QUIETMAP_SURFACE_REGION_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&v| v > 0)
    {
        return env.min(pool).max(1);
    }
    let Some(budget) = mem_budget_bytes() else {
        return FALLBACK_REGION_CONCURRENCY.min(pool).max(1);
    };
    // Equatorial (widest) tile metres × batch_n + halo on both sides — the actual
    // halo `TileBatch::build` allocates, upper-bounded across latitude. Tiny beside
    // the source set, but kept so a huge-halo / low-source config still fits.
    let tile_m = tile_pixel_size_m(zoom, 0.0) * TILE_PX as f64;
    let side_m = (batch_n as f64) * tile_m + 2.0 * halo_m;
    let halo_bytes = (side_m / 30.0).powi(2) * std::mem::size_of::<FusedPixel>() as f64;
    let per_region_bytes = halo_bytes.max(PER_REGION_PEAK_BYTES);
    ((budget as f64 * 0.8 / per_region_bytes) as usize).clamp(1, pool)
}

/// Total physical RAM in bytes from `/proc/meminfo` (`MemTotal`), or `None` off
/// Linux. Total (not available) keeps the cap stable across a run's own churn.
fn mem_total_bytes() -> Option<u64> {
    let txt = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = txt.lines().find_map(|l| l.strip_prefix("MemTotal:"))?;
    let kb: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// The RAM ceiling to budget concurrency against: the cgroup v2 `memory.max` the
/// engine actually runs under (`scripts/memcap` sets it via `systemd-run
/// -p MemoryMax=…` and the kernel SIGKILLs there), else physical `MemTotal`. Using
/// the real container limit — not the host total — is what stops the cap
/// over-committing a 32 GB worker whose memcap is only ~12 GB.
fn mem_budget_bytes() -> Option<u64> {
    cgroup_mem_max_bytes().or_else(mem_total_bytes)
}

/// cgroup v2 `memory.max` for THIS process, or `None` if unlimited (`"max"`) /
/// unreadable / cgroup v1. `/proc/self/cgroup` is a single `0::<path>` line under
/// v2; the limit lives at `/sys/fs/cgroup<path>/memory.max`.
fn cgroup_mem_max_bytes() -> Option<u64> {
    let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cg.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let max = std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.max")).ok()?;
    let max = max.trim();
    (max != "max").then(|| max.parse().ok()).flatten()
}

/// STREAM mode (`--stream`): the warm CPU surface worker the cell-stream orchestrator feeds — one
/// process with n_days + class_weights + ONE shared RealRasters resident (its mmap-LRU stays warm
/// across cells), R4 cell IDs read from stdin and each built on a warm OS-thread pool. At most
/// region_concurrency halos are in flight (the same memory cap the batch path uses). Per-cell
/// output is IDENTICAL to batch (same process_surface_region); only scheduling differs — the pool
/// is OS threads while the per-tile kernels use the global rayon pool (throughput, not bytes).
/// Prints `start <r4hex> <unix_ms>` before work, one `engine-spans-v1 {json}` evidence line, then
/// `done <r4hex> <written> <skipped> <ms>` (or `fail <r4hex> <err>`) per cell. n_days +
/// class_weights resolve ONCE from --seed-regions, so streamed cells inherit the build-wide value.
fn run_stream(args: &Args, layers: &[Source], halo_m: f64) -> Result<()> {
    use std::collections::VecDeque;
    use std::io::{BufRead, Write};
    use std::sync::{Arc, Condvar, Mutex};

    let seed = args.seed_regions.as_ref().context(
        "--stream requires --seed-regions (resolves the build-wide n_days + class_weights)",
    )?;
    // resolve_surface_{n_days,class_weights} key on .keys() only (they derive their own
    // grid_disk(1) source ring), so the tile values are unused — empty vecs skip computing
    // region_tiles for every seed cell.
    let seed_regions: BTreeMap<u64, Vec<(u32, u32)>> = read_r4_file(seed)?
        .into_iter()
        .map(|r4| (r4, Vec::new()))
        .collect();
    let n_days = resolve_surface_n_days(args, layers, &seed_regions)?;
    let class_weights = resolve_surface_class_weights(args, layers, &seed_regions, n_days)?;
    let rasters = RealRasters::new(&args.prepared_dir);
    let batch_n = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    let ctx = SurfaceCtx {
        zoom: args.zoom,
        halo_m,
        batch_n,
        n_days,
        class_weights,
        write_empty: args.write_empty,
        h3r4_dir: &args.h3r4_dir,
        output: &args.output,
        rasters: &rasters,
    };
    let heartbeat = Heartbeat::new(0, 0); // stream total is unknown; the per-cell `done` IS progress
    let n_workers = region_concurrency(args.region_concurrency, args.zoom, halo_m, batch_n);
    eprintln!(
        "stream: n_days={n_days}, layers={layers:?}, halo={halo_m:.0}m, {n_workers} worker(s) (≤ that many halos) — reading R4 cells from stdin"
    );

    // Morton-locality streaming pool (identical to gpu-airborne's): warm workers pull a CONTIGUOUS
    // run off a shared queue the reader fills in arrival (= Morton) order. One shared RealRasters +
    // n_workers caps the resident halos exactly like the batch path's region_concurrency.
    const PULL_BATCH: usize = 4;
    let work: StreamQueue = Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
    std::thread::scope(|scope| {
        for worker_slot in 0..n_workers {
            let work = Arc::clone(&work);
            let ctx = &ctx;
            let heartbeat = &heartbeat;
            scope.spawn(move || loop {
                let batch: Vec<(u64, Option<Vec<String>>)> = {
                    let (lock, cv) = &*work;
                    let mut g = lock.lock().unwrap();
                    loop {
                        if !g.0.is_empty() {
                            let take = g.0.len().min(PULL_BATCH);
                            break g.0.drain(..take).collect();
                        }
                        if g.1 {
                            break Vec::new();
                        }
                        g = cv.wait(g).unwrap();
                    }
                };
                if batch.is_empty() {
                    break;
                }
                for (r4, req_layers) in batch {
                    announce_stream_cell_started(r4);
                    let t = Instant::now();
                    let mut spans =
                        EngineCellSpans::new(r4, "build-heatmap-surface", worker_slot, t);
                    spans.metric_bool("cuda_event_timing_enabled", false);
                    let tiles = region_tiles(r4, ctx.zoom);
                    // Narrow this process's configured layers down to the requested (stale)
                    // subset for THIS cell — absent request = build every configured layer,
                    // today's behavior (paint-pipeline-v4 PR#1 §3). The agent only sends
                    // `layers=` for a strict subset of the group, so an EMPTY effective set
                    // means worker-config↔plan drift — fail LOUD (/fail → parked), never a
                    // hollow `done` that would let the hub seal an unbuilt stale layer.
                    let (effective, skipped) =
                        split_configured_layers(layers, req_layers.as_deref(), |s| layer_meta(s).2);
                    spans.metric("owned_tiles", serde_json::json!(tiles.len()));
                    spans.metric(
                        "effective_layers",
                        serde_json::json!(effective
                            .iter()
                            .map(|&s| layer_meta(s).2)
                            .collect::<Vec<_>>()),
                    );
                    let line = if effective.is_empty() {
                        let error = format!(
                            "fail {r4:x} layers-request matches none of configured [{}]",
                            skipped.join(",")
                        );
                        spans.finish_failed(t.elapsed(), &error);
                        error
                    } else {
                        match process_surface_region(ctx, r4, &tiles, heartbeat, &effective) {
                            Ok(st) => {
                                spans.push_aggregate_span(
                                    "source_load",
                                    st.t_load,
                                    Some(1),
                                    None,
                                    None,
                                );
                                spans.push_aggregate_span("raster", st.t_raster, None, None, None);
                                for (layer, layer_stats) in &st.by_layer {
                                    spans.push_aggregate_span(
                                        "cpu_scatter",
                                        layer_stats.scatter,
                                        None,
                                        None,
                                        Some(layer),
                                    );
                                }
                                // SurfaceStats currently measures collapse + encode + filesystem
                                // write as one interval. Keep it explicitly composite until those
                                // already-hot per-tile boundaries are split and measured.
                                spans.push_aggregate_span(
                                    "encode_write_composite",
                                    st.t_write,
                                    Some((st.written + st.skipped) as u64),
                                    Some(st.bytes as u64),
                                    None,
                                );
                                let wall = t.elapsed();
                                spans.finish_done(wall, st.written, st.skipped, Some(st.bytes));
                                format!(
                                    "done {r4:x} {} {} {}",
                                    st.written,
                                    st.skipped,
                                    wall.as_millis()
                                )
                            }
                            Err(e) => {
                                let line = format!("fail {r4:x} {e}");
                                spans.finish_failed(t.elapsed(), &line);
                                line
                            }
                        }
                    };
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{}", spans.line());
                    let _ = writeln!(out, "{line}");
                    let _ = out.flush();
                }
            });
        }
        // Reader on the main scope thread (StdinLock is !Send): hex R4s onto the queue tail in
        // arrival order, waking one worker each; on EOF flag done + wake all so they drain + exit.
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let s = line.trim();
            if s.is_empty() {
                continue;
            }
            let (hex, req_layers) = split_stream_line(s);
            match u64::from_str_radix(hex, 16) {
                Ok(r4) => {
                    let (lock, cv) = &*work;
                    lock.lock().unwrap().0.push_back((
                        r4,
                        req_layers.map(|v| v.into_iter().map(str::to_string).collect()),
                    ));
                    cv.notify_one();
                }
                Err(_) => eprintln!("stream: skip non-hex line: {s}"),
            }
        }
        let (lock, cv) = &*work;
        lock.lock().unwrap().1 = true;
        cv.notify_all();
    });
    if let Some(line) = noise_compute::propagation::census::report() {
        eprintln!("{line}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !(6..=18).contains(&args.zoom) {
        bail!("zoom {} out of range 6..=18", args.zoom);
    }

    // Concrete layers to build; `ground` fans out to all five. The shared halo
    // is the MAX reach among them (road 10 km in ground mode), built ONCE per
    // batch and reused by every layer's scatter — instead of one halo build per
    // layer-process today.
    let layers: Vec<Source> = match args.source {
        Source::Ground => vec![
            Source::Road,
            Source::Rail,
            Source::Industrial,
            Source::Building,
            Source::AircraftGround,
        ],
        s => vec![s],
    };
    // Drop any --exclude'd layers (the GPU hybrid excludes road/rail here so the
    // CPU pass builds only the point/ground layers alongside the GPU line pass).
    let layers: Vec<Source> = layers
        .into_iter()
        .filter(|l| !args.exclude.contains(l))
        .collect();
    if layers.is_empty() {
        bail!("all requested layers were excluded");
    }
    // Shared halo = the widest reach among the requested layers (road 10 km in
    // ground mode); a shorter-reach layer only ray-marches its own inner disk,
    // so the wider halo leaves its output unchanged.
    let halo_m = layers
        .iter()
        .map(|&s| layer_meta(s).1)
        .fold(0.0_f64, f64::max);

    // Admin table drives road default-AADT AND the C1 rail per-region period
    // split (EU freight ~55 % at night) — init when EITHER line layer is built,
    // else a rail-only run resolves Admin::UNKNOWN → world split while popup uses
    // real admin → parity break (Codex delta 1). Best-effort; filled ONCE here
    // before the parallel region loop — a process-wide OnceLock the per-region
    // loads only READ (concurrency-safe).
    if layers.contains(&Source::Road) || layers.contains(&Source::Rail) {
        let _ = admin::init_admin_table(&admin::default_admin_path(&args.h3r4_dir));
    }

    // Cell-stream worker: a warm process fed R4 cell IDs on stdin (admin table above is live;
    // mutually exclusive with the --tile/--bbox/--regions-file batch modes below).
    if args.stream {
        return run_stream(&args, &layers, halo_m);
    }

    // Target tiles → grouped by their centre R4 (a region loads its grid_disk(1)).
    let mut regions: BTreeMap<u64, Vec<(u32, u32)>> = BTreeMap::new();
    if let Some(rf) = &args.regions_file {
        // Cluster per-chunk unit: build exactly the listed output R4s' tiles.
        for r4 in read_r4_file(rf)? {
            regions.insert(r4, region_tiles(r4, args.zoom));
        }
        eprintln!("regions-file: {} output R4s", regions.len());
    } else {
        match (args.tile_x, args.tile_y, args.bbox) {
            (Some(x), Some(y), None) => {
                let r4 = tile_centre_r4(args.zoom, x, y).context("tile centre out of range")?;
                regions.entry(r4).or_default().push((x, y));
            }
            (None, None, Some(b)) => {
                let (xr, yr) = tile_range(args.zoom, b[0], b[1], b[2], b[3]);
                for y in yr {
                    for x in xr.clone() {
                        if let Some(r4) = tile_centre_r4(args.zoom, x, y) {
                            regions.entry(r4).or_default().push((x, y));
                        }
                    }
                }
            }
            _ => bail!("specify exactly one of --tile-x/--tile-y or --bbox"),
        }
    }
    if regions.is_empty() {
        bail!("no tiles to build");
    }
    let n_tiles: usize = regions.values().map(Vec::len).sum();

    let rasters = RealRasters::new(&args.prepared_dir);
    let batch_n = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };

    let n_days = resolve_surface_n_days(&args, &layers, &regions)?;
    let class_weights = resolve_surface_class_weights(&args, &layers, &regions, n_days)?;
    eprintln!(
        "{} region(s), {n_tiles} tile(s) at z={}, layers={:?}, halo={:.0} m, batch={batch_n}",
        regions.len(),
        args.zoom,
        layers,
        halo_m,
    );

    // Region order: a space-filling Morton curve so spatially-near regions stay
    // temporally near (raster tiles + adjacent halos stay hot). Each worker takes
    // one contiguous chunk of the curve.
    let order = morton_order(&regions.keys().copied().collect::<Vec<_>>());
    let concurrency =
        region_concurrency(args.region_concurrency, args.zoom, halo_m, batch_n).min(order.len());
    // chunk_size sized so the chunk COUNT == concurrency: at most `concurrency`
    // regions (halos) resident at once; inner blocks steal across the pool.
    let chunk_size = order.len().div_ceil(concurrency.max(1)).max(1);
    eprintln!(
        "region parallelism: {concurrency} concurrent ({} pool threads, chunk={chunk_size})",
        rayon::current_num_threads(),
    );

    let ctx = SurfaceCtx {
        zoom: args.zoom,
        halo_m,
        batch_n,
        n_days,
        class_weights,
        write_empty: args.write_empty,
        h3r4_dir: &args.h3r4_dir,
        output: &args.output,
        rasters: &rasters,
    };
    let heartbeat = Heartbeat::new(n_tiles, order.len());

    let t = Instant::now();
    let total: SurfaceStats = order
        .par_chunks(chunk_size)
        .map(|chunk| -> Result<SurfaceStats> {
            let mut local = SurfaceStats::default();
            for &r4 in chunk {
                local.merge(process_surface_region(
                    &ctx,
                    r4,
                    &regions[&r4],
                    &heartbeat,
                    &layers,
                )?);
            }
            Ok(local)
        })
        .try_reduce(SurfaceStats::default, |mut a, b| {
            a.merge(b);
            Ok(a)
        })?;

    report(&regions, n_tiles, &total, t.elapsed());
    if let Some(line) = noise_compute::propagation::census::report() {
        eprintln!("{line}");
    }
    Ok(())
}

/// Resolve the build-wide ground-ops Lden divisor. Cluster runs pass `--n-days`
/// verbatim (the master resolved one value over the whole area so chunks can't
/// diverge). Single-host runs derive it from the build's own airport_traffic
/// arrows; absent traffic → 1 (ground-ops then produces nothing). If ANY arrows
/// exist `resolve_n_days` MUST succeed — its Err on inconsistent/zero metadata
/// is the seam-guard, and dividing real events by 1 would overstate the layer by
/// 10·log10(n_days) (e.g. +25 dB for a 365-day window).
fn resolve_surface_n_days(
    args: &Args,
    layers: &[Source],
    regions: &BTreeMap<u64, Vec<(u32, u32)>>,
) -> Result<f64> {
    if let Some(n) = args.n_days {
        return Ok(n as f64);
    }
    if !layers.contains(&Source::AircraftGround) {
        return Ok(1.0);
    }
    let mut src = std::collections::BTreeSet::new();
    for &r4 in regions.keys() {
        let c = CellIndex::try_from(r4).expect("valid R4");
        src.extend(c.grid_disk::<Vec<_>>(1).into_iter().map(u64::from));
    }
    let src: Vec<u64> = src.into_iter().collect();
    let has_traffic = src.iter().any(|&r4| {
        args.h3r4_dir
            .join(format!("{r4:015x}"))
            .join("airport_traffic.arrow")
            .exists()
    });
    if !has_traffic {
        return Ok(1.0);
    }
    let traffic = SourceSel {
        cruise: false,
        airborne: false,
        traffic: true,
    };
    Ok(resolve_n_days(&args.h3r4_dir, &src, traffic)? as f64)
}

/// Resolve the GA 365-day hybrid weight LUT for the ground-ops layer
/// (the only surface layer that consumes it), from the build's
/// `airport_traffic.arrow` `sample_days_by_class` metadata. Uniform when
/// ground ops isn't in the build or no traffic arrows exist
/// (`ga-365d-hybrid-plan.md` §2).
fn resolve_surface_class_weights(
    args: &Args,
    layers: &[Source],
    regions: &BTreeMap<u64, Vec<(u32, u32)>>,
    n_days: f64,
) -> Result<noise_compute::emission::aircraft::ClassWeights> {
    use noise_compute::emission::aircraft::ClassWeights;
    if !layers.contains(&Source::AircraftGround) {
        return Ok(ClassWeights::uniform());
    }
    let mut src = std::collections::BTreeSet::new();
    for &r4 in regions.keys() {
        let c = CellIndex::try_from(r4).expect("valid R4");
        src.extend(c.grid_disk::<Vec<_>>(1).into_iter().map(u64::from));
    }
    let src: Vec<u64> = src.into_iter().collect();
    let traffic = SourceSel {
        cruise: false,
        airborne: false,
        traffic: true,
    };
    tile_painter::worklist::resolve_class_weights(&args.h3r4_dir, &src, traffic, n_days as u16)
}

/// End-of-build telemetry — same lines the sequential builder printed, summed
/// over the parallel workers (CPU-like under outer rayon).
fn report(
    regions: &BTreeMap<u64, Vec<(u32, u32)>>,
    n_tiles: usize,
    total: &SurfaceStats,
    elapsed: Duration,
) {
    let t_scatter: Duration = total.by_layer.values().map(|v| v.scatter).sum();
    let per_layer: String = total
        .by_layer
        .iter()
        .map(|(name, stats)| {
            // Pair-level skip where the kernel reports pairs (line/point, on the
            // byte-space stop); ground-ops has no pair count, so fall back to the
            // ray-level ratio it always printed.
            let n = if stats.pairs > 0 {
                stats.pairs
            } else {
                (stats.path_calls + stats.skipped_calls).max(1)
            };
            format!(
                "{name} {:.1}s/{:.2}Gpr({:.0}%skip)",
                stats.scatter.as_secs_f64(),
                n as f64 / 1e9,
                stats.skipped_calls as f64 / n as f64 * 100.0
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    eprintln!(
        "telemetry: {} regions, {n_tiles} tiles | {per_layer} | load {:.2}s · halo {:.2}s · scatter {:.2}s · write {:.2}s · wall {:.1}s",
        regions.len(),
        total.t_load.as_secs_f64(),
        total.t_raster.as_secs_f64(),
        t_scatter.as_secs_f64(),
        total.t_write.as_secs_f64(),
        elapsed.as_secs_f64(),
    );
    for (name, stats) in &total.by_layer {
        let n = if stats.pairs > 0 {
            stats.pairs
        } else {
            (stats.path_calls + stats.skipped_calls).max(1)
        };
        eprintln!(
            "layer-stats {name}: loaded_rows={} scatter_s={:.3} path_calls={} skipped_calls={} pairs={} skip_pct={:.1} raster_samples={} ground_rows_in_reach={} ground_unique_microsegs={}",
            stats.loaded_rows,
            stats.scatter.as_secs_f64(),
            stats.path_calls,
            stats.skipped_calls,
            stats.pairs,
            stats.skipped_calls as f64 / n as f64 * 100.0,
            stats.raster_samples,
            stats.ground_rows_in_reach,
            stats.ground_unique_microsegs,
        );
    }
    let cell_reads = total.raster_samples * 4; // bilinear quad (px00/01/10/11)
    eprintln!(
        "reads: {cell_reads} ray-march cell-reads (4 × {} samples) / {} batch-halo cells = {:.0}× re-read",
        total.raster_samples,
        total.grid_cells,
        cell_reads as f64 / total.grid_cells.max(1) as f64,
    );
    eprintln!(
        "done: {} tiles written, {} skipped, {} bytes, {:.1} s",
        total.written,
        total.skipped,
        total.bytes,
        elapsed.as_secs_f64(),
    );
}
