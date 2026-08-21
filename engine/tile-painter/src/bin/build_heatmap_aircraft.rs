//! Compute aircraft heatmap tiles at a configurable Mercator zoom,
//! region by region. Three target selections:
//!   --world                  every populated R4 (+ its grid_disk(1) ring)
//!   --bbox S,W,N,E           every tile at --zoom intersecting the bbox
//!   --tile-x X --tile-y Y    a single tile
//!
//! Each output region loads only its grid_disk(1) sources through a
//! bounded LRU, so resident memory stays flat as the build scales from
//! one airport to the whole globe. Regions are split into one contiguous
//! Morton-ordered chunk per worker, each with its own LRU; an optional
//! `--shard i/n` carves a contiguous slice for multi-host builds. See
//! `region_runner` for the per-region pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use h3o::CellIndex;
use raster_reader::fused_tile_z13::default_batch_size;
use raster_reader::RealRasters;
use rayon::prelude::*;

use tile_painter::engine_spans::EngineCellSpans;
use tile_painter::grid::tile_range;
use tile_painter::r4_source_cache::{R4SourceCache, SourceSel};
use tile_painter::region_runner::{
    announce_stream_cell_started, morton_order, process_region, read_r4_file, region_tiles,
    tile_centre_r4, RegionCtx, RegionStats,
};
use tile_painter::renderer_evidence::{
    maybe_run_static_attestation, DependencyProfile, RegionTerminalStatus, RendererEvidence,
    RuntimeParameters, StaticAttestationParameters,
};
use tile_painter::worklist::{any_source_arrow, resolve_n_days, WorkList};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Source {
    Cruise,
    Airborne,
    All,
}

impl Source {
    fn sel(self) -> SourceSel {
        match self {
            Source::Cruise => SourceSel {
                cruise: true,
                airborne: false,
                traffic: false,
            },
            Source::Airborne => SourceSel {
                cruise: false,
                airborne: true,
                traffic: false,
            },
            // Airport ground-ops moved to the surface ground pass (it ray-marches
            // terrain like road/rail); the aircraft binary is NPD-only now.
            Source::All => SourceSel {
                cruise: true,
                airborne: true,
                traffic: false,
            },
        }
    }
}

fn evidence_layer_for_selection(sel: SourceSel) -> &'static str {
    match (sel.cruise, sel.airborne) {
        (true, false) => "aircraft-cruise",
        (false, true) => "aircraft-airborne",
        (true, true) => "aircraft-combined",
        (false, false) => unreachable!("aircraft selection always enables a source"),
    }
}

