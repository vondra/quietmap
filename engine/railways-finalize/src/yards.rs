//! Yard tracks lose the line prior: an untagged, non-through track inside a
//! `railway=yard` polygon is yard infrastructure, and the
//! yard polygon itself emits as a rail-yard facility (industrial source_type
//! 5). Stamping `service = yard` here routes those rows through the existing
//! service rules (no class prior, own evidence kept, excluded from other
//! tracks' cross-sections).
//!
//! A track is through (keeps its prior) when it is tagged (ref/name),
//! main-tagged (usage 0), timetable-covered (any interval on its key), or its
//! way extends outside every yard polygon in this square. The stamp runs on
//! the merged pre-finalize batch so traffic and the written `service` column
//! agree. Squares without `industrial.arrow`, without yard rows, or without
//! stamp candidates pass through unchanged.

use arrow::array::{Array, ArrayRef, BinaryArray, RecordBatch, StringArray, UInt8Array};
use grid::poly::{decode_grid_poly, ring_contains, GridRing};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::square_intervals::Interval;
use crate::write::{col_i16, col_i32, col_i64, col_u8};

/// Yard facility marker: industrial `source_type` for rail yards.
pub const YARD_SOURCE_TYPE: u8 = 5;

/// OSM rail `service=yard` value.
const SERVICE_YARD: u8 = 1;

