//! Pipeline step `structures-finalize`, per square: z14-block the merged
//! `structures.arrow` (the Python merge writes plain 4 096-row chunks without
//! any envelope), then write `structures.qoix` from the final bytes.
//!
//! Blocking is the ONE implementation every layer uses
//! (`arrow_batching::blocked_by_z14_cell`): rows are re-grouped by the z14 cell
//! of their envelope midpoint so the popup decodes only the batches within
//! reach. Row order is not a contract of this file — the index follows the
//! `screening_ordinal` column, the emission-view proof runs on the merge's
//! in-memory table, every reader filters rows by their own coordinates — so
//! the re-batch is free to permute rows; every row's values survive
//! unchanged, as does the schema metadata (contract, grid, the merge's
//! resume key). Idempotent: a table that already carries `qm_blocks`, or has
//! no rows (nothing to prune, no key by definition), is left alone.

use std::io::{Cursor, Write};
use std::path::Path;

use arrow::array::Array;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::FileWriter;
use arrow_batching::RowBbox;
use grid::Square;
use square_store::grid_cols::{col_binary, col_i32, col_u8, grid_cell_lonlat};
use square_store::store::{STRUCTURE_KIND_BARRIER, STRUCTURE_KIND_BUILDING};
use square_store::structure_contract;

use crate::square_obstacle_index::{
    write_square_obstacle_index, ObstacleIndexReceipt, STRUCTURES_ARROW, STRUCTURES_QOIX,
};

/// What the step did for one square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructuresFinalizeReceipt {
    /// `true` when `structures.arrow` was rewritten z14-blocked.
    pub blocked: bool,
    pub index: ObstacleIndexReceipt,
}

/// Finalize one square: block its `structures.arrow` when the merge's plain
/// chunks are still on disk, then write or verify `structures.qoix` against
/// the final bytes. `Ok(None)` when the square has no `structures.arrow`.
pub fn finalize_square_structures(
    square_dir: &Path,
    square: Square,
) -> Result<Option<StructuresFinalizeReceipt>, String> {
    let arrow_path = square_dir.join(STRUCTURES_ARROW);
    if !arrow_path.is_file() {
        return Ok(None);
    }
    let merged =
        std::fs::read(&arrow_path).map_err(|e| format!("read {}: {e}", arrow_path.display()))?;
    remove_crash_leftovers(square_dir)?;
    let blocked = blocked_structures_ipc(&merged, &arrow_path)?;
    if let Some(bytes) = &blocked {
        write_file_atomically(square_dir, STRUCTURES_ARROW, &[bytes])?;
    }
    let bytes = blocked.as_deref().unwrap_or(&merged);
    let index = write_square_obstacle_index(square_dir, square, bytes)?;
    Ok(Some(StructuresFinalizeReceipt {
        blocked: blocked.is_some(),
        index,
    }))
}