#[derive(Parser, Debug)]
struct Args {
    /// Single tile — requires --tile-x and --tile-y.
    #[arg(long)]
    tile_x: Option<u32>,
    #[arg(long)]
    tile_y: Option<u32>,
    /// Bbox mode — `south_lat,west_lon,north_lat,east_lon`.
    #[arg(long, value_parser = parse_bbox)]
    bbox: Option<[f64; 4]>,
    /// World mode — every populated R4 plus its grid_disk(1) ring.
    #[arg(long, default_value_t = false)]
    world: bool,
    #[arg(long, default_value_t = 12)]
    zoom: u8,
    #[arg(long)]
    h3r4_dir: PathBuf,
    #[arg(long)]
    prepared_dir: PathBuf,
    /// Required to build; optional for --print-n-days (which exits before writing).
    #[arg(long)]
    output: Option<PathBuf>,
    /// Build-wide Lden divisor. Omit to derive it (and assert
    /// cross-arrow consistency) from metadata; if given, it is verified
    /// against the metadata and a mismatch is fatal.
    #[arg(long)]
    n_days: Option<u16>,
    #[arg(long, value_enum, default_value_t = Source::All)]
    source: Source,
    /// Per-batch dimension. 0 = auto-detect from L3 size.
    #[arg(long, default_value_t = 0)]
    batch_size: u32,
    /// Decoded-R4 LRU capacity. Keep ≥ grid_disk(1)=7 to cache a whole
    /// region's ring; the default holds several neighbouring regions.
    #[arg(long, default_value_t = 64)]
    r4_cache: usize,
    /// Disable the space-filling region order (iterate raw key order) —
    /// for measuring the cache re-load factor the ordering buys.
    #[arg(long, default_value_t = false)]
    no_r4_order: bool,
    /// Multi-host shard `i/n` — build only the i-th contiguous slice of
    /// the ordered work-list (0-based). rsync the disjoint outputs.
    #[arg(long, value_parser = parse_shard)]
    shard: Option<(usize, usize)>,
    #[arg(long, default_value_t = false)]
    write_empty: bool,
    /// Build EXACTLY the output R4s listed in this file (one 15-hex cell/line),
    /// instead of --world/--bbox/--tile. The cluster orchestrator's per-chunk
    /// work unit; disjoint chunks → no double build.
    #[arg(long)]
    regions_file: Option<PathBuf>,
    /// Resolve the build-wide `n_days` over the selection's sources, print it,
    /// and exit (no build). The cluster master calls this ONCE for the whole
    /// area, then passes `--n-days` to every chunk so they can't diverge.
    #[arg(long, default_value_t = false)]
    print_n_days: bool,
    /// STREAM mode: read output R4 cell IDs (one hex/line) from stdin and build each on a warm
    /// OS-thread pool — n_days + class_weights + RealRasters resident, each worker its own R4
    /// source LRU reused across cells (no per-chunk process spawn). Prints `start <r4hex>
    /// <unix_ms>` before work, one `engine-spans-v1 {json}` evidence line, and `done <r4hex>
    /// <written> <skipped> <ms>` (or `fail <r4hex> <err>`) as it finishes. The persistent CPU
    /// worker the cell-stream orchestrator feeds.
    #[arg(long, default_value_t = false)]
    stream: bool,
    /// STREAM mode: resolve the build-wide n_days + class_weights ONCE from this seed regions-file
    /// (the orchestrator's representative source set); streamed cells inherit it, asserted vs --n-days.
    #[arg(long)]
    seed_regions: Option<PathBuf>,
}

/// Shared streaming work queue: (pending Morton-ordered cells, stream-closed flag) under a mutex +
/// a condvar to park idle workers — identical to gpu-airborne's, so the box agent feeds either.
type StreamQueue = std::sync::Arc<(
    std::sync::Mutex<(std::collections::VecDeque<u64>, bool)>,
    std::sync::Condvar,
)>;

/// (output R4 → its target tiles, the source R4 set whose `n_days` must agree) — a batch mode's
/// region plan (regions-file / world / bbox / single tile).
type RegionPlan = (BTreeMap<u64, Vec<(u32, u32)>>, Vec<u64>);

fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return Err(format!("expected south,west,north,east; got {parts:?}"));
    }
    let v: Result<Vec<f64>, _> = parts.iter().map(|p| p.parse::<f64>()).collect();
    let v = v.map_err(|e| format!("bbox float parse: {e}"))?;
    if v[0] >= v[2] {
        return Err(format!("south {} must be < north {}", v[0], v[2]));
    }
    if v[1] >= v[3] {
        return Err(format!("west {} must be < east {}", v[1], v[3]));
    }
    Ok([v[0], v[1], v[2], v[3]])
}

/// Parse a `i/n` multi-host shard spec (0-based index, `n` shards).
fn parse_shard(s: &str) -> Result<(usize, usize), String> {
    let (i, n) = s.split_once('/').ok_or("expected i/n")?;
    let i: usize = i.parse().map_err(|e| format!("shard index: {e}"))?;
    let n: usize = n.parse().map_err(|e| format!("shard count: {e}"))?;
    if n == 0 || i >= n {
        return Err(format!("need 0 <= i < n, got {i}/{n}"));
    }
    Ok((i, n))
}