/// Stamp yard tracks in `merged` and return the patched batch (or the input
/// unchanged when there is nothing to stamp).
pub fn stamp_yard_service(
    merged: &RecordBatch,
    dir: &Path,
    intervals: &HashMap<(i64, i16), Vec<Interval>>,
) -> Result<RecordBatch, String> {
    let yards = load_yard_rings(dir)?;
    if yards.is_empty() || merged.num_rows() == 0 {
        return Ok(merged.clone());
    }
    let start_gx = col_i32(merged, "start_gx")?;
    let start_gy = col_i32(merged, "start_gy")?;
    let end_gx = col_i32(merged, "end_gx")?;
    let end_gy = col_i32(merged, "end_gy")?;
    let osm_id = col_i64(merged, "osm_id")?;
    let segment_idx = col_i16(merged, "segment_idx")?;
    let usage = col_u8(merged, "usage")?;
    let service = col_u8(merged, "service")?;

    let inside: Vec<bool> = (0..merged.num_rows())
        .map(|row| {
            let gx = start_gx.value(row) / 2 + end_gx.value(row) / 2;
            let gy = start_gy.value(row) / 2 + end_gy.value(row) / 2;
            yards.iter().any(|ring| ring_contains(ring, gx, gy))
        })
        .collect();
    if !inside.iter().any(|inside| *inside) {
        return Ok(merged.clone());
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
            || intervals
                .get(&(osm_id.value(row), segment_idx.value(row)))
                .is_some_and(|list| !list.is_empty())
        {
            continue;
        }
        stamp[row] = true;
    }
    if !stamp.iter().any(|stamp| *stamp) {
        return Ok(merged.clone());
    }
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
    let reader = arrow::ipc::reader::FileReader::try_new(cursor, None)
        .map_err(|err| format!("opens {}: {err}", path.display()))?;
    let mut rings = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|err| format!("reads {}: {err}", path.display()))?;
        let Some(source_type) = batch
            .column_by_name("source_type")
            .and_then(|col| col.as_any().downcast_ref::<arrow::array::UInt8Array>())
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int16Array, Int32Array, Int64Array, StringArray, UInt8Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use grid::poly::encode_grid_poly;

    /// Merged-lookalike batch: (osm_id, seg_idx, mid_x, mid_y, usage, service, name).
    fn merged_batch(rows: &[(i64, i16, i32, i32, u8, u8, &str)]) -> RecordBatch {
        let schema = Schema::new(vec![
            Field::new("start_gx", DataType::Int32, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("osm_id", DataType::Int64, false),
            Field::new("segment_idx", DataType::Int16, false),
            Field::new("usage", DataType::UInt8, false),
            Field::new("service", DataType::UInt8, false),
            Field::new("ref", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int32Array::from(
                    rows.iter().map(|r| r.2 - 5).collect::<Vec<_>>(),
                )) as ArrayRef,
                Arc::new(Int32Array::from(
                    rows.iter().map(|r| r.3).collect::<Vec<_>>(),
                )),
                Arc::new(Int32Array::from(
                    rows.iter().map(|r| r.2 + 5).collect::<Vec<_>>(),
                )),
                Arc::new(Int32Array::from(
                    rows.iter().map(|r| r.3).collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from(
                    rows.iter().map(|r| r.0).collect::<Vec<_>>(),
                )),
                Arc::new(Int16Array::from(
                    rows.iter().map(|r| r.1).collect::<Vec<_>>(),
                )),
                Arc::new(UInt8Array::from(
                    rows.iter().map(|r| r.4).collect::<Vec<_>>(),
                )),
                Arc::new(UInt8Array::from(
                    rows.iter().map(|r| r.5).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter().map(|_| "").collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter().map(|r| r.6).collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap()
    }

    fn yard_dir(rings: &[(u8, GridRing)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("yards-stamp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let schema = Schema::new(vec![
            Field::new("source_type", DataType::UInt8, false),
            Field::new("geom", DataType::Binary, false),
        ]);
        let geoms: Vec<Vec<u8>> = rings.iter().map(|r| encode_grid_poly(&r.1)).collect();
        let geom_refs: Vec<&[u8]> = geoms.iter().map(Vec::as_slice).collect();
        let batch = RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![
                Arc::new(UInt8Array::from(
                    rings.iter().map(|r| r.0).collect::<Vec<_>>(),
                )) as ArrayRef,
                Arc::new(BinaryArray::from(geom_refs)),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(dir.join("industrial.arrow")).unwrap();
        let mut writer = arrow::ipc::writer::FileWriter::try_new(file, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        dir
    }

    #[test]
    fn stamp_keeps_through_tracks_and_tags() {
        let yard: GridRing = vec![(0, 0), (200, 0), (200, 200), (0, 200)];
        let dir = yard_dir(&[
            (YARD_SOURCE_TYPE, yard),
            (0, vec![(0, 0), (10, 0), (10, 10)]),
        ]);
        let merged = merged_batch(&[
            (11, 0, 100, 100, 1, 0, ""),          // untagged branch inside -> yard
            (12, 0, 100, 120, 1, 0, "Thru Main"), // named inside -> through
            (13, 0, 500, 500, 1, 0, ""),          // untagged outside -> kept
            (14, 0, 100, 140, 1, 0, ""),          // timetable-covered -> through
            (15, 0, 100, 160, 0, 0, ""),          // main-tagged inside -> through
            (16, 0, 100, 180, 1, 0, ""),          // way reaches outside -> through
            (16, 1, 600, 100, 1, 0, ""),
        ]);
        let mut intervals: HashMap<(i64, i16), Vec<Interval>> = HashMap::new();
        intervals.insert(
            (14, 0),
            vec![Interval {
                osm_id: 14,
                segment_idx: 0,
                from_m: 0.0,
                to_m: 10.0,
                country: *b"US",
                source_id: 100,
                passenger: 10.0,
                freight: 0.0,
                passenger_status: 2,
                freight_status: 0,
                matching: 0,
            }],
        );
        let stamped = stamp_yard_service(&merged, &dir, &intervals).unwrap();
        let service = stamped
            .column_by_name("service")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let got: Vec<u8> = (0..stamped.num_rows()).map(|r| service.value(r)).collect();
        assert_eq!(got, vec![1, 0, 0, 0, 0, 0, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_industrial_arrow_passes_through() {
        let dir = std::env::temp_dir().join(format!("yards-stamp-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let merged = merged_batch(&[(11, 0, 100, 100, 1, 0, "")]);
        let stamped = stamp_yard_service(&merged, &dir, &HashMap::new()).unwrap();
        let service = stamped
            .column_by_name("service")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        assert_eq!(service.value(0), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
