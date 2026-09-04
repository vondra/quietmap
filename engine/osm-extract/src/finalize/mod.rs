//! Finalize: read spill buckets → group by square → write Arrow IPC per square directory.
//!
//! Spill TSV layouts (first column is always the numeric square id `y*512+x`,
//! coordinates are snapped z30 grid cells, rings are `gx,gy;…` text):
//! - segments (roads/railways/barriers/airport_lines):
//!   `sq osm_id seg_idx s_gx s_gy e_gx e_gy length_m …type cols`
//! - buildings: `sq osm_id c_gx c_gy btype buse height floors name street
//!   housenumber opening_hours area_source ring`
//! - industrial: `sq osm_id c_gx c_gy stype subtype name hub_h power ring`
//! - leisure: `sq osm_id c_gx c_gy sport opening_hours name ring`
//! - airport_areas: `sq osm_id c_gx c_gy atype name ref icao iata operator
//!   surface width_m aerodrome_type access ring`
//! - poi: `sq gx gy class` (join input only, never a final arrow)

use anyhow::Result;
use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::ipc::writer::FileWriter;
use grid::grid_to_meters;
use grid::poly::{meters_to_lonlat, ring_bbox_lonlat};
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::poi_join::{JoinStats, PoiIndex};
use crate::spill::{parse_ring_text, square_from_spill_key};

mod write_airport;
mod write_barriers;
mod write_buildings;
mod write_industrial;
mod write_leisure;
mod write_railways;
mod write_roads;

use write_airport::{write_airport_areas, write_airport_lines};
use write_barriers::write_barriers;
use write_buildings::write_buildings;
use write_industrial::write_industrial;
use write_leisure::write_leisure;
use write_railways::write_railways;
use write_roads::write_roads;

/// Per-file schema contracts. Bumped from the hex era (`buildings_v2`,
/// `leisure_v1`): integer grid columns, `geom` binaries, no leisure capacity.
/// Stamped into arrow metadata; consumers fail loud on a mismatch.
pub const BUILDINGS_CONTRACT_V3: &str = "buildings_v3";
pub const LEISURE_CONTRACT_V2: &str = "leisure_v2";
/// Schema metadata key pinning the coordinate grid of every file.
pub const GRID_CONTRACT_KEY: &str = "grid";
pub const GRID_CONTRACT_Z30: &str = "z30";

/// Read all spill buckets and write final per-square Arrow IPC files.
/// Returns number of distinct square directories created.
///
/// Each `(source, bucket)` pair is an independent unit of work: `bucket` is a
/// pure function of the square id, so a given square lands in exactly one
/// bucket — two units never write the same `{source}.arrow` path.
pub fn finalize(spill_dir: &Path, output_dir: &Path, num_buckets: usize) -> Result<usize> {
    // `poi` is intentionally NOT a final source — it is the footprint-join
    // input consumed when finalizing `buildings`.
    const SOURCES: [&str; 8] = [
        "roads",
        "railways",
        "airport_areas",
        "airport_lines",
        "buildings",
        "industrial",
        "barriers",
        "leisure",
    ];

    let units: Vec<(&str, usize)> = SOURCES
        .iter()
        .flat_map(|s| (0..num_buckets).map(move |b| (*s, b)))
        .filter(|(s, b)| spill_dir.join(format!("{}_{:03}.tsv", s, b)).exists())
        .collect();
    eprintln!(
        "  Finalizing {} spill buckets across rayon pool...",
        units.len()
    );

    // Shared across the parallel building buckets.
    let join_stats = JoinStats::default();
    let dir_sets = units
        .par_iter()
        .map(|(source, bucket)| {
            finalize_bucket(source, *bucket, spill_dir, output_dir, &join_stats)
        })
        .collect::<Result<Vec<HashSet<String>>>>()?;

    let (checked, reclassified) = join_stats.report();
    if checked > 0 {
        eprintln!(
            "  POI footprint join: {reclassified}/{checked} `building=yes` reclassified \
             ({:.1}%)",
            reclassified as f64 / checked as f64 * 100.0
        );
    }

    let square_dirs: HashSet<String> = dir_sets.into_iter().flatten().collect();
    Ok(square_dirs.len())
}

