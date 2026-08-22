//! Pyramid-downsample an HM3 layer's tile STORE from a base zoom down.
//!
//! Usage (z12 base — the 512@z12 world):
//!
//!   build-pyramid --store-dir data/tiles/2026/store/road \
//!     --base-zoom 12 --dst-zoom 2
//!
//! Each level reads four `2×2` source tiles from the `z{N}` store pair,
//! energy-mean-pools each `2×2` block of source pixels into one destination
//! pixel, writes one destination tile (working codec). Empty tiles (all
//! silence sentinel) are tombstoned.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;

use tile_painter::pyramid::{build_pyramid, scope_from_args};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    store_dir: PathBuf,
    /// Base (source) Mercator zoom — the finest level in the store (z12 for current
    /// 512-pixel tiles).
    #[arg(long, default_value_t = 12)]
    base_zoom: u8,
    /// Lowest pyramid level to generate (inclusive). Defaults to z=2 — the
    /// serving floor for EVERY layer (frontend MIN_ZOOM): a single-layer view
    /// at world zoom is a first-class use case, and deck's TileLayer renders
    /// nothing below its minZoom (owner report 2026-07-09).
    #[arg(long, default_value_t = 2)]
    dst_zoom: u8,
    /// `south,west,north,east` — rebuild only the ancestors of the tiles this
    /// bbox covers. Mutually exclusive with `--tiles-from`. Omit both for a
    /// full rebuild.
    #[arg(long, value_parser = parse_bbox)]
    bbox: Option<[f64; 4]>,
    /// Path to a file of exact `x y` base-zoom tile pairs (one per line) —
    /// only their ancestors are re-rolled, one fold per changed child, never
    /// a whole bbox's worth regardless of how the tiles are scattered. This
    /// binary only ever needs one file (one layer at a time); repeatable for
    /// CLI-shape consistency with build-heatmap-combine's own --tiles-from.
    /// Mutually exclusive with `--bbox`. See pyramid::RebuildScope.
    #[arg(long)]
    tiles_from: Vec<PathBuf>,
}

fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.parse())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bbox: {e}"))?;
    if v.len() != 4 {
        return Err("expected south,west,north,east".into());
    }
    Ok([v[0], v[1], v[2], v[3]])
}

fn main() -> Result<()> {
    let args = Args::parse();
    let scope = scope_from_args(args.bbox, &args.tiles_from)?;
    eprintln!(
        "pyramid: {} → {} in {}",
        args.base_zoom,
        args.dst_zoom,
        args.store_dir.display()
    );
    let t = Instant::now();
    // The written levels' HM3 source_id derives from the source store's own
    // header — no flag to get wrong (project rule: no CLI for derivable values).
    let written = build_pyramid(&args.store_dir, args.base_zoom, args.dst_zoom, scope)?;
    eprintln!(
        "done: {} tiles written in {:.1} s",
        written,
        t.elapsed().as_secs_f64()
    );
    Ok(())
}
