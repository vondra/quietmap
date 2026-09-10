//! Block layout invariants: row multiset, envelopes, cell bounds, order, record parsing.

use super::*;
use arrow::array::{Float64Array, Int64Array};
use arrow::datatypes::{DataType, Field};

fn point_bbox(lat: f64, lon: f64) -> RowBbox {
    [lat, lon, lat, lon]
}

/// Synthetic file: ids 0..n on a lat/lon lattice with `step` degrees; a step
/// well under a z14 cell (0.022° lon, 0.014° lat at 50° N) piles thousands
/// of rows into one cell, a larger step scatters them over many.
fn synthetic(n: usize, step: f64) -> (Schema, Vec<ArrayRef>, Vec<RowBbox>) {
    let ids: Vec<i64> = (0..n as i64).collect();
    let lats: Vec<f64> = (0..n).map(|i| 50.0 + (i % 97) as f64 * step).collect();
    let lons: Vec<f64> = (0..n).map(|i| 14.0 + (i / 97) as f64 * step).collect();
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

fn blocks_of(schema: &Schema) -> Vec<Block> {
    parse_blocks(schema.metadata().get(QM_BLOCKS_KEY).unwrap()).unwrap()
}

fn column<T: 'static>(batch: &RecordBatch, index: usize) -> &T {
    batch.column(index).as_any().downcast_ref::<T>().unwrap()
}

#[test]
fn round_trip_preserves_rows_and_every_row_inside_its_batch_envelope() {
    let n = 10_000;
    let (schema, cols, bboxes) = synthetic(n, 0.01);
    let (schema, batches) = blocked_by_z14_cell(schema, cols, &bboxes).unwrap();
    let blocks = blocks_of(&schema);
    assert_eq!(blocks.len(), batches.len());
    assert!(batches.len() > 1);
    let mut seen = vec![false; n];
    for (block, batch) in blocks.iter().zip(&batches) {
        let ids = column::<Int64Array>(batch, 0);
        let lats = column::<Float64Array>(batch, 1);
        let lons = column::<Float64Array>(batch, 2);
        for i in 0..batch.num_rows() {
            let id = ids.value(i) as usize;
            assert!(!seen[id], "duplicate row {id}");
            seen[id] = true;
            let bb = block.bbox;
            assert!(lats.value(i) >= bb[0] && lats.value(i) <= bb[2]);
            assert!(lons.value(i) >= bb[1] && lons.value(i) <= bb[3]);
        }
    }
    assert!(seen.into_iter().all(|s| s));
}

#[test]
fn a_batch_never_spans_two_cells_and_never_exceeds_the_row_cap() {
    // 9 700 rows inside 0.001° × 0.001° at Prague: one z14 cell, three batches.
    let (schema, cols, bboxes) = synthetic(9_700, 0.00001);
    let (schema, batches) = blocked_by_z14_cell(schema, cols, &bboxes).unwrap();
    let blocks = blocks_of(&schema);
    assert_eq!(batches.len(), 3);
    assert_eq!(
        batches
            .iter()
            .map(RecordBatch::num_rows)
            .collect::<Vec<_>>(),
        [4096, 4096, 1508]
    );
    assert!(blocks
        .iter()
        .all(|b| (b.cell_x, b.cell_y) == (blocks[0].cell_x, blocks[0].cell_y)));

    let (schema, cols, bboxes) = synthetic(10_000, 0.01);
    let (schema, batches) = blocked_by_z14_cell(schema, cols, &bboxes).unwrap();
    for (block, batch) in blocks_of(&schema).iter().zip(&batches) {
        assert!(batch.num_rows() <= MAX_ROWS_PER_BLOCK_BATCH);
        let lats = column::<Float64Array>(batch, 1);
        let lons = column::<Float64Array>(batch, 2);
        for i in 0..batch.num_rows() {
            let cell = z14_cell_of_bbox_midpoint(&point_bbox(lats.value(i), lons.value(i)));
            assert_eq!(cell, (block.cell_x, block.cell_y));
        }
    }
}

#[test]
fn cells_in_row_major_order_and_input_order_inside_a_cell_reproducibly() {
    let encode = || {
        let (schema, cols, bboxes) = synthetic(10_000, 0.01);
        let (schema, batches) = blocked_by_z14_cell(schema, cols, &bboxes).unwrap();
        let ids: Vec<Vec<i64>> = batches
            .iter()
            .map(|b| column::<Int64Array>(b, 0).values().to_vec())
            .collect();
        (schema.metadata().get(QM_BLOCKS_KEY).unwrap().clone(), ids)
    };
    let (encoded, ids) = encode();
    assert_eq!(encode(), (encoded.clone(), ids.clone()));
    let blocks = parse_blocks(&encoded).unwrap();
    let keys: Vec<(u16, u16)> = blocks.iter().map(|b| (b.cell_y, b.cell_x)).collect();
    assert!(
        keys.windows(2).all(|w| w[0] <= w[1]),
        "row-major cell order"
    );
    assert!(
        ids.iter()
            .all(|batch| batch.windows(2).all(|w| w[0] < w[1])),
        "input order inside a cell"
    );
}

