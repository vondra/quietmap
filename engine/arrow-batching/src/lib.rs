//! Spatially-batched arrow format — the ONE definition of `qm_batch_bboxes`.
//!
//! Writers (osm-extract finalize, aircraft-extract arrow_io) sort a file's rows
//! by Morton code of their bbox center, chunk them into record batches, and
//! stamp one bbox per batch into schema custom_metadata. The popup reader
//! (source-reader) skips decoding any batch whose bbox lies farther from the
//! click than the source class's audibility radius. File IO stays in each
//! caller — this crate only transforms columns and defines the metadata
//! contract. The contract itself is `qm_batch_bboxes` below — this crate IS the
//! SSOT (the design note it used to cite is not in this repo).

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, UInt32Array};
use arrow::compute::take;
use arrow::datatypes::Schema;
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;

/// Schema custom_metadata key holding the per-batch bboxes as JSON
/// `[[min_lat,min_lon,max_lat,max_lon], ...]` in batch order. Arrow IPC
/// metadata is file-scoped; batch index ↔ array index is the contract.
/// Readers MUST ignore the key when its length differs from the file's
/// batch count (enrichment rewrites may re-chunk) — degrade to load-all.
pub const QM_BATCH_BBOXES_KEY: &str = "qm_batch_bboxes";

/// Decode granule: big enough to amortize per-batch IPC overhead (footer
/// block + one memcpy per decode), small enough that one batch of a dense
/// hex stays a fine prune granule (657k-segment SFO file → 64 batches).
pub const TARGET_ROWS_PER_BATCH: usize = 4096;

/// Caps the metadata size (64 bboxes ≈ 3 KB JSON) and the footer block list.
pub const MAX_BATCHES_PER_FILE: usize = 64;

/// Morton sort grid: 64×64 cells over the file's bbox — finer than the max
/// batch count so consecutive-row runs cluster tightly inside each batch.
const MORTON_GRID: usize = 64;

/// One row's geometry envelope, degrees: `[min_lat, min_lon, max_lat, max_lon]`.
/// For point rows use a degenerate box. A geometry straddling the antimeridian
/// yields a near-global box — never pruned, which is safe (just unpruned).
pub type RowBbox = [f64; 4];

/// Sort `columns` spatially by `row_bboxes`, chunk into batches, and stamp
/// `qm_batch_bboxes` into the schema (existing metadata — contracts, n_days —
/// is preserved). Empty input returns a single empty batch WITHOUT the bbox
/// key (nothing to prune). Single-batch output skips the sort (order is
/// irrelevant to one bbox) but still carries the key so a reader can skip the
/// whole file with one distance test.
pub fn spatially_batched(
    base_schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[RowBbox],
) -> Result<(Arc<Schema>, Vec<RecordBatch>), ArrowError> {
    let n = row_bboxes.len();
    for (i, col) in columns.iter().enumerate() {
        if col.len() != n {
            return Err(ArrowError::InvalidArgumentError(format!(
                "spatially_batched: column {i} has {} rows, bboxes have {n}",
                col.len()
            )));
        }
    }

    if n == 0 {
        let schema = Arc::new(base_schema);
        let batch = RecordBatch::try_new_with_options(
            schema.clone(),
            columns,
            &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(0)),
        )?;
        return Ok((schema, vec![batch]));
    }

    let num_batches = n
        .div_ceil(TARGET_ROWS_PER_BATCH)
        .clamp(1, MAX_BATCHES_PER_FILE);
    let rows_per_batch = n.div_ceil(num_batches);

    let (columns, ordered_bboxes) = if num_batches > 1 {
        let perm = morton_permutation(row_bboxes);
        let idx = UInt32Array::from(perm.iter().map(|&i| i as u32).collect::<Vec<_>>());
        let taken = columns
            .iter()
            .map(|c| take(c.as_ref(), &idx, None))
            .collect::<Result<Vec<_>, _>>()?;
        let bboxes = perm.iter().map(|&i| row_bboxes[i]).collect::<Vec<_>>();
        (taken, bboxes)
    } else {
        (columns, row_bboxes.to_vec())
    };

    let mut batch_bboxes = Vec::with_capacity(num_batches);
    for chunk in ordered_bboxes.chunks(rows_per_batch) {
        let mut bb = chunk[0];
        for b in &chunk[1..] {
            bb[0] = bb[0].min(b[0]);
            bb[1] = bb[1].min(b[1]);
            bb[2] = bb[2].max(b[2]);
            bb[3] = bb[3].max(b[3]);
        }
        batch_bboxes.push(bb);
    }

    let mut metadata: HashMap<String, String> = base_schema.metadata().clone();
    metadata.insert(
        QM_BATCH_BBOXES_KEY.to_string(),
        encode_batch_bboxes(&batch_bboxes),
    );
    let schema = Arc::new(Schema::new_with_metadata(
        base_schema.fields().clone(),
        metadata,
    ));

    let full = RecordBatch::try_new(schema.clone(), columns)?;
    let batches = (0..batch_bboxes.len())
        .map(|i| {
            let offset = i * rows_per_batch;
            full.slice(offset, rows_per_batch.min(n - offset))
        })
        .collect();
    Ok((schema, batches))
}