/// Sort one spill bucket by square id, group consecutive rows, and write one
/// `{source}.arrow` per square directory. Returns the square directories it created.
///
/// `sort` groups on disk; its stdout streams straight into the encoder — no
/// intermediate sorted copy, so peak scratch stays at the spill size. `-S`
/// bounds per-sort RAM and `--parallel=1` keeps each sort single-threaded
/// (parallelism is across buckets). A nonzero sort exit aborts finalize.
fn finalize_bucket(
    source: &str,
    bucket: usize,
    spill_dir: &Path,
    output_dir: &Path,
    join_stats: &JoinStats,
) -> Result<HashSet<String>> {
    let mut square_dirs = HashSet::new();
    let path = spill_dir.join(format!("{source}_{bucket:03}.tsv"));

    // The buildings unit consumes its bucket's POI nodes for the footprint
    // join. A POI inside a building shares its square → its bucket, so the
    // join is fully local to this unit (no cross-bucket lookup).
    let poi_index = if source == "buildings" {
        load_poi_bucket(spill_dir, bucket)?
    } else {
        PoiIndex::default()
    };

    let mut child = std::process::Command::new("sort")
        .args([
            "-t\t",
            "-k1,1n",
            "-T",
            spill_dir.to_str().unwrap_or("/tmp"),
            "-S",
            "1G",
            "--parallel=1",
        ])
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let reader = BufReader::with_capacity(1 << 20, child.stdout.take().expect("sort stdout piped"));
    let mut current_id: Option<u32> = None;
    let mut current_rows: Vec<Vec<String>> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let fields: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
        if fields.is_empty() {
            continue;
        }
        let sq_id: u32 = fields[0].parse().unwrap_or(u32::MAX);

        if Some(sq_id) != current_id && !current_rows.is_empty() {
            if let Some(id) = current_id {
                square_dirs.insert(flush_square(
                    source,
                    id,
                    &current_rows,
                    output_dir,
                    &poi_index,
                    join_stats,
                )?);
            }
            current_rows.clear();
        }
        current_id = Some(sq_id);
        current_rows.push(fields);
    }
    if !current_rows.is_empty() {
        if let Some(id) = current_id {
            square_dirs.insert(flush_square(
                source,
                id,
                &current_rows,
                output_dir,
                &poi_index,
                join_stats,
            )?);
        }
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("sort failed for {source} bucket {bucket} ({status})");
    }

    eprintln!("    {source} [{bucket:03}]: {} squares", square_dirs.len());
    Ok(square_dirs)
}

/// Load `poi_<bucket>.tsv` (unsorted is fine — `PoiIndex` keys by square).
/// Absent when a region has no function POIs → an empty index (no-op join).
fn load_poi_bucket(spill_dir: &Path, bucket: usize) -> Result<PoiIndex> {
    let path = spill_dir.join(format!("poi_{bucket:03}.tsv"));
    if !path.exists() {
        return Ok(PoiIndex::default());
    }
    let reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
    Ok(PoiIndex::from_lines(reader.lines().map_while(Result::ok)))
}

/// Write one square's accumulated rows to `{source}.arrow` under
/// `z9/<x>/<y>/`; returns the square dir name. A stale-spill id outside the
/// z9 range is skipped loud (never misfiled).
fn flush_square(
    source: &str,
    spill_key: u32,
    rows: &[Vec<String>],
    output_dir: &Path,
    poi_index: &PoiIndex,
    join_stats: &JoinStats,
) -> Result<String> {
    let Some(square) = square_from_spill_key(spill_key) else {
        anyhow::bail!("spill key {spill_key} outside the z9 range");
    };
    let name = grid::square_name(square);
    let dir = output_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string());
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{source}.arrow"));
    match source {
        "roads" => write_roads(rows, &path),
        "railways" => write_railways(rows, &path),
        "airport_areas" => write_airport_areas(rows, &path),
        "airport_lines" => write_airport_lines(rows, &path),
        "buildings" => write_buildings(rows, &path, square, poi_index, join_stats),
        "industrial" => write_industrial(rows, &path),
        "barriers" => write_barriers(rows, &path),
        "leisure" => write_leisure(rows, &path),
        _ => Ok(()),
    }?;
    Ok(name)
}

