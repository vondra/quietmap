//! Yard tracks lose the line prior: an untagged, non-through track inside a
//! `railway=yard` polygon is yard infrastructure, and the
//! yard polygon itself emits as a rail-yard facility (industrial source_type
//! 5). Stamping `service = yard` here routes those rows through the existing
//! service rules (no class prior, own evidence kept, excluded from other
//! tracks' cross-sections).
//!
//! A track is through (keeps its prior) when it is tagged (ref/name),
//! main-tagged (usage 0), timetable-covered (any non-crossing interval on its
//! key), or its way extends outside every yard polygon in this square. The
//! stamp runs on the merged pre-finalize batch so traffic and the written
//! `service` column agree. Squares without `industrial.arrow`, without yard
//! rows, or without stamp candidates pass through unchanged.

use crate::osm::geometry::poly::PreparedRing;
use crate::sources::cells::{Z30Ring as GridRing, decode_z30_ring as decode_grid_poly};
use arrow_array::{Array, ArrayRef, BinaryArray, RecordBatch, StringArray, UInt8Array};
use arrow_select::concat::concat_batches;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::square_intervals::Interval;
use super::write::{col_i16, col_i32, col_i64, col_u8};

/// Yard facility marker: industrial `source_type` for rail yards.
pub const YARD_SOURCE_TYPE: u8 = 5;

/// OSM rail `service=yard` value.
const SERVICE_YARD: u8 = 1;

/// Decide which rows of `merged` are untagged non-through yard tracks in `dir`.
/// Returns `Ok(None)` when no row in the square is stamped.
pub(crate) fn yard_stamp_flags(
    merged: &RecordBatch,
    dir: &Path,
    has_direct_claim: impl Fn(i64, i16) -> bool,
) -> Result<Option<Vec<bool>>, String> {
    let yards = load_yard_rings(dir)?;
    if yards.is_empty() || merged.num_rows() == 0 {
        return Ok(None);
    }
    yard_stamp_flags_with_rings(merged, &yards, has_direct_claim)
}

fn yard_stamp_flags_with_rings(
    merged: &RecordBatch,
    yards: &[GridRing],
    has_direct_claim: impl Fn(i64, i16) -> bool,
) -> Result<Option<Vec<bool>>, String> {
    let start_gx = col_i32(merged, "start_gx")?;
    let start_gy = col_i32(merged, "start_gy")?;
    let end_gx = col_i32(merged, "end_gx")?;
    let end_gy = col_i32(merged, "end_gy")?;
    let osm_id = col_i64(merged, "osm_id")?;
    let segment_idx = col_i16(merged, "segment_idx")?;
    let usage = col_u8(merged, "usage")?;
    let service = col_u8(merged, "service")?;

    let prepared: Vec<PreparedRing> = yards
        .iter()
        .filter_map(|ring| PreparedRing::new(ring))
        .collect();
    let inside: Vec<bool> = (0..merged.num_rows())
        .map(|row| {
            let gx = start_gx.value(row) / 2 + end_gx.value(row) / 2;
            let gy = start_gy.value(row) / 2 + end_gy.value(row) / 2;
            prepared.iter().any(|ring| ring.contains(gx, gy))
        })
        .collect();
    if !inside.iter().any(|inside| *inside) {
        return Ok(None);
    }
    // Ways reaching outside every yard polygon are through lines.
    let mut way_outside: HashSet<i64> = HashSet::new();
    for (row, inside) in inside.iter().enumerate() {
        if !inside {
            way_outside.insert(osm_id.value(row));
        }
    }
    let mut stamp: Vec<bool> = vec![false; merged.num_rows()];
    for (row, inside) in inside.iter().enumerate() {
        if !inside
            || service.value(row) != 0
            || usage.value(row) == 0
            || way_outside.contains(&osm_id.value(row))
            || !corridor_at(merged, row).is_empty()
            || has_direct_claim(osm_id.value(row), segment_idx.value(row))
        {
            continue;
        }
        stamp[row] = true;
    }
    Ok(stamp.iter().any(|stamp| *stamp).then_some(stamp))
}

pub(crate) type YardEdgeKey = ([u16; 2], i64, i16);