#[test]
fn empty_input_single_empty_batch_no_key() {
    let (schema, cols, _) = synthetic(0, 0.01);
    let (schema, batches) = blocked_by_z14_cell(schema, cols, &[]).unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_rows(), 0);
    assert!(!schema.metadata().contains_key(QM_BLOCKS_KEY));
}

#[test]
fn existing_metadata_preserved() {
    let (schema, cols, bboxes) = synthetic(10, 0.01);
    let schema = schema
        .with_metadata([("buildings_contract".to_string(), "buildings_v2".to_string())].into());
    let (schema, _) = blocked_by_z14_cell(schema, cols, &bboxes).unwrap();
    assert_eq!(
        schema
            .metadata()
            .get("buildings_contract")
            .map(String::as_str),
        Some("buildings_v2")
    );
    assert!(schema.metadata().contains_key(QM_BLOCKS_KEY));
}

#[test]
fn parse_blocks_rejects_malformed_and_round_trips_a_record() {
    let block = Block {
        cell_x: 8_823,
        cell_y: 5_593,
        bbox: [49.5, 13.9, 50.1, 14.6],
        alt_m: [-12.5, 3_050.0],
    };
    let encoded = encode_blocks(&[block]);
    assert_eq!(parse_blocks(&encoded).unwrap(), vec![block]);
    assert_eq!(parse_blocks(&encode_blocks(&[])).unwrap(), vec![]);
    assert!(parse_blocks("not base64!").is_none());
    assert!(parse_blocks("").is_none());
    assert!(
        parse_blocks(&BASE64.encode([2u8])).is_none(),
        "unknown version"
    );
    assert!(
        parse_blocks(&encoded[..encoded.len() - 4]).is_none(),
        "truncated record"
    );
    let inverted = Block {
        bbox: [50.1, 13.9, 49.5, 14.6],
        ..block
    };
    assert!(
        parse_blocks(&encode_blocks(&[inverted])).is_none(),
        "min_lat > max_lat"
    );
    let outside = Block {
        cell_x: 1 << BLOCK_ZOOM,
        ..block
    };
    assert!(
        parse_blocks(&encode_blocks(&[outside])).is_none(),
        "cell beyond the z14 axis"
    );
    let inverted_altitude = Block {
        alt_m: [3_050.0, -12.5],
        ..block
    };
    assert!(
        parse_blocks(&encode_blocks(&[inverted_altitude])).is_none(),
        "min_alt > max_alt"
    );
}

/// Surface writers stamp `0, 0`; the airborne writer's per-row ranges union per batch.
#[test]
fn block_altitude_range_is_the_union_of_its_rows_and_zero_for_surface_layers() {
    let (schema, cols, bboxes) = synthetic(9_700, 0.00001);
    let altitudes: Vec<RowAltitudeRange> = (0..9_700)
        .map(|i| [100.0 + i as f32, 200.0 + i as f32])
        .collect();
    let (schema, batches) =
        blocked_by_z14_cell_with_altitude(schema.clone(), cols.clone(), &bboxes, &altitudes)
            .unwrap();
    assert_eq!(
        blocks_of(&schema)
            .iter()
            .map(|b| b.alt_m)
            .collect::<Vec<_>>(),
        [[100.0, 4_295.0], [4_196.0, 8_391.0], [8_292.0, 9_899.0]]
    );
    assert_eq!(batches.len(), 3);
    let (schema, _) = blocked_by_z14_cell(schema.as_ref().clone(), cols, &bboxes).unwrap();
    assert!(blocks_of(&schema).iter().all(|b| b.alt_m == [0.0, 0.0]));
    let (schema, cols, bboxes) = synthetic(3, 0.01);
    assert!(blocked_by_z14_cell_with_altitude(schema, cols, &bboxes, &[[0.0; 2]; 2]).is_err());
}

#[test]
fn bbox_distance_zero_inside_positive_outside() {
    let bb: RowBbox = [50.0, 14.0, 50.1, 14.2];
    assert_eq!(point_to_bbox_distance_m(50.05, 14.1, &bb), 0.0);
    let d = point_to_bbox_distance_m(50.05, 14.35, &bb); // ~0.15° lon east ≈ 10.7 km
    assert!((9_000.0..12_500.0).contains(&d), "d={d}");
}

/// A click beside the antimeridian measures a box on the other side of the
/// seam by the short arc: a seam z14 cell is ~1 km away, not one box width.
#[test]
fn bbox_distance_crosses_the_antimeridian_by_the_short_arc() {
    let seam_cell: RowBbox = [0.0, -180.0, 0.01, -179.98];
    let d = point_to_bbox_distance_m(0.0, 179.99, &seam_cell);
    assert!((1_000.0..1_300.0).contains(&d), "d={d}");
    let (x, _) = z14_cell_of_bbox_midpoint(&[0.0, 179.99, 0.01, -179.99]);
    assert!(
        x == 0 || x == (1 << BLOCK_ZOOM) - 1,
        "seam geometry filed at x={x}"
    );
}