/// The z14-blocked IPC bytes of a merged table, or `None` when the file is
/// already final (carries `qm_blocks`, or has no rows). The contract is
/// checked first so a broken table is never stamped as finished.
pub fn blocked_structures_ipc(bytes: &[u8], label: &Path) -> Result<Option<Vec<u8>>, String> {
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", label.display()))?;
    let schema = reader.schema();
    structure_contract::validate_schema(&schema)?;
    if schema
        .metadata()
        .contains_key(arrow_batching::QM_BLOCKS_KEY)
    {
        return Ok(None);
    }
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("arrow batch {}: {e}", label.display()))?;
    let merged = arrow::compute::concat_batches(&schema, &batches)
        .map_err(|e| format!("{}: {e}", label.display()))?;
    if merged.num_rows() == 0 {
        return Ok(None);
    }
    let row_bboxes = (0..merged.num_rows())
        .map(|row| row_bbox(&merged, row).map_err(|e| format!("{}: {e}", label.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let (schema, batches) = arrow_batching::blocked_by_z14_cell(
        schema.as_ref().clone(),
        merged.columns().to_vec(),
        &row_bboxes,
    )
    .map_err(|e| format!("{}: {e}", label.display()))?;
    let mut out = Vec::with_capacity(bytes.len());
    let mut writer =
        FileWriter::try_new(&mut out, &schema).map_err(|e| format!("{}: {e}", label.display()))?;
    for batch in &batches {
        writer
            .write(batch)
            .map_err(|e| format!("{}: {e}", label.display()))?;
    }
    writer
        .finish()
        .map_err(|e| format!("{}: {e}", label.display()))?;
    drop(writer);
    Ok(Some(out))
}

/// Envelope of one row's complete geometry: every ring of a footprint, the
/// two ends of a wall microsegment, the centroid cell for a geometry-less row.
fn row_bbox(batch: &arrow::record_batch::RecordBatch, row: usize) -> Result<RowBbox, String> {
    let missing = |name: &str| format!("missing {name}");
    let kind = col_u8(batch, "kind").ok_or_else(|| missing("kind"))?;
    let geom = col_binary(batch, "geom").ok_or_else(|| missing("geom"))?;
    let rings: Vec<grid::poly::GridRing> = if geom.is_null(row) {
        Vec::new()
    } else if kind.value(row) == STRUCTURE_KIND_BUILDING {
        grid::poly::decode_grid_polygons(geom.value(row))
            .ok_or_else(|| format!("row {row} has invalid building topology"))?
            .into_iter()
            .flatten()
            .collect()
    } else if kind.value(row) == STRUCTURE_KIND_BARRIER {
        vec![grid::poly::decode_grid_poly(geom.value(row))
            .ok_or_else(|| format!("row {row} invalid wall geometry"))?]
    } else {
        return Err(format!(
            "unknown structure kind {} at row {row}",
            kind.value(row)
        ));
    };
    let union = |a: RowBbox, b: RowBbox| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    };
    let mut bbox = rings
        .iter()
        .filter_map(|ring| grid::poly::ring_bbox_lonlat(ring))
        .reduce(union);
    // The popup accepts a building row by its EMISSION position (the OSM
    // centroid and ring the merge kept beside the Overture footprint), which
    // a `covers` match can leave well outside the footprint: the envelope
    // must contain everything a row-level gate can measure against.
    let point = |gx_name: &str, gy_name: &str| -> Result<Option<RowBbox>, String> {
        let gx = col_i32(batch, gx_name).ok_or_else(|| missing(gx_name))?;
        let gy = col_i32(batch, gy_name).ok_or_else(|| missing(gy_name))?;
        if gx.is_null(row) || gy.is_null(row) {
            return Ok(None);
        }
        let (lon, lat) = grid_cell_lonlat(gx.value(row), gy.value(row));
        Ok(Some([lat, lon, lat, lon]))
    };
    if kind.value(row) == STRUCTURE_KIND_BUILDING {
        if let Some(emission) = point("emission_centroid_gx", "emission_centroid_gy")? {
            bbox = Some(bbox.map_or(emission, |b| union(b, emission)));
        }
        let emission_geom =
            col_binary(batch, "emission_geom").ok_or_else(|| missing("emission_geom"))?;
        if !emission_geom.is_null(row) {
            let ring = grid::poly::decode_grid_poly(emission_geom.value(row))
                .ok_or_else(|| format!("row {row} has an invalid emission ring"))?;
            if let Some(emission) = grid::poly::ring_bbox_lonlat(&ring) {
                bbox = Some(bbox.map_or(emission, |b| union(b, emission)));
            }
        }
    }
    if let Some(bbox) = bbox {
        return Ok(bbox);
    }
    point("centroid_gx", "centroid_gy")?
        .ok_or_else(|| format!("row {row} has neither geometry nor a centroid"))
}

/// Same-directory tmp + fsync + rename + directory fsync. rename() keeps the
/// old inode alive for a popup that still maps it and swaps readers
/// atomically; a crash leaves only a `<name>.tmp.<pid>` the next run sweeps.
pub(crate) fn write_file_atomically(
    square_dir: &Path,
    name: &str,
    parts: &[&[u8]],
) -> Result<(), String> {
    let path = square_dir.join(name);
    let tmp = square_dir.join(format!("{name}.tmp.{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        for part in parts {
            f.write_all(part)?;
        }
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)?;
        std::fs::File::open(square_dir)?.sync_all()
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("write {}: {e}", path.display())
    })
}