/// Union of `grid_disk(1)` over a set of output regions — the source R4s
/// those regions will load (and whose `n_days` must agree).
fn ring_union(regions: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut set: BTreeSet<u64> = BTreeSet::new();
    for r4 in regions {
        if let Ok(cell) = CellIndex::try_from(r4) {
            for n in cell.grid_disk::<Vec<_>>(1) {
                set.insert(u64::from(n));
            }
        }
    }
    set.into_iter().collect()
}

/// STREAM mode (`--stream`): the warm CPU aircraft worker the cell-stream orchestrator feeds — one
/// process with n_days + class_weights + RealRasters resident, R4 cell IDs read from stdin and each
/// built on a warm OS-thread pool (each worker its own R4SourceCache, reused across cells). Per-cell
/// output is IDENTICAL to the batch path (same region_tiles + process_region); only the scheduling
/// differs — the pool is OS threads while the per-tile kernels use the global rayon pool, so on a
/// big box this oversubscribes differently than batch's outer-rayon par_chunks (throughput, not
/// bytes). n_days + class_weights resolve ONCE from --seed-regions, as every chunk did in the batch.
fn run_stream(args: &Args, sel: SourceSel) -> Result<()> {
    use std::collections::VecDeque;
    use std::io::{BufRead, Write};
    use std::sync::{Arc, Condvar, Mutex};

    let seed = args.seed_regions.as_ref().context(
        "--stream requires --seed-regions (resolves the build-wide n_days + class_weights)",
    )?;
    let source_r4s = ring_union(read_r4_file(seed)?.into_iter());
    if !any_source_arrow(&args.h3r4_dir, &source_r4s, sel)? {
        bail!(
            "--seed-regions has no source for the selection — cannot resolve n_days/class_weights"
        );
    }
    let resolved = resolve_n_days(&args.h3r4_dir, &source_r4s, sel)?;
    let n_days = match args.n_days {
        Some(cli) if cli != resolved => {
            bail!("--n-days {cli} disagrees with arrow metadata ({resolved})")
        }
        _ => resolved,
    };
    let class_weights =
        tile_painter::worklist::resolve_class_weights(&args.h3r4_dir, &source_r4s, sel, n_days)?;
    let rasters = RealRasters::new(&args.prepared_dir);
    let batch_n = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    let output = args
        .output
        .as_deref()
        .context("--output is required to build")?;
    let ctx = RegionCtx {
        zoom: args.zoom,
        sel,
        n_days,
        class_weights,
        batch_n,
        output,
        h3r4_dir: &args.h3r4_dir,
        write_empty: args.write_empty,
        rasters: &rasters,
    };
    let n_workers = rayon::current_num_threads().max(1);
    let evidence_layers = vec![evidence_layer_for_selection(sel).to_string()];
    let evidence = RendererEvidence::from_env(
        "build-heatmap-aircraft",
        RuntimeParameters {
            zoom: args.zoom,
            batch_size: batch_n,
            n_days: Some(n_days),
            rayon_threads: rayon::current_num_threads(),
            stream_workers: n_workers,
            region_concurrency_configured: n_workers,
            region_concurrency_effective: n_workers,
            max_regions_per_claim: PULL_BATCH,
            layers: evidence_layers.clone(),
        },
    )?;
    eprintln!("stream: n_days={n_days}, {n_workers} worker(s) — reading R4 cells from stdin");

    // Morton-locality streaming pool (identical to gpu-airborne's): warm workers pull a CONTIGUOUS
    // run off a shared queue the reader fills in arrival (= Morton) order, so each keeps the batch
    // path's grid_disk(1) ring-cache reuse across its run. The mutex is held only to splice a run
    // off the front; the build runs unlocked. Inner kernels rayon-parallelise within a region.
    const PULL_BATCH: usize = 4;
    let work: StreamQueue = Arc::new((Mutex::new((VecDeque::new(), false)), Condvar::new()));
    std::thread::scope(|scope| {
        for worker_slot in 0..n_workers {
            let work = Arc::clone(&work);
            let ctx = &ctx;
            let evidence = evidence.clone();
            let evidence_layers = &evidence_layers;
            scope.spawn(move || {
                let mut cache = R4SourceCache::new(&args.h3r4_dir, args.r4_cache, ctx.sel);
                loop {
                    let batch: Vec<u64> = {
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
                    for r4 in batch {
                        let interval_id = evidence
                            .region_claim(r4, worker_slot)
                            .expect("emit aircraft region claim");
                        announce_stream_cell_started(r4);
                        let t = Instant::now();
                        let mut spans =
                            EngineCellSpans::new(r4, "build-heatmap-aircraft", worker_slot, t);
                        spans.metric_bool("cuda_event_timing_enabled", false);
                        let tiles = region_tiles(r4, ctx.zoom);
                        let evidence_layer_refs: Vec<&str> =
                            evidence_layers.iter().map(String::as_str).collect();
                        let dependencies = evidence.region_dependencies(
                            r4,
                            &args.prepared_dir,
                            &args.h3r4_dir,
                            &tiles,
                            ctx.zoom,
                            0.0,
                            &evidence_layer_refs,
                            DependencyProfile::Aircraft,
                        );
                        spans.metric("owned_tiles", serde_json::json!(tiles.len()));
                        spans.metric(
                            "sources",
                            serde_json::json!({
                                "cruise": ctx.sel.cruise,
                                "airborne": ctx.sel.airborne,
                            }),
                        );
                        let line = match dependencies
                            .and_then(|()| process_region(ctx, &mut cache, r4, &tiles))
                        {
                            Ok(st) => {
                                spans.push_aggregate_span(
                                    "source_load",
                                    st.t_load,
                                    Some(1),
                                    None,
                                    None,
                                );
                                spans.push_aggregate_span("raster", st.t_raster, None, None, None);
                                if ctx.sel.cruise {
                                    spans.push_aggregate_span(
                                        "cpu_scatter",
                                        st.t_cruise_scatter,
                                        None,
                                        None,
                                        Some("cruise"),
                                    );
                                }
                                if ctx.sel.airborne {
                                    spans.push_aggregate_span(
                                        "cpu_scatter",
                                        st.t_airborne_scatter,
                                        None,
                                        None,
                                        Some("airborne"),
                                    );
                                }
                                // RegionStats' existing write timer intentionally includes HM3
                                // collapse/encoding. Report that honest composite, not two guessed
                                // numbers, while retaining its exact byte counter.
                                spans.push_aggregate_span(
                                    "encode_write_composite",
                                    st.t_write,
                                    Some((st.tiles_written + st.tiles_skipped) as u64),
                                    Some(st.bytes_written as u64),
                                    None,
                                );
                                let wall = t.elapsed();
                                spans.finish_done(
                                    wall,
                                    st.tiles_written,
                                    st.tiles_skipped,
                                    Some(st.bytes_written),
                                );
                                for &(x, y) in &tiles {
                                    for &layer in &evidence_layer_refs {
                                        let output = ctx
                                            .output
                                            .join(ctx.zoom.to_string())
                                            .join(x.to_string())
                                            .join(format!("{y}.bin"));
                                        evidence
                                            .tile_terminal(
                                                r4,
                                                layer,
                                                ctx.zoom,
                                                x,
                                                y,
                                                ctx.output,
                                                &output,
                                                "all-periods-silent",
                                            )
                                            .expect("emit aircraft tile terminal");
                                    }
                                }
                                evidence
                                    .region_terminal(
                                        r4,
                                        worker_slot,
                                        interval_id,
                                        RegionTerminalStatus::Done,
                                        st.tiles_written,
                                        st.tiles_skipped,
                                        None,
                                    )
                                    .expect("emit aircraft region terminal");
                                format!(
                                    "done {r4:x} {} {} {}",
                                    st.tiles_written,
                                    st.tiles_skipped,
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
                                    .expect("emit aircraft region failure");
                                line
                            }
                        };
                        let mut out = std::io::stdout().lock();
                        let _ = writeln!(out, "{}", spans.line());
                        let _ = writeln!(out, "{line}");
                        let _ = out.flush();
                    }
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

fn main() -> Result<()> {
    let args = Args::parse();
    if !(6..=18).contains(&args.zoom) {
        bail!("zoom {} out of supported range 6..=18", args.zoom);
    }

    // Which source layers to build (--source); the work-list + n_days
    // checks only consider these arrow types.
    let sel = args.source.sel();
    let static_layers = vec![evidence_layer_for_selection(sel).to_string()];
    let static_batch_n = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    if maybe_run_static_attestation(
        "build-heatmap-aircraft",
        StaticAttestationParameters {
            runtime: RuntimeParameters {
                zoom: args.zoom,
                batch_size: static_batch_n,
                n_days: args.n_days,
                rayon_threads: rayon::current_num_threads(),
                stream_workers: rayon::current_num_threads(),
                region_concurrency_configured: rayon::current_num_threads(),
                region_concurrency_effective: rayon::current_num_threads(),
                max_regions_per_claim: 4,
                layers: static_layers.clone(),
            },
            accepted_options: [
                "--batch-size/1",
                "--bbox/1",
                "--h3r4-dir/1",
                "--n-days/1",
                "--output/1",
                "--prepared-dir/1",
                "--print-n-days/0",
                "--r4-cache/1",
                "--regions-file/1",
                "--seed-regions/1",
                "--shard/1",
                "--source/1",
                "--stream/0",
                "--tile-x/1",
                "--tile-y/1",
                "--world/0",
                "--write-empty/0",
                "--zoom/1",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            prepared_root: args.prepared_dir.clone(),
            h3r4_dir: args.h3r4_dir.clone(),
            halo_m: 0.0,
            layers: static_layers,
            profile: DependencyProfile::Aircraft,
        },
    )? {
        return Ok(());
    }

    // Cell-stream worker: a warm process fed R4 cell IDs on stdin (mutually exclusive with the
    // --world/--bbox/--regions-file batch modes below).
    if args.stream {
        return run_stream(&args, sel);
    }

    // Cluster master fast path: resolve + print the build-wide n_days and exit.
    // Every chunk then gets the SAME divisor via --n-days; a per-chunk
    // resolve_n_days would only see its own R4s and could silently diverge from
    // the whole-area value (a 14- vs 365-day seam is ~14 dB). /gg.
    if args.print_n_days {
        let src = match &args.regions_file {
            Some(rf) => ring_union(read_r4_file(rf)?.into_iter()),
            None if args.world => WorkList::scan(&args.h3r4_dir, sel)?.source_r4s,
            None => bail!("--print-n-days needs --regions-file or --world"),
        };
        println!("{}", resolve_n_days(&args.h3r4_dir, &src, sel)?);
        return Ok(());
    }

    // Output regions (R4 → its target tiles) + the source R4 set whose
    // n_days we assert consistent. World derives both from the disk
    // work-list; bbox/single group their tiles by centre-R4 and take the
    // grid_disk(1) load set of the touched regions.
    let (regions, source_r4s): RegionPlan = if let Some(rf) = &args.regions_file {
        // Cluster per-chunk unit: build EXACTLY the listed output R4s (each its
        // full region_tiles). Disjoint chunks (centre-R4 ownership) → no tile
        // built twice; the area's outer edge slightly overhangs the bbox.
        let r4s = read_r4_file(rf)?;
        eprintln!("regions-file: {} output R4s", r4s.len());
        let regions = r4s
            .iter()
            .map(|&r4| (r4, region_tiles(r4, args.zoom)))
            .collect();
        (regions, ring_union(r4s.iter().copied()))
    } else {
        match (args.world, args.tile_x, args.tile_y, args.bbox) {
            (true, None, None, None) => {
                let wl = WorkList::scan(&args.h3r4_dir, sel)?;
                eprintln!(
                    "world: {} source R4s → {} output regions",
                    wl.source_r4s.len(),
                    wl.output_r4s.len()
                );
                let regions = wl
                    .output_r4s
                    .iter()
                    .map(|&r4| (r4, region_tiles(r4, args.zoom)))
                    .collect();
                (regions, wl.source_r4s)
            }
            (false, Some(x), Some(y), None) => {
                let r4 =
                    tile_centre_r4(args.zoom, x, y).context("tile centre lat/lon out of range")?;
                (
                    BTreeMap::from([(r4, vec![(x, y)])]),
                    ring_union(std::iter::once(r4)),
                )
            }
            (false, None, None, Some(b)) => {
                let (xr, yr) = tile_range(args.zoom, b[0], b[1], b[2], b[3]);
                let mut regions: BTreeMap<u64, Vec<(u32, u32)>> = BTreeMap::new();
                for y in yr {
                    for x in xr.clone() {
                        if let Some(r4) = tile_centre_r4(args.zoom, x, y) {
                            regions.entry(r4).or_default().push((x, y));
                        }
                    }
                }
                let src = ring_union(regions.keys().copied());
                (regions, src)
            }
            _ => bail!("specify exactly one of --world, --tile-x/--tile-y, or --bbox"),
        }
    };

    if regions.is_empty() {
        bail!("no regions to build");
    }
    let n_tiles: usize = regions.values().map(Vec::len).sum();

    // A cluster chunk can hold one source but not another (a rural chunk with road data but
    // no airborne); building the absent one is a no-op, not the fatal "no source arrows" that
    // resolve_n_days raises (Codex /gg). Without this a no-airborne chunk fails the whole job —
    // and post-E7c that job (the GPU `line` job) also owns road/rail, which would be lost.
    if !any_source_arrow(&args.h3r4_dir, &source_r4s, sel)? {
        eprintln!("no source arrows for the selection in this chunk — nothing to build");
        return Ok(());
    }
    // One build-wide n_days, data-derived and verified against any
    // explicit --n-days (drops the old silent default that shipped a
    // 17 dB error on 2026-05-25).
    let resolved = resolve_n_days(&args.h3r4_dir, &source_r4s, sel)?;
    let n_days = match args.n_days {
        Some(cli) if cli != resolved => {
            bail!("--n-days {cli} disagrees with arrow metadata ({resolved})")
        }
        _ => resolved,
    };
    // GA 365-day hybrid weight LUT, resolved once build-wide from the
    // source arrows' `sample_days_by_class` (consistency-asserted like
    // n_days). Threaded into the airborne scatter; cruise is airline-only.
    let class_weights =
        tile_painter::worklist::resolve_class_weights(&args.h3r4_dir, &source_r4s, sel, n_days)?;
    eprintln!(
        "{} region(s), {} tile(s) at z={}, n_days={}",
        regions.len(),
        n_tiles,
        args.zoom,
        n_days
    );

    let rasters = RealRasters::new(&args.prepared_dir);
    let batch_n = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    let output = args
        .output
        .as_deref()
        .context("--output is required to build")?;
    let ctx = RegionCtx {
        zoom: args.zoom,
        sel,
        n_days,
        class_weights,
        batch_n,
        output,
        h3r4_dir: &args.h3r4_dir,
        write_empty: args.write_empty,
        rasters: &rasters,
    };

    // Space-filling region order so neighbouring regions' grid_disk(1)
    // rings stay hot in the LRU (raw key order is geographically random).
    let keys: Vec<u64> = regions.keys().copied().collect();
    let n_total = keys.len();
    let mut order = if args.no_r4_order {
        keys
    } else {
        morton_order(&keys)
    };

    // Multi-host shard: a contiguous slice keeps each host's Morton
    // locality intact; the disjoint outputs rsync together.
    if let Some((i, n)) = args.shard {
        let chunk = order.len().div_ceil(n);
        let lo = (i * chunk).min(order.len());
        let hi = ((i + 1) * chunk).min(order.len());
        order = order[lo..hi].to_vec();
        eprintln!("shard {i}/{n}: {} of {n_total} regions", order.len());
    }
    if order.is_empty() {
        eprintln!("nothing to build — shard/bbox selected 0 regions");
        return Ok(());
    }

    // Within-machine parallelism: ≈ one contiguous chunk per worker
    // (fewer on small runs) — each keeps its own sub-curve hot with its
    // OWN LRU, lock-free since RealRasters is Sync (per-slot Mutex) and
    // shared read-only. Inner kernels also use rayon; under outer
    // saturation they run near-sequentially per region — the win for the
    // sparse regions that can't fill the cores on their own.
    let n_threads = rayon::current_num_threads().max(1);
    let chunk_size = order.len().div_ceil(n_threads).max(1);
    eprintln!(
        "{n_threads} workers × --r4-cache {} R4 (per-worker LRU; \
         RAM ≈ workers × cache × R4 size — hub R4s are 100s of MB)",
        args.r4_cache
    );
    let t = Instant::now();
    let (total, hits, misses) = order
        .par_chunks(chunk_size)
        .map(|chunk| -> Result<(RegionStats, u64, u64)> {
            let mut cache = R4SourceCache::new(&args.h3r4_dir, args.r4_cache, sel);
            let mut local = RegionStats::default();
            for &r4 in chunk {
                local.merge(process_region(&ctx, &mut cache, r4, &regions[&r4])?);
            }
            let (h, m) = cache.stats();
            Ok((local, h, m))
        })
        .try_reduce(
            || (RegionStats::default(), 0, 0),
            |(mut a, ah, am), (b, bh, bm)| {
                a.merge(b);
                Ok((a, ah + bh, am + bm))
            },
        )?;

    let hit_pct = 100.0 * hits as f64 / (hits + misses).max(1) as f64;
    eprintln!(
        "done: {} tiles written, {} skipped, {} bytes, {} region(s), {:.1} s",
        total.tiles_written,
        total.tiles_skipped,
        total.bytes_written,
        order.len(),
        t.elapsed().as_secs_f64()
    );
    eprintln!(
        "phases (summed, CPU-like under outer rayon): load {:.1}s · raster {:.1}s · scatter {:.1}s (cruise {:.1}s · airborne {:.1}s) · write {:.1}s",
        total.t_load.as_secs_f64(),
        total.t_raster.as_secs_f64(),
        total.t_scatter.as_secs_f64(),
        total.t_cruise_scatter.as_secs_f64(),
        total.t_airborne_scatter.as_secs_f64(),
        total.t_write.as_secs_f64(),
    );
    let c = &total.cruise;
    if c.buckets_seen > 0 {
        eprintln!(
            "cruise: {} buckets ({} in reach, {} terrain rejected, {} broadcast); {} kernel evals ({} below 20 dB)",
            c.buckets_seen,
            c.buckets_in_reach,
            c.buckets_terrain_rejected,
            c.buckets_broadcast,
            c.pairs_evaluated,
            c.pairs_below_threshold,
        );
    }
    let a = &total.airborne;
    if a.sub_segments_seen > 0 {
        eprintln!(
            "airborne: {} rows ({} in bbox), {} sub-segs → pruned {} cpa / {} slant / {} ground-stale; \
             admitted {} near + {} coarse [<2km {} / 2-8km {} / ≥8km {}]; {} kernel evals ({} below 20 dB)",
            a.rows_seen, a.rows_bbox_pass, a.sub_segments_seen,
            a.sub_segments_outside_tile, a.sub_segments_slant_pruned, a.sub_segments_invalid,
            a.sub_near, a.sub_segments_coarse,
            a.coarse_band[0], a.coarse_band[1], a.coarse_band[2],
            a.pairs_evaluated, a.pairs_below_threshold,
        );
    }
    eprintln!(
        "cache: {hits} hits / {misses} misses ({hit_pct:.0}% hit, {n_threads} threads, order={})",
        if args.no_r4_order { "raw" } else { "morton" }
    );
    Ok(())
}