/// Stable permutation of row indices by Morton code of each bbox center on a
/// MORTON_GRID² grid over the overall bbox. Morton (Z-order) keeps spatially
/// close rows close in the order, so contiguous chunks get tight bboxes.
fn morton_permutation(row_bboxes: &[RowBbox]) -> Vec<usize> {
    let mut overall = row_bboxes[0];
    for b in &row_bboxes[1..] {
        overall[0] = overall[0].min(b[0]);
        overall[1] = overall[1].min(b[1]);
        overall[2] = overall[2].max(b[2]);
        overall[3] = overall[3].max(b[3]);
    }
    let lat_span = (overall[2] - overall[0]).max(1e-9);
    let lon_span = (overall[3] - overall[1]).max(1e-9);
    let cell = |b: &RowBbox| -> u32 {
        let clat = (b[0] + b[2]) / 2.0;
        let clon = (b[1] + b[3]) / 2.0;
        let gy = (((clat - overall[0]) / lat_span) * MORTON_GRID as f64) as usize;
        let gx = (((clon - overall[1]) / lon_span) * MORTON_GRID as f64) as usize;
        morton_interleave(
            gx.min(MORTON_GRID - 1) as u16,
            gy.min(MORTON_GRID - 1) as u16,
        )
    };
    let mut perm: Vec<usize> = (0..row_bboxes.len()).collect();
    perm.sort_by_key(|&i| (cell(&row_bboxes[i]), i));
    perm
}

/// Interleave the low 16 bits of x and y (x in even positions).
fn morton_interleave(x: u16, y: u16) -> u32 {
    fn spread(v: u16) -> u32 {
        let mut v = v as u32;
        v = (v | (v << 8)) & 0x00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333;
        v = (v | (v << 1)) & 0x5555_5555;
        v
    }
    spread(x) | (spread(y) << 1)
}

fn encode_batch_bboxes(bboxes: &[RowBbox]) -> String {
    let parts: Vec<String> = bboxes
        .iter()
        .map(|b| format!("[{},{},{},{}]", b[0], b[1], b[2], b[3]))
        .collect();
    format!("[{}]", parts.join(","))
}

/// Parse the `qm_batch_bboxes` metadata value. Returns `None` on any
/// malformation — the reader then treats the file as unpruned (never abort:
/// a stale/re-chunked value must degrade, not fail the popup).
pub fn parse_batch_bboxes(value: &str) -> Option<Vec<RowBbox>> {
    let inner = value.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for group in inner.split("],") {
        let group = group.trim().trim_start_matches('[').trim_end_matches(']');
        let nums: Vec<f64> = group
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .ok()?;
        if nums.len() != 4 || nums[0] > nums[2] || nums[1] > nums[3] {
            return None;
        }
        out.push([nums[0], nums[1], nums[2], nums[3]]);
    }
    Some(out)
}

/// Great-circle distance (meters) from a point to the closest point of a
/// bbox; 0 when inside. Clamping lon/lat to the box then haversine is exact
/// for boxes that don't straddle the antimeridian (those arrive near-global
/// and return ~0 — safely unpruned).
pub fn point_to_bbox_distance_m(lat: f64, lon: f64, bbox: &RowBbox) -> f64 {
    let clat = lat.clamp(bbox[0], bbox[2]);
    let clon = lon.clamp(bbox[1], bbox[3]);
    haversine_m(lat, lon, clat, clon)
}

