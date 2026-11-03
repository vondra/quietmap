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
use arrow_array::ArrayRef;
use arrow_ipc::writer::FileWriter;
use arrow_schema::{Field, Schema};
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::osm::poi_join::{JoinStats, PoiIndex};
use crate::osm::spill::{parse_ring_text, square_from_spill_key};
use crate::osm::transport::{NodeAliases, load_node_aliases};

mod evidence;
mod write_airport;
mod write_barriers;
mod write_buildings;
mod write_industrial;
mod write_leisure;
pub mod write_railway_globals;
mod write_railways;
mod write_roads;
pub mod write_source_pieces;
mod write_transport_nodes;

use write_airport::{write_airport_areas, write_airport_lines};
use write_barriers::write_barriers;
use write_buildings::write_buildings;
use write_industrial::write_industrial;
use write_leisure::write_leisure;
use write_railway_globals::{write_railway_ways, write_train_routes};
use write_railways::write_railways;
use write_roads::write_roads;
use write_source_pieces::write_source_pieces;

/// Current building contract: z30 geometry, emission-only `area_source`, and
/// `building_use` 3 for explicitly open carports (never an indoor envelope).
/// Stamped into Arrow metadata; consumers reject older classification semantics.
pub const BUILDINGS_CONTRACT_V5: &str = "buildings_v5";
/// Schema metadata key pinning the coordinate grid of every file.
pub const GRID_CONTRACT_KEY: &str = "grid";
pub const GRID_CONTRACT_Z30: &str = "z30";

/// Read all spill buckets and write final per-square Arrow IPC files.
/// Returns number of distinct square directories created.
///
/// Each `(source, bucket)` pair is an independent unit of work: `bucket` is a
/// pure function of the square id, so a given square lands in exactly one
/// bucket — two units never write the same `{source}.arrow` path. The roads and
/// railways units also write the square's `{source}.pieces.arrow`; the two railway
/// globals beside the year are built by one more task on the same pool.
pub fn finalize(
    spill_dir: &Path,
    output_dir: &Path,
    num_buckets: usize,
    squares: &[crate::dev4::Square],
) -> Result<usize> {
    // `poi` is intentionally NOT a final source — it is the footprint-join
    // input consumed when finalizing `buildings`.
    const SOURCES: [&str; 9] = [
        "transport_nodes",
        "roads",
        "railways",
        "airport_areas",
        "airport_lines",
        "buildings",
        "industrial",
        "barriers",
        "leisure",
    ];

    let selected: HashSet<u32> = squares
        .iter()
        .map(|square| square.y * 512 + square.x)
        .collect();
    let selected_buckets: HashSet<_> = selected
        .iter()
        .map(|square| *square as usize % num_buckets)
        .collect();
    let units: Vec<(&str, usize)> = SOURCES
        .iter()
        .flat_map(|s| (0..num_buckets).map(move |b| (*s, b)))
        .filter(|(s, b)| {
            (selected.is_empty() || selected_buckets.contains(b))
                && spill_dir.join(format!("{}_{:03}.tsv", s, b)).exists()
        })
        .collect();
    eprintln!(
        "  Finalizing {} spill buckets across rayon pool...",
        units.len()
    );

    let aliases = load_node_aliases(spill_dir)?;
    let no_aliases = HashMap::new();
    let railway_aliases = aliases.get("railways").unwrap_or(&no_aliases);
    // Shared across the parallel building buckets.
    let join_stats = JoinStats::default();
    let (dir_sets, railway_globals) = rayon::join(
        || {
            units
                .par_iter()
                .map(|(source, bucket)| {
                    finalize_bucket(
                        source,
                        *bucket,
                        spill_dir,
                        output_dir,
                        &aliases,
                        &join_stats,
                        &selected,
                    )
                })
                .collect::<Result<Vec<HashSet<String>>>>()
        },
        || {
            write_railway_ways(spill_dir, output_dir, railway_aliases)?;
            write_train_routes(spill_dir, output_dir)
        },
    );
    let dir_sets = dir_sets?;
    railway_globals?;

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

/// Write one square's accumulated rows to `{source}.arrow` under
/// `z9/<x>/<y>/`; returns the square dir name. A stale-spill id outside the
/// z9 range is skipped loud (never misfiled).
fn flush_square(
    source: &str,
    spill_key: u32,
    rows: &[Vec<String>],
    output_dir: &Path,
    aliases: &NodeAliases,
    poi_index: &PoiIndex,
    join_stats: &JoinStats,
) -> Result<String> {
    let Some(square) = square_from_spill_key(spill_key) else {
        anyhow::bail!("spill key {spill_key} outside the z9 range");
    };
    let name = crate::osm::geometry::square_name(square);
    let dir = output_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string());
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{source}.arrow"));
    if matches!(source, "roads" | "railways") {
        let no_aliases = HashMap::new();
        write_source_pieces(
            source,
            rows,
            aliases.get(source).unwrap_or(&no_aliases),
            &dir.join(format!("{source}.pieces.arrow")),
        )?;
    }
    match source {
        "transport_nodes" => write_transport_nodes::write_transport_nodes(rows, &path),
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

/// Preserve row values and pin the coordinate grid; runtime block indexes are not inputs.
pub(super) fn write_arrow(path: &Path, schema: Schema, columns: Vec<ArrayRef>) -> Result<()> {
    let mut md = schema.metadata().clone();
    md.insert(GRID_CONTRACT_KEY.to_string(), GRID_CONTRACT_Z30.to_string());
    let schema = Schema::new_with_metadata(schema.fields().clone(), md);
    let batch = arrow_array::RecordBatch::try_new(std::sync::Arc::new(schema), columns)?;
    write_single_batch_arrow(path, batch)
}

/// A plain one-batch Arrow file through a sibling temp and rename, never an append.
pub(crate) fn write_single_batch_arrow(path: &Path, batch: arrow_array::RecordBatch) -> Result<()> {
    let tmp_path = path.with_extension("arrow.tmp");
    let file = File::create(&tmp_path)?;
    let mut writer = FileWriter::try_new(file, &batch.schema())?;
    writer.write(&batch)?;
    writer.finish()?;
    writer.get_ref().sync_all()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Decode a TSV ring column for writers. `None` = absent or malformed (the
/// writer then stores a null geometry + centroid bbox, same as a point).
pub(super) fn decode_tsv_ring(s: &str) -> Option<Vec<(i32, i32)>> {
    parse_ring_text(s)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod evidence_tests;

mod bucket;
use bucket::finalize_bucket;
