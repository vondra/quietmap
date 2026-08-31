//! Finalize: read spill buckets → group by hex_id → write Arrow IPC per hex directory.

use anyhow::Result;
use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::ipc::writer::FileWriter;
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::poi_join::{JoinStats, PoiIndex};

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

/// Per-file schema contract (settlement v2 phase 2). Bumped on `buildings.arrow`
/// because the v2 extract adds an `opening_hours` column and re-numbers
/// `building_type` (new SILENT/HOUSE/FOOD_RETAIL/HOSPITALITY classes); a v1
/// reader would mis-profile every building. `leisure.arrow` ships at its own v1.
/// Stamped into the arrow metadata; consumers fail loud on a mismatch
/// (Convention-B per-file contract, no global SCHEMA_VERSION bump).
pub const BUILDINGS_CONTRACT_V2: &str = "buildings_v2";
pub const LEISURE_CONTRACT_V1: &str = "leisure_v1";

/// Read all spill buckets and write final per-hex Arrow IPC files.
/// Returns number of distinct hex directories created.
///
/// Each `(source, bucket)` pair is an independent unit of work. `spill.rs` assigns
/// every feature to `bucket = (hex_id >> 28) % num_buckets`, a pure function of
/// `hex_id`, so a given hex lands in exactly one bucket — two units never write the
/// same `{source}.arrow` path. The units therefore run across the rayon pool instead
/// of one core (the per-row Arrow encode was the single-thread bottleneck). The only
/// shared filesystem touch is `create_dir_all` when two sources hit the same hex dir,
/// which tolerates the concurrent-create race.
pub fn finalize(spill_dir: &Path, output_dir: &Path, num_buckets: usize) -> Result<usize> {
    // `poi` is intentionally NOT a final source — it is the footprint-join
    // input consumed when finalizing `buildings` (settlement v2 phase 2).
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

    // Shared across the parallel building buckets (settlement v2 phase 2).
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

    // Distinct hexes across all sources — a hex with both roads and buildings appears
    // in two bucket sets, so union before counting (matches the old shared-set count).
    let hex_dirs: HashSet<String> = dir_sets.into_iter().flatten().collect();
    Ok(hex_dirs.len())
}

/// Sort one spill bucket by hex_id, group consecutive rows, and write one
/// `{source}.arrow` per hex directory. Returns the hex directories it created.
///
/// `sort` groups by hex_id on disk; its stdout streams straight into the encoder —
/// no intermediate `*_sorted.tsv`, so peak scratch stays at the spill size (not +1
/// sorted copy per concurrent unit, ~184 GB for planet roads) and we skip a
/// write+read round-trip. `-S` bounds per-sort RAM and `--parallel=1` keeps each
/// sort single-threaded because up to ~ncpu of these run at once (parallelism is
/// across buckets, not within one sort); the sort is not the bottleneck (the Arrow
/// encode is). A nonzero sort exit (ENOSPC, OOM-kill) aborts finalize — it must
/// never silently drop a whole bucket of features.
fn finalize_bucket(
    source: &str,
    bucket: usize,
    spill_dir: &Path,
    output_dir: &Path,
    join_stats: &JoinStats,
) -> Result<HashSet<String>> {
    let mut hex_dirs = HashSet::new();
    let path = spill_dir.join(format!("{source}_{bucket:03}.tsv"));

    // Settlement v2 phase 2: the buildings unit consumes its bucket's POI nodes
    // for the footprint join. A POI inside a building shares its R4 hex → its
    // bucket, so the join is fully local to this unit (no cross-bucket lookup).
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
    let mut current_hex: u64 = 0;
    let mut current_rows: Vec<Vec<String>> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let fields: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
        if fields.is_empty() {
            continue;
        }
        let hex_id: u64 = fields[0].parse().unwrap_or(0);

        if hex_id != current_hex && !current_rows.is_empty() {
            hex_dirs.insert(flush_hex(
                source,
                current_hex,
                &current_rows,
                output_dir,
                &poi_index,
                join_stats,
            )?);
            current_rows.clear();
        }
        current_hex = hex_id;
        current_rows.push(fields);
    }
    if !current_rows.is_empty() {
        hex_dirs.insert(flush_hex(
            source,
            current_hex,
            &current_rows,
            output_dir,
            &poi_index,
            join_stats,
        )?);
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("sort failed for {source} bucket {bucket} ({status})");
    }

    eprintln!("    {source} [{bucket:03}]: {} hexes", hex_dirs.len());
    Ok(hex_dirs)
}