/// Haversine on the WGS-84 mean radius (~111,195 m/°lat). NOTE: the engine's
/// row-level filters use flat-earth metrics with ~110,540 m/°lat, i.e. THIS
/// function measures the same physical gap ~0.6% longer — callers gating
/// against row-filter radii must add slack (see source-reader's
/// GATE_RADIUS_SLACK) or a boundary row's batch gets dropped.
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let (dlat, dlon) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float64Array, Int64Array};
    use arrow::datatypes::{DataType, Field};

    fn point_bbox(lat: f64, lon: f64) -> RowBbox {
        [lat, lon, lat, lon]
    }

    /// Build a synthetic file: ids 0..n scattered on a lat/lon grid.
    fn synthetic(n: usize) -> (Schema, Vec<ArrayRef>, Vec<RowBbox>) {
        let ids: Vec<i64> = (0..n as i64).collect();
        let lats: Vec<f64> = (0..n).map(|i| 50.0 + (i % 97) as f64 * 0.01).collect();
        let lons: Vec<f64> = (0..n).map(|i| 14.0 + (i / 97) as f64 * 0.01).collect();
        let bboxes: Vec<RowBbox> = lats
            .iter()
            .zip(&lons)
            .map(|(&la, &lo)| point_bbox(la, lo))
            .collect();
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("lat", DataType::Float64, false),
            Field::new("lon", DataType::Float64, false),
        ]);
        let cols: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(lats)),
            Arc::new(Float64Array::from(lons)),
        ];
        (schema, cols, bboxes)
    }

    #[test]
    fn round_trip_preserves_rows_and_bounds() {
        let n = 10_000;
        let (schema, cols, bboxes) = synthetic(n);
        let (schema, batches) = spatially_batched(schema, cols, &bboxes).unwrap();

        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, n);

        let parsed =
            parse_batch_bboxes(schema.metadata().get(QM_BATCH_BBOXES_KEY).unwrap()).unwrap();
        assert_eq!(parsed.len(), batches.len());
        assert!(batches.len() > 1 && batches.len() <= MAX_BATCHES_PER_FILE);

        // Row multiset preserved + every row inside its batch's bbox.
        let mut seen = vec![false; n];
        for (bi, batch) in batches.iter().enumerate() {
            let ids = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            let lats = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let lons = batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                let id = ids.value(i) as usize;
                assert!(!seen[id], "duplicate row {id}");
                seen[id] = true;
                let bb = parsed[bi];
                assert!(lats.value(i) >= bb[0] && lats.value(i) <= bb[2]);
                assert!(lons.value(i) >= bb[1] && lons.value(i) <= bb[3]);
            }
        }
        assert!(seen.into_iter().all(|s| s));
    }

    #[test]
    fn empty_input_single_empty_batch_no_key() {
        let (schema, cols, _) = synthetic(0);
        let (schema, batches) = spatially_batched(schema, cols, &[]).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 0);
        assert!(!schema.metadata().contains_key(QM_BATCH_BBOXES_KEY));
    }

    #[test]
    fn single_batch_keeps_input_order_and_carries_key() {
        let (schema, cols, bboxes) = synthetic(100);
        let (schema, batches) = spatially_batched(schema, cols, &bboxes).unwrap();
        assert_eq!(batches.len(), 1);
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert!((0..100).all(|i| ids.value(i) == i as i64));
        let parsed =
            parse_batch_bboxes(schema.metadata().get(QM_BATCH_BBOXES_KEY).unwrap()).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn existing_metadata_preserved() {
        let (schema, cols, bboxes) = synthetic(10);
        let schema = schema
            .with_metadata([("buildings_contract".to_string(), "buildings_v2".to_string())].into());
        let (schema, _) = spatially_batched(schema, cols, &bboxes).unwrap();
        assert_eq!(
            schema
                .metadata()
                .get("buildings_contract")
                .map(String::as_str),
            Some("buildings_v2")
        );
        assert!(schema.metadata().contains_key(QM_BATCH_BBOXES_KEY));
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(parse_batch_bboxes("not json").is_none());
        assert!(parse_batch_bboxes("[[1,2,3]]").is_none());
        assert!(parse_batch_bboxes("[[2,1,1,3]]").is_none()); // min_lat > max_lat
        assert_eq!(parse_batch_bboxes("[]").unwrap().len(), 0);
        let one = parse_batch_bboxes("[[49.5,13.9,50.1,14.6]]").unwrap();
        assert_eq!(one, vec![[49.5, 13.9, 50.1, 14.6]]);
    }

    #[test]
    fn bbox_distance_zero_inside_positive_outside() {
        let bb: RowBbox = [50.0, 14.0, 50.1, 14.2];
        assert_eq!(point_to_bbox_distance_m(50.05, 14.1, &bb), 0.0);
        let d = point_to_bbox_distance_m(50.05, 14.35, &bb); // ~0.15° lon east ≈ 10.7 km
        assert!((9_000.0..12_500.0).contains(&d), "d={d}");
    }
}
