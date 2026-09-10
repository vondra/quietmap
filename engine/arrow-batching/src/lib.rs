//! z14 block layout of prepared Arrow files — the ONE definition of `qm_blocks`.
//!
//! Writers (osm-extract finalize, aircraft-extract arrow_io) group a file's
//! rows by the global z14 cell of their envelope midpoint, chunk each cell into
//! record batches of at most [`MAX_ROWS_PER_BLOCK_BATCH`] rows, and stamp one
//! fixed binary record per batch — cell and full-geometry envelope — into the
//! schema metadata. The popup reader (square-store) skips decoding any batch
//! whose envelope lies farther from the click than the layer's reach. File IO
//! stays in each caller; this crate only orders columns and owns the record.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, UInt32Array};
use arrow::compute::take;
use arrow::datatypes::Schema;
use arrow::error::ArrowError;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

/// Schema metadata key: base64 of `[u8 version = 1]` then one
/// [`BLOCK_RECORD_LEN`]-byte little-endian record per batch, in batch order —
/// `u16 z14 x, u16 z14 y, f64 min_lat, f64 min_lon, f64 max_lat, f64 max_lon,
/// f32 min_alt_m, f32 max_alt_m` (the altitude range is airborne's; surface
/// layers write `0, 0`).
/// Readers MUST ignore the key when its record count differs from the file's
/// batch count (an enrichment rewrite may re-chunk) — degrade to load-all.
pub const QM_BLOCKS_KEY: &str = "qm_blocks";

/// Decode granule inside one cell: big enough to amortize per-batch IPC
/// overhead, small enough that a dense city cell stays a fine prune granule.
pub const MAX_ROWS_PER_BLOCK_BATCH: usize = 4096;

/// 32 × 32 cells per z9 square; 1.57 km at Prague. Nests into the z13 paint tile.
pub const BLOCK_ZOOM: u32 = 14;
const QM_BLOCKS_VERSION: u8 = 1;
const BLOCK_RECORD_LEN: usize = 2 + 2 + 4 * 8 + 2 * 4;

/// One row's geometry envelope, degrees: `[min_lat, min_lon, max_lat, max_lon]`.
/// For point rows use a degenerate box. A geometry straddling the antimeridian
/// yields a near-global box — never pruned, which is safe (just unpruned).
pub type RowBbox = [f64; 4];

/// One row's altitude range in metres: `[min_alt_m, max_alt_m]`.
pub type RowAltitudeRange = [f32; 2];

/// One record batch's block: its z14 cell, the envelope of its complete
/// geometries and the altitude range of its rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub cell_x: u16,
    pub cell_y: u16,
    pub bbox: RowBbox,
    pub alt_m: RowAltitudeRange,
}

/// z14 cell of a row envelope's midpoint.
pub fn z14_cell_of_bbox_midpoint(bbox: &RowBbox) -> (u16, u16) {
    // The short-arc midpoint files a seam-straddling geometry under a seam
    // cell, not under Greenwich.
    let (x, y) = grid::web_mercator_cell_axes(
        (bbox[0] + bbox[2]) / 2.0,
        grid::geo::wrapped_longitude_midpoint(bbox[1], bbox[3]),
        BLOCK_ZOOM,
    );
    (x as u16, y as u16)
}

/// Order `columns` by the z14 cell of `row_bboxes` (row-major, stable inside a
/// cell so bytes are reproducible), chunk every cell into batches of at most
/// [`MAX_ROWS_PER_BLOCK_BATCH`] rows, and stamp `qm_blocks` into the schema
/// (existing metadata — contracts, n_days — is preserved). Empty input returns
/// a single empty batch WITHOUT the key (nothing to prune). Surface layers have
/// no altitude; their block records carry `0, 0`.
pub fn blocked_by_z14_cell(
    base_schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[RowBbox],
) -> Result<(Arc<Schema>, Vec<RecordBatch>), ArrowError> {
    let ground = vec![[0.0_f32; 2]; row_bboxes.len()];
    blocked_by_z14_cell_with_altitude(base_schema, columns, row_bboxes, &ground)
}