/// Stamp a per-file contract into an arrow's schema metadata.
fn schema_with_contract(fields: Vec<Field>, key: &str, value: &str) -> Schema {
    let mut md = HashMap::new();
    md.insert(key.to_string(), value.to_string());
    Schema::new(fields).with_metadata(md)
}

/// Parse a grid int column, 0 on garbage (a malformed TSV row then lands on a
/// real cell — writers only call this after a `row.len()` gate, so garbage
/// here is a corrupt spill, and the value still round-trips for inspection).
fn parse_grid_cell(s: &str) -> i32 {
    s.parse().unwrap_or(0)
}

/// The ONE arrow-write path for all 8 layer writers: rows are spatially
/// sorted, chunked into record batches, and per-batch bboxes stamped into
/// schema metadata so the popup reader can skip out-of-reach batches without
/// decoding them. Every file also carries the `grid=z30` coordinate pin —
/// a reader that does not know integer grids refuses the file instead of
/// misreading it.
pub(super) fn write_arrow_spatially_batched(
    path: &Path,
    schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[arrow_batching::RowBbox],
) -> Result<()> {
    let mut md = schema.metadata().clone();
    md.insert(GRID_CONTRACT_KEY.to_string(), GRID_CONTRACT_Z30.to_string());
    let schema = Schema::new_with_metadata(schema.fields().clone(), md);
    let (schema, batches) = arrow_batching::spatially_batched(schema, columns, row_bboxes)?;
    // Sibling-temp + rename: a re-extract runs while the server reads the OLD
    // files, and the popup's lazy reader keeps mmaps open long past load —
    // truncating the live inode would SIGBUS a later first-touch decode.
    // rename() keeps the old inode alive and swaps readers atomically.
    let tmp_path = path.with_extension("arrow.tmp");
    let file = File::create(&tmp_path)?;
    let mut writer = FileWriter::try_new(file, &schema)?;
    for batch in &batches {
        writer.write(batch)?;
    }
    writer.finish()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Row envelope of a microsegment from snapped endpoints, as lon/lat degrees
/// for the batch-prune metadata.
pub(super) fn segment_row_bbox(
    s_gx: i32,
    s_gy: i32,
    e_gx: i32,
    e_gy: i32,
) -> arrow_batching::RowBbox {
    let (sx, sy) = grid_to_meters(s_gx, s_gy);
    let (ex, ey) = grid_to_meters(e_gx, e_gy);
    let (s_lon, s_lat) = meters_to_lonlat(sx, sy);
    let (e_lon, e_lat) = meters_to_lonlat(ex, ey);
    [
        s_lat.min(e_lat),
        s_lon.min(e_lon),
        s_lat.max(e_lat),
        s_lon.max(e_lon),
    ]
}

/// Row envelope of a polygon-or-point feature: the snapped ring's bbox when
/// present — centroid-only would under-prune large areas — else a degenerate
/// point box at the centroid grid cell.
pub(super) fn polygon_row_bbox(
    ring: Option<&[(i32, i32)]>,
    c_gx: i32,
    c_gy: i32,
) -> arrow_batching::RowBbox {
    if let Some(ring) = ring {
        if let Some(bb) = ring_bbox_lonlat(ring) {
            return bb;
        }
    }
    let (cx, cy) = grid_to_meters(c_gx, c_gy);
    let (c_lon, c_lat) = meters_to_lonlat(cx, cy);
    [c_lat, c_lon, c_lat, c_lon]
}

/// Decode a TSV ring column for writers. `None` = absent or malformed (the
/// writer then stores a null geometry + centroid bbox, same as a point).
pub(super) fn decode_tsv_ring(s: &str) -> Option<Vec<(i32, i32)>> {
    parse_ring_text(s)
}

#[cfg(test)]
mod tests;