/// Load `poi_<bucket>.tsv` (unsorted is fine — `PoiIndex` keys by hex). Absent
/// when a region has no function POIs → an empty index (the join is a no-op).
fn load_poi_bucket(spill_dir: &Path, bucket: usize) -> Result<PoiIndex> {
    let path = spill_dir.join(format!("poi_{bucket:03}.tsv"));
    if !path.exists() {
        return Ok(PoiIndex::default());
    }
    let reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
    Ok(PoiIndex::from_lines(reader.lines().map_while(Result::ok)))
}

/// Write one hex's accumulated rows to `{source}.arrow`; returns the hex dir name.
fn flush_hex(
    source: &str,
    hex: u64,
    rows: &[Vec<String>],
    output_dir: &Path,
    poi_index: &PoiIndex,
    join_stats: &JoinStats,
) -> Result<String> {
    let hex_str = format!("{hex:015x}");
    let dir = output_dir.join(&hex_str);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{source}.arrow"));
    match source {
        "roads" => write_roads(rows, &path),
        "railways" => write_railways(rows, &path),
        "airport_areas" => write_airport_areas(rows, &path),
        "airport_lines" => write_airport_lines(rows, &path),
        "buildings" => write_buildings(rows, &path, hex, poi_index, join_stats),
        "industrial" => write_industrial(rows, &path),
        "barriers" => write_barriers(rows, &path),
        "leisure" => write_leisure(rows, &path),
        _ => Ok(()),
    }?;
    Ok(hex_str)
}

/// Stamp a per-file contract into an arrow's schema metadata (Convention-B).
fn schema_with_contract(fields: Vec<Field>, key: &str, value: &str) -> Schema {
    let mut md = HashMap::new();
    md.insert(key.to_string(), value.to_string());
    Schema::new(fields).with_metadata(md)
}

/// The ONE arrow-write path for all 8 layer writers: rows are spatially
/// sorted, chunked into record batches, and per-batch bboxes stamped into
/// schema metadata so the popup reader can skip out-of-reach batches without
/// decoding them. `row_bboxes` MUST be
/// parallel to the APPENDED rows — a writer that `continue`s a malformed TSV
/// row pushes no bbox for it.
pub(super) fn write_arrow_spatially_batched(
    path: &Path,
    schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[arrow_batching::RowBbox],
) -> Result<()> {
    let (schema, batches) = arrow_batching::spatially_batched(schema, columns, row_bboxes)?;
    // Sibling-temp + rename, mirroring aircraft-extract's write_record_batches:
    // a re-extract runs while Fastify serves the OLD files, and the popup's
    // lazy reader keeps mmaps open long past load time — truncating the live
    // inode via File::create(path) would SIGBUS a later first-touch decode
    // (Codex /gg 2026-07-10). rename() keeps the old inode alive for open
    // maps and swaps readers to the new file atomically.
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

/// Row envelope of a ≤250 m microsegment (roads/railways/barriers/airport
/// lines): the segment's endpoint box.
pub(super) fn segment_row_bbox(
    s_lat: f64,
    s_lon: f64,
    e_lat: f64,
    e_lon: f64,
) -> arrow_batching::RowBbox {
    [
        s_lat.min(e_lat),
        s_lon.min(e_lon),
        s_lat.max(e_lat),
        s_lon.max(e_lon),
    ]
}

/// Row envelope of a polygon-or-point feature: the exact WKB footprint bbox
/// when present — centroid-only would under-prune large areas (an audible
/// plant edge can sit kilometres from its centroid) — else a degenerate
/// point box at the centroid.
pub(super) fn polygon_row_bbox(
    wkb_hex: &str,
    centroid_lat: f64,
    centroid_lon: f64,
) -> arrow_batching::RowBbox {
    if !wkb_hex.is_empty() {
        if let Some(fp) = noise_compute::wkb::WkbFootprint::parse(wkb_hex) {
            let (min_lat, max_lat, min_lon, max_lon) = fp.bbox();
            return [min_lat, min_lon, max_lat, max_lon];
        }
    }
    [centroid_lat, centroid_lon, centroid_lat, centroid_lon]
}