/// [`blocked_by_z14_cell`] whose block records also carry the batch's altitude
/// range, the union of `row_altitudes` — airborne readers skip a block whose
/// rows all fly beyond reach.
pub fn blocked_by_z14_cell_with_altitude(
    base_schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[RowBbox],
    row_altitudes: &[RowAltitudeRange],
) -> Result<(Arc<Schema>, Vec<RecordBatch>), ArrowError> {
    let n = row_bboxes.len();
    if row_altitudes.len() != n {
        return Err(ArrowError::InvalidArgumentError(format!(
            "blocked_by_z14_cell: {} altitude ranges, bboxes have {n}",
            row_altitudes.len()
        )));
    }
    for (i, col) in columns.iter().enumerate() {
        if col.len() != n {
            return Err(ArrowError::InvalidArgumentError(format!(
                "blocked_by_z14_cell: column {i} has {} rows, bboxes have {n}",
                col.len()
            )));
        }
    }
    if n == 0 {
        let schema = Arc::new(base_schema);
        let batch = RecordBatch::try_new_with_options(
            schema.clone(),
            columns,
            &RecordBatchOptions::new().with_row_count(Some(0)),
        )?;
        return Ok((schema, vec![batch]));
    }

    let cells: Vec<(u16, u16)> = row_bboxes.iter().map(z14_cell_of_bbox_midpoint).collect();
    let mut permutation: Vec<usize> = (0..n).collect();
    permutation.sort_by_key(|&i| (cells[i].1, cells[i].0));
    let indices = UInt32Array::from(permutation.iter().map(|&i| i as u32).collect::<Vec<_>>());
    let columns = columns
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<Result<Vec<_>, _>>()?;

    let mut blocks: Vec<Block> = Vec::new();
    let mut batch_ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < n {
        let cell = cells[permutation[start]];
        let mut end = start;
        while end < n && end - start < MAX_ROWS_PER_BLOCK_BATCH && cells[permutation[end]] == cell {
            end += 1;
        }
        let mut bbox = row_bboxes[permutation[start]];
        let mut alt_m = row_altitudes[permutation[start]];
        for &row in &permutation[start + 1..end] {
            let b = row_bboxes[row];
            bbox = [
                bbox[0].min(b[0]),
                bbox[1].min(b[1]),
                bbox[2].max(b[2]),
                bbox[3].max(b[3]),
            ];
            let a = row_altitudes[row];
            alt_m = [alt_m[0].min(a[0]), alt_m[1].max(a[1])];
        }
        blocks.push(Block {
            cell_x: cell.0,
            cell_y: cell.1,
            bbox,
            alt_m,
        });
        batch_ranges.push((start, end - start));
        start = end;
    }

    let mut metadata: HashMap<String, String> = base_schema.metadata().clone();
    metadata.insert(QM_BLOCKS_KEY.to_string(), encode_blocks(&blocks));
    let schema = Arc::new(Schema::new_with_metadata(
        base_schema.fields().clone(),
        metadata,
    ));
    let full = RecordBatch::try_new(schema.clone(), columns)?;
    let batches = batch_ranges
        .into_iter()
        .map(|(offset, length)| full.slice(offset, length))
        .collect();
    Ok((schema, batches))
}

/// The `qm_blocks` metadata value for `blocks` in batch order.
pub fn encode_blocks(blocks: &[Block]) -> String {
    let mut bytes = Vec::with_capacity(1 + blocks.len() * BLOCK_RECORD_LEN);
    bytes.push(QM_BLOCKS_VERSION);
    for block in blocks {
        bytes.extend_from_slice(&block.cell_x.to_le_bytes());
        bytes.extend_from_slice(&block.cell_y.to_le_bytes());
        for value in block.bbox {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in block.alt_m {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    BASE64.encode(bytes)
}

/// Parse the `qm_blocks` metadata value. `None` on any malformation — the
/// reader then treats the file as unpruned (never abort: a stale value must
/// degrade, not fail the popup).
pub fn parse_blocks(value: &str) -> Option<Vec<Block>> {
    let bytes = BASE64.decode(value.trim()).ok()?;
    let (&version, records) = bytes.split_first()?;
    if version != QM_BLOCKS_VERSION || records.len() % BLOCK_RECORD_LEN != 0 {
        return None;
    }
    let axis = 1u16 << BLOCK_ZOOM;
    records
        .chunks_exact(BLOCK_RECORD_LEN)
        .map(|record| {
            let u16_at = |at: usize| u16::from_le_bytes(record[at..at + 2].try_into().unwrap());
            let f64_at = |at: usize| f64::from_le_bytes(record[at..at + 8].try_into().unwrap());
            let f32_at = |at: usize| f32::from_le_bytes(record[at..at + 4].try_into().unwrap());
            let block = Block {
                cell_x: u16_at(0),
                cell_y: u16_at(2),
                bbox: [f64_at(4), f64_at(12), f64_at(20), f64_at(28)],
                alt_m: [f32_at(36), f32_at(40)],
            };
            let bbox = block.bbox;
            let well_formed = block.cell_x < axis
                && block.cell_y < axis
                && bbox.iter().all(|v| v.is_finite())
                && bbox[0] <= bbox[2]
                && bbox[1] <= bbox[3]
                && block.alt_m.iter().all(|v| v.is_finite())
                && block.alt_m[0] <= block.alt_m[1];
            well_formed.then_some(block)
        })
        .collect()
}

/// Great-circle distance (meters) from a point to the closest point of a
/// bbox; 0 when inside. Clamping lon/lat to the box then haversine; the point
/// is also tried one turn east and west, so a click beside the antimeridian
/// measures a box on the other side of the seam by the short arc (a straddling
/// geometry arrives as a near-global box and returns ~0 — safely unpruned).
pub fn point_to_bbox_distance_m(lat: f64, lon: f64, bbox: &RowBbox) -> f64 {
    let clat = lat.clamp(bbox[0], bbox[2]);
    [lon - 360.0, lon, lon + 360.0]
        .into_iter()
        .map(|lon| haversine_m(lat, lon, clat, lon.clamp(bbox[1], bbox[3])))
        .fold(f64::INFINITY, f64::min)
}

/// Haversine on the WGS-84 mean radius (~111,195 m/°lat). NOTE: the engine's
/// row-level filters use flat-earth metrics with ~110,540 m/°lat, i.e. THIS
/// function measures the same physical gap ~0.6% longer — callers gating
/// against row-filter radii must add slack (see square-store's
/// GATE_RADIUS_SLACK) or a boundary row's batch gets dropped.
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let (dlat, dlon) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

#[cfg(test)]
mod tests;
