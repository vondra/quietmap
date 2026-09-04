//! Production GPU airborne builder: the region-resident kernel (`noise_gpu::airborne`)
//! wired into the cluster's per-chunk region loop. Mirrors `build-heatmap-aircraft`'s CLI
//! (`--regions-file --n-days --h3r4-dir --prepared-dir --zoom --output`) so
//! `cluster-build-chunk.sh` can swap it in for the airborne source on a GPU box. Cruise
//! stays on the CPU builder (a different kernel, not yet ported).
//!
//! One GPU → the region loop is SEQUENTIAL (the device parallelises within each tile);
//! Morton order keeps the R4 source LRU hot, exactly like the CPU builder.
//!
//!   NOISE_GPU_PREPARED=… DATA_YEAR=… gpu-airborne --regions-file <r4-list> --n-days N \
//!       --h3r4-dir <h3r4> --prepared-dir <prep> --zoom <z> --output <dir>
//!   printf '841e309ffffffff tiles=4414,2786,4\n' | gpu-airborne --stream …   (bounded window)
//!   gpu-airborne --bbox S,W,N,E …      gpu-airborne --tile-x X --tile-y Y …   (dev modes)
//!
//! Submodules: `prep` (CPU prep stage — pack candidates, receiver lattices, and obstacle data),
//! `build` (GPU build stage — scatter the SoA into per-tile accumulators + write tiles),
//! `stream` (the persistent `--stream` parallel-prep/GPU double buffer).

// This bin's path is `src/gpu_airborne.rs` (Cargo.toml `[[bin]] path`), so Rust resolves a bare
// `mod prep;` to `src/prep.rs`, NOT `src/gpu_airborne/prep.rs`. Point each submodule at the
// `gpu_airborne/` subdirectory explicitly (the `main.rs`/`mod.rs` auto-subdir rule doesn't apply
// to a custom-named bin entry).
#[path = "gpu_airborne/build.rs"]
mod build;
#[path = "gpu_airborne/prep.rs"]
mod prep;
#[path = "gpu_airborne/stream.rs"]
mod stream;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use h3o::CellIndex;
use noise_gpu::airborne::AirborneGpu;
use raster_reader::fused_tile_z13::default_batch_size;
use raster_reader::RealRasters;
use rayon::prelude::*;
use tile_painter::grid::tile_range;
use tile_painter::r4_source_cache::{R4SourceCache, SourceSel};
use tile_painter::region_runner::{morton_order, region_tiles, tile_centre_r4};
use tile_painter::tile_store::PUBLISHED_BASE_ZOOM;
use tile_painter::worklist::{any_source_arrow, resolve_n_days};

use crate::build::process_region_gpu;
use crate::stream::run_stream;

/// Airborne only — cruise/traffic ride the CPU builder for now.
pub(crate) const SEL: SourceSel = SourceSel {
    cruise: false,
    airborne: true,
    traffic: false,
};

#[derive(Parser, Debug)]
pub(crate) struct Args {
    /// Build EXACTLY the output R4s in this file (one 15-hex cell/line) — the cluster's
    /// per-chunk unit. Disjoint chunks (centre-R4 ownership) → no tile built twice.
    #[arg(long)]
    regions_file: Option<PathBuf>,
    /// Dev: every tile at --zoom intersecting `south,west,north,east`.
    #[arg(long, value_parser = parse_bbox)]
    bbox: Option<[f64; 4]>,
    /// Dev: a single tile (requires --tile-y).
    #[arg(long)]
    tile_x: Option<u32>,
    #[arg(long)]
    tile_y: Option<u32>,
    /// The zoom the world is painted at; every lower zoom is a pyramid level of this paint.
    #[arg(long, default_value_t = PUBLISHED_BASE_ZOOM)]
    pub(crate) zoom: u8,
    #[arg(long)]
    pub(crate) h3r4_dir: PathBuf,
    #[arg(long)]
    pub(crate) prepared_dir: PathBuf,
    #[arg(long)]
    pub(crate) output: PathBuf,
    /// Build-wide Lden divisor. Omit to derive it from arrow metadata; if given it is
    /// verified against the metadata (a mismatch is fatal — same contract as the CPU builder).
    #[arg(long)]
    n_days: Option<u16>,
    /// Per-batch dimension. 0 = auto-detect from L3 size.
    #[arg(long, default_value_t = 0)]
    batch_size: u32,
    /// Decoded-R4 LRU capacity (≥ grid_disk(1)=7 to cache a region's ring). 16 keeps the ring
    /// plus a locality margin warm while bounding the decoded-source baseline — 64 (the old,
    /// untuned default) accumulated ~16 GB of dense hub-cell sources in the LRU, starving a dense
    /// cell's region_candidates Vec of host memcap. Morton order shares 4-5 of 7 ring neighbours,
    /// so 16 holds the working set with no extra re-decodes.
    #[arg(long, default_value_t = 16)]
    pub(crate) r4_cache: usize,
    #[arg(long, default_value_t = false)]
    pub(crate) write_empty: bool,
    /// STREAM mode: read output R4 cells (one `<r4hex> [tiles=x,y,side]` per line) from stdin and
    /// build each in a warm pipeline: parallel CPU candidate prep ahead of two VRAM-gated CUDA
    /// streams (large cells automatically run alone). Prints `done <r4hex> <written> <skipped>
    /// <ms> ...` (or `fail <r4hex> <err>`) per cell as it finishes. The persistent worker the
    /// cluster orchestrator feeds. Requires --seed-regions (resolves n_days + class_weights once).
    #[arg(long, default_value_t = false)]
    stream: bool,
    /// STREAM mode: resolve the build-wide n_days + class_weights ONCE at startup from this seed
    /// regions-file (the orchestrator's representative source set). Streamed cells inherit it,
    /// consistency-asserted vs --n-days — same contract as the batch path's single resolve.
    #[arg(long)]
    seed_regions: Option<PathBuf>,
}

fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.parse::<f64>().map_err(|e| format!("bbox float: {e}")))
        .collect::<Result<_, _>>()?;
    if v.len() != 4 {
        return Err(format!(
            "expected south,west,north,east; got {} values",
            v.len()
        ));
    }
    if v[0] >= v[2] || v[1] >= v[3] {
        return Err(format!("need south<north and west<east, got {v:?}"));
    }
    Ok([v[0], v[1], v[2], v[3]])
}

/// Union of `grid_disk(1)` over the output regions — the source R4 set whose `n_days` must agree.
pub(crate) fn ring_union(regions: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut set: BTreeSet<u64> = BTreeSet::new();
    for r4 in regions {
        if let Ok(cell) = CellIndex::try_from(r4) {
            for nbr in cell.grid_disk::<Vec<_>>(1) {
                set.insert(u64::from(nbr));
            }
        }
    }
    set.into_iter().collect()
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !(6..=18).contains(&args.zoom) {
        bail!("zoom {} out of supported range 6..=18", args.zoom);
    }
    let z = args.zoom;
    let static_batch_n = if args.batch_size == 0 {
        raster_reader::fused_tile_z13::default_batch_size()
    } else {
        args.batch_size
    };
    let static_workers = std::env::var("QM_GPU_STREAM_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 2);
    if tile_painter::renderer_evidence::maybe_run_static_attestation(
        "gpu-airborne",
        tile_painter::renderer_evidence::StaticAttestationParameters {
            runtime: tile_painter::renderer_evidence::RuntimeParameters {
                zoom: z,
                batch_size: static_batch_n,
                n_days: args.n_days,
                rayon_threads: rayon::current_num_threads(),
                stream_workers: static_workers,
                region_concurrency_configured: static_workers,
                region_concurrency_effective: static_workers,
                max_regions_per_claim: 4,
                layers: vec!["aircraft-airborne".to_string()],
            },
            accepted_options: [
                "--batch-size/1",
                "--bbox/1",
                "--h3r4-dir/1",
                "--n-days/1",
                "--output/1",
                "--prepared-dir/1",
                "--r4-cache/1",
                "--regions-file/1",
                "--seed-regions/1",
                "--stream/0",
                "--tile-x/1",
                "--tile-y/1",
                "--write-empty/0",
                "--zoom/1",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            prepared_root: args.prepared_dir.clone(),
            h3r4_dir: args.h3r4_dir.clone(),
            halo_m: noise_compute::emission::aircraft::RECEIVER_HORIZON_MAX_M,
            layers: vec!["aircraft-airborne".to_string()],
            profile: tile_painter::renderer_evidence::DependencyProfile::Aircraft,
        },
    )? {
        return Ok(());
    }

    if args.stream {
        return run_stream(&args, z);
    }

    // Output regions (R4 → its target tiles): regions-file (cluster) | bbox | single tile (dev).
    let regions: BTreeMap<u64, Vec<(u32, u32)>> =
        match (&args.regions_file, &args.bbox, args.tile_x, args.tile_y) {
            (Some(rf), None, None, None) => {
                let r4s = tile_painter::region_runner::read_r4_file(rf)?;
                eprintln!("regions-file: {} output R4s", r4s.len());
                r4s.into_iter()
                    .map(|r4| (r4, region_tiles(r4, z)))
                    .collect()
            }
            (None, Some(b), None, None) => {
                let (xr, yr) = tile_range(z, b[0], b[1], b[2], b[3]);
                let mut m: BTreeMap<u64, Vec<(u32, u32)>> = BTreeMap::new();
                for y in yr {
                    for x in xr.clone() {
                        if let Some(r4) = tile_centre_r4(z, x, y) {
                            m.entry(r4).or_default().push((x, y));
                        }
                    }
                }
                m
            }
            (None, None, Some(x), Some(y)) => {
                let r4 = tile_centre_r4(z, x, y).context("tile centre out of range")?;
                BTreeMap::from([(r4, vec![(x, y)])])
            }
            _ => bail!("specify exactly one of --regions-file, --bbox, or --tile-x/--tile-y"),
        };
    if regions.is_empty() {
        bail!("no regions to build");
    }

    // One build-wide n_days, data-derived and verified against any explicit --n-days (the
    // cluster resolves it once for the whole area and passes it to every chunk).
    let source_r4s = ring_union(regions.keys().copied());
    // A chunk can hold road/rail but no airborne (rural). Building the absent airborne is a
    // no-op, not a fatal resolve — else the shared `line` job loses its road/rail too.
    if !any_source_arrow(&args.h3r4_dir, &source_r4s, SEL)? {
        eprintln!("no airborne data in this chunk — nothing to build");
        return Ok(());
    }
    let resolved = resolve_n_days(&args.h3r4_dir, &source_r4s, SEL)?;
    let n_days = match args.n_days {
        Some(cli) if cli != resolved => {
            bail!("--n-days {cli} disagrees with arrow metadata ({resolved})")
        }
        _ => resolved,
    };
    // GA full-year hybrid weight LUT, resolved once build-wide from the
    // source arrows' `sample_days_by_class` (consistency-asserted like
    // n_days) and uploaded device-global by `AirborneGpu::new`.
    let class_weights =
        tile_painter::worklist::resolve_class_weights(&args.h3r4_dir, &source_r4s, SEL, n_days)?;
    let n_tiles: usize = regions.values().map(Vec::len).sum();
    eprintln!(
        "{} region(s), {n_tiles} tile(s) at z={z}, n_days={n_days}",
        regions.len()
    );

    let rasters = RealRasters::new(&args.prepared_dir);
    let bn = if args.batch_size == 0 {
        default_batch_size()
    } else {
        args.batch_size
    };
    // Per-worker GPU + LRU, rayon over a contiguous Morton chunk per worker — mirrors
    // build_heatmap_aircraft's par_chunks. Each worker owns its AirborneGpu (its own CUDA
    // stream, M1b) and its R4SourceCache. The near/far candidate gate that used to run
    // single-threaded per tile on the CPU (the "1 core at 98%, 15 idle" wall) now runs on
    // the GPU as a counting-sort inside `scatter_region` (M4), so the device stays saturated
    // (~95%) instead of stalling on the CPU between launches. Commutative over regions, and
    // parity-equivalent to the per-tile `scatter_tile` (compare_hm3: 0 cells > 0.5 dB).
    let order = morton_order(&regions.keys().copied().collect::<Vec<_>>());
    let n_workers = rayon::current_num_threads().max(1);
    let chunk_size = order.len().div_ceil(n_workers).max(1);
    let t = Instant::now();
    let (written, skipped, hits, misses) = order
        .par_chunks(chunk_size)
        .map(|chunk| -> Result<(usize, usize, u64, u64)> {
            let gpu = AirborneGpu::new(&class_weights);
            let mut cache = R4SourceCache::new(&args.h3r4_dir, args.r4_cache.max(7), SEL);
            let (mut w, mut s) = (0usize, 0usize);
            for &r4 in chunk {
                let (rw, rs) = process_region_gpu(
                    &gpu,
                    &mut cache,
                    &rasters,
                    &args,
                    z,
                    bn,
                    n_days,
                    r4,
                    &regions[&r4],
                )?;
                w += rw;
                s += rs;
            }
            let (h, m) = cache.stats();
            Ok((w, s, h, m))
        })
        .try_reduce(
            || (0, 0, 0, 0),
            |(a, b, c, d), (e, f, g, h)| Ok((a + e, b + f, c + g, d + h)),
        )?;
    let hit_pct = 100.0 * hits as f64 / (hits + misses).max(1) as f64;
    eprintln!(
        "done: {written} tiles written, {skipped} skipped, {} region(s), {:.1} s | \
         cache {hits} hits / {misses} misses ({hit_pct:.0}% hit, {n_workers} workers)",
        regions.len(),
        t.elapsed().as_secs_f64(),
    );
    Ok(())
}