/// Pre-compute stamped `(square, osm_id, segment_idx)` yard keys across every square in the
/// global crossing graph so yard tracks never enter crossing runs or parallel cross-sections.
pub(crate) fn yard_stamped_keys(
    geography: &Path,
    squares: &HashSet<[u16; 2]>,
    direct_claims: &HashSet<YardEdgeKey>,
) -> Result<HashSet<YardEdgeKey>, String> {
    let list: Vec<[u16; 2]> = squares.iter().copied().collect();
    let per_square: Result<Vec<Vec<YardEdgeKey>>, String> = list
        .into_par_iter()
        .map(|square| {
            let dir = geography.join(format!("z9/{}/{}", square[0], square[1]));
            let yards = load_yard_rings(&dir)?;
            if yards.is_empty() {
                return Ok(Vec::new());
            }
            let Some(table) = crate::dev4::read_table(&dir.join("railways.arrow"))? else {
                return Ok(Vec::new());
            };
            let Some(first) = table.batches.first() else {
                return Ok(Vec::new());
            };
            let merged = concat_batches(&first.schema(), &table.batches)
                .map_err(|err| format!("concat yard railways: {err}"))?;
            let Some(flags) = yard_stamp_flags_with_rings(&merged, &yards, |way, seg| {
                direct_claims.contains(&(square, way, seg))
            })?
            else {
                return Ok(Vec::new());
            };
            let osm_id = col_i64(&merged, "osm_id")?;
            let segment_idx = col_i16(&merged, "segment_idx")?;
            Ok(flags
                .into_iter()
                .enumerate()
                .filter_map(|(row, stamped)| {
                    stamped.then_some((square, osm_id.value(row), segment_idx.value(row)))
                })
                .collect())
        })
        .collect();
    Ok(per_square?.into_iter().flatten().collect())
}

/// Stamp yard tracks in `merged` and return the patched batch (or the input
/// unchanged when there is nothing to stamp).
pub fn stamp_yard_service(
    merged: &RecordBatch,
    dir: &Path,
    intervals: &HashMap<(i64, i16), Vec<Interval>>,
) -> Result<RecordBatch, String> {
    let Some(stamp) = yard_stamp_flags(merged, dir, |way, seg| {
        intervals.get(&(way, seg)).is_some_and(|list| {
            list.iter()
                .any(|interval| interval.resolved_section.is_none())
        })
    })?
    else {
        return Ok(merged.clone());
    };
    let service = col_u8(merged, "service")?;
    let patched: Vec<u8> = (0..merged.num_rows())
        .map(|row| {
            if stamp[row] {
                SERVICE_YARD
            } else {
                service.value(row)
            }
        })
        .collect();
    let service_idx = merged
        .schema()
        .column_with_name("service")
        .map(|(idx, _)| idx)
        .ok_or_else(|| "railways Arrow missing service".to_string())?;
    let mut columns: Vec<ArrayRef> = merged.columns().to_vec();
    columns[service_idx] = Arc::new(UInt8Array::from(patched));
    RecordBatch::try_new(merged.schema(), columns)
        .map_err(|err| format!("yard stamp rebuilds the service column: {err}"))
}

/// `ref`, else `name`, of one merged row (empty when the row is untagged).
fn corridor_at(batch: &RecordBatch, row: usize) -> String {
    for name in ["ref", "name"] {
        if let Some(col) = batch
            .column_by_name(name)
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .filter(|col| !col.is_null(row))
        {
            let value = col.value(row).trim().to_string();
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}

/// Exterior rings of the square's yard-facility rows. A missing
/// `industrial.arrow` (or one without yard rows) is no yards, not an error;
/// an unreadable file is.
fn load_yard_rings(dir: &Path) -> Result<Vec<GridRing>, String> {
    let path = dir.join("industrial.arrow");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("reads {}: {err}", path.display())),
    };
    let cursor = std::io::Cursor::new(bytes);
    let reader = arrow_ipc::reader::FileReader::try_new(cursor, None)
        .map_err(|err| format!("opens {}: {err}", path.display()))?;
    let mut rings = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|err| format!("reads {}: {err}", path.display()))?;
        let Some(source_type) = batch
            .column_by_name("source_type")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::UInt8Array>())
        else {
            continue;
        };
        let geom = batch
            .column_by_name("geom")
            .and_then(|col| col.as_any().downcast_ref::<BinaryArray>());
        for row in 0..batch.num_rows() {
            if source_type.value(row) != YARD_SOURCE_TYPE {
                continue;
            }
            let ring = geom
                .filter(|geom| !geom.is_null(row))
                .and_then(|geom| decode_grid_poly(geom.value(row)));
            if let Some(ring) = ring {
                rings.push(ring);
            }
        }
    }
    Ok(rings)
}
