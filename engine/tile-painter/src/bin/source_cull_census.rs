//! How many of a region's line rows can actually reach one tile? DIAGNOSTIC ONLY.
//!
//! Sizes the per-tile source cull before any ABI or kernel work is spent on it. The GPU
//! uploads a region's sources ONCE and every block of every tile walks all of them
//! (`gpu_surface.rs`: `nsrc = region_rows…len()`), rejecting each in-kernel; the CPU builder
//! already culls per tile (`scatter_line.rs:165-181`). The 2026-08-14 census measured 1,389
//! real candidates out of 49,173 region rows on the dense Dobris rail tile — this recomputes
//! that on today's data with the bound the GPU port would actually use.
//!
//! The bound is the CPU's reach box grown by the kernel's per-block slack, so it is a strict
//! superset of `keep = (de <= sp[s*12+1] + block_reach_ub)` for every block of the tile.
//!
//! Loads with `Admin::UNKNOWN`: admin only feeds the C1 period-split fallback for rows whose
//! batch carries no baked triplet, and the number this tool reports is geometric — a row is
//! kept or not by its own solved reach against the tile box, so the census is robust to it.
//!
//! Usage: source_cull_census <h3r4-dir> <r4-hex> <zoom> <x> <y>

use anyhow::{Context, Result};
use h3o::CellIndex;
use noise_compute::admin::Admin;
use noise_compute::propagation::geo::reach_box_half_extents_deg;
use raster_reader::fused_tile_z13::{TileBbox, TILE_PX};
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_rail::RailData;

/// The kernel's cull slack for ONE 16×16-pixel block, at cos = 1 so it bounds every
/// latitude — mirrors `block_reach_ub` in `scatter.cu:722-725`.
fn block_slack_m(bbox: &TileBbox) -> f64 {
    const BIN_W: f64 = 16.0;
    const M_LAT: f64 = 110_540.0;
    const M_LON_EQ: f64 = 111_320.0;
    let frac = BIN_W / TILE_PX as f64;
    let dlat_half_m = 0.5 * (bbox.north_lat - bbox.south_lat).abs() * frac * M_LAT;
    let dlon_half_m = 0.5 * (bbox.east_lon - bbox.west_lon).abs() * frac * M_LON_EQ;
    (dlat_half_m * dlat_half_m + dlon_half_m * dlon_half_m).sqrt() + 1.0
}

/// Can this row reach any pixel of the tile, for any block? Superset by construction.
fn reaches_tile(row: &LineRow, bbox: &TileBbox, slack_m: f64) -> bool {
    let reach = row.max_distance_m + slack_m;
    let s_lat = row.start_lat.min(row.end_lat);
    let n_lat = row.start_lat.max(row.end_lat);
    let w_lon = row.start_lon.min(row.end_lon);
    let e_lon = row.start_lon.max(row.end_lon);
    let (d_lat, d_lon) = reach_box_half_extents_deg(n_lat.abs().max(s_lat.abs()), reach);
    !(s_lat - d_lat > bbox.north_lat
        || n_lat + d_lat < bbox.south_lat
        || w_lon - d_lon > bbox.east_lon
        || e_lon + d_lon < bbox.west_lon)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 {
        eprintln!("usage: source_cull_census <h3r4-dir> <r4-hex> <zoom> <x> <y>");
        std::process::exit(64);
    }
    let h3r4 = std::path::PathBuf::from(&a[0]);
    let r4 = u64::from_str_radix(&a[1], 16).context("r4 hex")?;
    let (zoom, x, y): (u8, u32, u32) = (a[2].parse()?, a[3].parse()?, a[4].parse()?);

    let cell = CellIndex::try_from(r4).map_err(|e| anyhow::anyhow!("bad r4: {e}"))?;
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(Into::into)
        .collect();
    let rows = RailData::load_for_r4s(&h3r4, &ring, Admin::UNKNOWN)?.into_rows();

    let bbox = TileBbox::from_xyz(zoom, x, y);
    let slack = block_slack_m(&bbox);
    let kept = rows
        .iter()
        .filter(|r| reaches_tile(r, &bbox, slack))
        .count();

    println!("region rows        {}", rows.len());
    println!("block slack        {slack:.1} m");
    println!("kept for this tile {kept}");
    println!(
        "retention          {:.3} %   reduction {:.1}x",
        100.0 * kept as f64 / rows.len().max(1) as f64,
        rows.len() as f64 / kept.max(1) as f64,
    );
    Ok(())
}