/// The step runs after the merge and is then the only writer of both files,
/// so any `structures.{arrow,qoix}.tmp.*` in the square is a crash leftover
/// of an earlier run (another pid) — up to a metro square's 1.1 GB, never
/// reclaimed otherwise.
fn remove_crash_leftovers(square_dir: &Path) -> Result<(), String> {
    let prefixes = [
        format!("{STRUCTURES_ARROW}.tmp."),
        format!("{STRUCTURES_QOIX}.tmp."),
    ];
    for entry in
        std::fs::read_dir(square_dir).map_err(|e| format!("{}: {e}", square_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("{}: {e}", square_dir.display()))?;
        let name = entry.file_name();
        if prefixes
            .iter()
            .any(|p| name.to_string_lossy().starts_with(p))
        {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("remove {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square_obstacle_index::{load_square_obstacle_index, structures_fingerprint};
    use crate::structure_test_fixture as fx;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    /// One row per z14 cell, listed south-to-north so the row-major re-batch
    /// must move rows; a geometry-less row files under its centroid.
    fn merged_rows() -> Vec<fx::StructureRow> {
        let house = |lat: f64, lon: f64, osm_id: i64| fx::StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(lat, lon)),
            height_m: 12,
            envelope_class: 1,
            centroid_lonlat: Some((lon + 0.0001, lat + 0.0001)),
            osm_id: Some(osm_id),
            building_type: Some(1),
            area_m2: Some(450.0),
            name: Some(format!("house {osm_id}")),
            ..Default::default()
        };
        vec![
            house(LAT, LON, 1),
            house(LAT + 0.03, LON, 2),
            fx::StructureRow {
                kind: STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(LON, LAT + 0.06), (LON + 0.001, LAT + 0.061)]),
                height_m: 3,
                centroid_lonlat: Some((LON + 0.0005, LAT + 0.0605)),
                osm_id: Some(11),
                segment_idx: Some(2),
                ..Default::default()
            },
            fx::StructureRow {
                kind: STRUCTURE_KIND_BUILDING,
                ring_lonlat: None,
                height_m: 0,
                centroid_lonlat: Some((LON, LAT + 0.09)),
                osm_id: Some(3),
                screening_ordinal: Some(u32::MAX), // never indexed: no geometry
                ..Default::default()
            },
        ]
    }

    fn identity(path: &Path) -> (u64, u64, i64, i64) {
        let m = std::fs::metadata(path).unwrap();
        (m.ino(), m.len(), m.mtime(), m.mtime_nsec())
    }

    fn read_all(path: &Path) -> arrow::record_batch::RecordBatch {
        let reader = FileReader::try_new(std::fs::File::open(path).unwrap(), None).unwrap();
        let schema = reader.schema();
        let batches: Vec<_> = reader.map(Result::unwrap).collect();
        arrow::compute::concat_batches(&schema, &batches).unwrap()
    }

    /// A merge-matched building keeps the OSM emission centroid and ring beside
    /// the Overture footprint, and the popup gates the row on that emission
    /// position: the block envelope must cover it, or a click 2 km from the
    /// emission point could prune the batch the row lives in.
    #[test]
    fn block_envelope_covers_the_emission_position_of_a_matched_building() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        // Footprint at the square centre, OSM emission ring and centroid ~300 m east.
        let far_lon = LON + 0.0042;
        let row = fx::StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            height_m: 10,
            envelope_class: 1,
            centroid_lonlat: Some((LON, LAT)),
            osm_id: Some(9),
            building_type: Some(1),
            area_m2: Some(200.0),
            emission_ring_lonlat: Some(fx::square_ring_lonlat(LAT, far_lon)),
            emission_centroid_lonlat: Some((far_lon, LAT)),
            ..Default::default()
        };
        fx::write_structure_file(&dir.join(STRUCTURES_ARROW), &[row], true);
        finalize_square_structures(&dir, square).unwrap().unwrap();
        let blocked = read_all(&dir.join(STRUCTURES_ARROW));
        let blocks = arrow_batching::parse_blocks(
            blocked
                .schema()
                .metadata()
                .get(arrow_batching::QM_BLOCKS_KEY)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(blocks.len(), 1);
        let bbox = blocks[0].bbox;
        assert!(
            bbox[1] <= LON && bbox[3] >= far_lon,
            "envelope {bbox:?} misses the emission ring"
        );
    }

    /// The merge's plain chunks become z14 blocks once — every row's values
    /// and the schema metadata survive, the index binds to the final bytes,
    /// and a rerun writes neither file (a 0-row table included).
    #[test]
    fn merged_table_is_blocked_once_and_the_index_binds_to_the_final_bytes() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let dir = fx::square_dir(tmp.path(), square);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(finalize_square_structures(&dir, square).unwrap(), None);
        let arrow = dir.join(STRUCTURES_ARROW);
        fx::write_structure_file(&arrow, &merged_rows(), true);
        std::fs::write(
            dir.join(format!("{STRUCTURES_ARROW}.tmp.1")),
            b"crashed merge",
        )
        .unwrap();
        let merged = read_all(&arrow);
        assert!(!merged
            .schema()
            .metadata()
            .contains_key(arrow_batching::QM_BLOCKS_KEY));

        let first = finalize_square_structures(&dir, square).unwrap().unwrap();
        assert!(first.blocked && first.index.written, "{first:?}");
        assert_eq!(first.index.edge_count, 9, "two houses and one wall");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            2,
            "pair only, tmp swept"
        );

        let blocked = read_all(&arrow);
        let blocks = arrow_batching::parse_blocks(
            blocked
                .schema()
                .metadata()
                .get(arrow_batching::QM_BLOCKS_KEY)
                .expect("finalized table carries qm_blocks"),
        )
        .unwrap();
        assert_eq!(blocks.len(), 4, "one block per z14 cell");
        assert!(
            blocks.windows(2).all(|w| w[0].cell_y < w[1].cell_y),
            "row-major cells: {blocks:?}"
        );
        for (key, value) in merged.schema().metadata() {
            assert_eq!(blocked.schema().metadata().get(key), Some(value), "{key}");
        }
        assert_eq!(blocked.num_rows(), merged.num_rows());
        let ordinal = |batch: &arrow::record_batch::RecordBatch, row: usize| {
            square_store::grid_cols::col_u32(batch, "screening_ordinal")
                .unwrap()
                .value(row)
        };
        let merged_row_of = |o: u32| (0..merged.num_rows()).find(|&r| ordinal(&merged, r) == o);
        let order: Vec<u32> = (0..blocked.num_rows())
            .map(|r| ordinal(&blocked, r))
            .collect();
        assert_eq!(
            order,
            [u32::MAX, 2, 1, 0],
            "south-listed rows end up north-first"
        );
        for row in 0..blocked.num_rows() {
            let source = merged_row_of(ordinal(&blocked, row)).unwrap();
            for (column, original) in blocked.columns().iter().zip(merged.columns()) {
                assert_eq!(
                    column.slice(row, 1).as_ref(),
                    original.slice(source, 1).as_ref(),
                    "row {row}"
                );
            }
        }
        let final_bytes = std::fs::read(&arrow).unwrap();
        assert!(
            load_square_obstacle_index(&dir, Some(structures_fingerprint(&final_bytes)))
                .unwrap()
                .is_some()
        );

        let before = (identity(&arrow), identity(&dir.join(STRUCTURES_QOIX)));
        let again = finalize_square_structures(&dir, square).unwrap().unwrap();
        assert!(!again.blocked && !again.index.written, "{again:?}");
        assert_eq!(
            before,
            (identity(&arrow), identity(&dir.join(STRUCTURES_QOIX)))
        );

        let empty = fx::square_dir(tmp.path(), grid::Square { x: 0, y: 0 });
        std::fs::create_dir_all(&empty).unwrap();
        fx::write_structure_file(&empty.join(STRUCTURES_ARROW), &[], true);
        let bytes = std::fs::read(empty.join(STRUCTURES_ARROW)).unwrap();
        let first = finalize_square_structures(&empty, grid::Square { x: 0, y: 0 })
            .unwrap()
            .unwrap();
        assert!(!first.blocked && first.index.written && first.index.edge_count == 0);
        assert_eq!(std::fs::read(empty.join(STRUCTURES_ARROW)).unwrap(), bytes);
        let again = finalize_square_structures(&empty, grid::Square { x: 0, y: 0 })
            .unwrap()
            .unwrap();
        assert!(!again.blocked && !again.index.written, "{again:?}");
    }
}
