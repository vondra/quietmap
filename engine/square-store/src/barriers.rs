//! Wall listing off the merged structure table: kind=1 rows with their
//! 2-point grid geometry, midpoint and height. Response fields keep the old
//! barriers.arrow shape so popup JSON is unchanged.

use super::geo::flat_dist;
use super::grid_cols::*;
use super::store::STRUCTURE_KIND_BARRIER;
use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use std::collections::BTreeMap;

/// Provenance range for a wall identity `(osm_id, segment_idx)`: OSM ids are
/// non-negative and below 2^47 (mirrors the old ScreeningSourceId::wall
/// range check — the dedupe key below is the tuple itself).
const WALL_OSM_ID_LIMIT: i64 = 1_i64 << 47;

fn check_wall_provenance(osm_id: i64, segment_idx: i16) -> Result<(), String> {
    if !(0..WALL_OSM_ID_LIMIT).contains(&osm_id) {
        return Err(format!(
            "invalid structures.arrow provenience ({osm_id}, {segment_idx}): osm_id out of range"
        ));
    }
    Ok(())
}

/// One wall microsegment for the popup lane.
#[derive(Debug, serde::Serialize)]
pub struct BarrierResult {
    pub osm_id: i64,
    #[serde(skip_serializing)]
    pub segment_idx: i16,
    pub height: f32,
    /// Segment midpoint (`dist_m`'s reference point).
    pub lat: f64,
    pub lon: f64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub dist_m: f64,
}

pub fn query_barriers_from_batches(
    batches: &[RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Result<Vec<BarrierResult>, String> {
    let mut results = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let kind = col_u8(batch, "kind")
            .ok_or_else(|| "structures.arrow missing required kind column".to_string())?;
        let osm_id = col_i64(batch, "osm_id")
            .ok_or_else(|| "structures.arrow missing required osm_id column".to_string())?;
        let segment_idx = col_i16(batch, "segment_idx")
            .ok_or_else(|| "structures.arrow missing required segment_idx column".to_string())?;
        let height = col_f32(batch, "height_m")
            .ok_or_else(|| "structures.arrow missing required height_m column".to_string())?;
        let geometry = col_binary(batch, "geom")
            .ok_or_else(|| "structures.arrow missing required geom column".to_string())?;
        let cgx = col_i32(batch, "centroid_gx")
            .ok_or_else(|| "structures.arrow missing required centroid_gx column".to_string())?;
        let cgy = col_i32(batch, "centroid_gy")
            .ok_or_else(|| "structures.arrow missing required centroid_gy column".to_string())?;

        for i in 0..n {
            if kind.value(i) != STRUCTURE_KIND_BARRIER {
                continue;
            }
            // A wall without its provenance or shape cannot be listed: nulls
            // here are a broken extract, and defaults would silently read the
            // identity of wall (0, 0).
            if osm_id.is_null(i) || segment_idx.is_null(i) || geometry.is_null(i) {
                return Err(format!(
                    "structures.arrow barrier row {i} lacks osm_id, segment_idx or geom"
                ));
            }
            check_wall_provenance(osm_id.value(i), segment_idx.value(i))?;
            let (mid_lon, mid_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
            let dist = flat_dist(lat, lon, mid_lat, mid_lon);
            if dist > max_radius {
                continue;
            }
            let ring = decode_geom(Some(geometry.value(i))).ok_or_else(|| {
                format!("structures.arrow barrier row {i}: geom is not a wall microsegment")
            })?;
            if ring.len() < 2 {
                return Err(format!(
                    "structures.arrow barrier row {i}: geom is not a wall microsegment"
                ));
            }
            let (s_lon, s_lat) = grid_cell_lonlat(ring[0].0, ring[0].1);
            let (e_lon, e_lat) = grid_cell_lonlat(ring[ring.len() - 1].0, ring[ring.len() - 1].1);

            results.push(BarrierResult {
                osm_id: osm_id.value(i),
                segment_idx: segment_idx.value(i),
                height: height.value(i),
                lat: mid_lat,
                lon: mid_lon,
                start_lat: s_lat,
                start_lon: s_lon,
                end_lat: e_lat,
                end_lon: e_lon,
                dist_m: dist,
            });
        }
    }

    canonicalize_barrier_results(results)
}

/// Stable-dedupe exact repeated emissions and reject one ID naming two shapes.
pub fn canonicalize_barrier_results(
    results: Vec<BarrierResult>,
) -> Result<Vec<BarrierResult>, String> {
    let mut seen = BTreeMap::new();
    let mut unique = Vec::with_capacity(results.len());
    for result in results {
        check_wall_provenance(result.osm_id, result.segment_idx)?;
        let geometry_bits = [
            result.start_lat.to_bits(),
            result.start_lon.to_bits(),
            result.end_lat.to_bits(),
            result.end_lon.to_bits(),
            u64::from(result.height.to_bits()),
        ];
        match seen.entry((result.osm_id, result.segment_idx)) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(geometry_bits);
                unique.push(result);
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if *entry.get() == geometry_bits => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(format!(
                    "barrier provenience ({}, {}) names different geometry",
                    result.osm_id, result.segment_idx
                ));
            }
        }
    }
    Ok(unique)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::writer::FileWriter;
    use std::fs::File;
    use std::sync::Arc;

    /// One wall row per (osm_id, segment_idx): a microsegment running NE from
    /// a start lon/lat. Plus an optional building row (kind=0, never listed).
    fn wall_batch(walls: &[(i64, i16, f64, f64)], with_building: bool) -> RecordBatch {
        let mut kind = vec![];
        let mut osm_id = vec![];
        let mut seg = vec![];
        let mut height = vec![];
        let mut geom: Vec<Option<Vec<u8>>> = vec![];
        let mut cgx = vec![];
        let mut cgy = vec![];
        for (id, idx, slat, slon) in walls.iter().copied() {
            let (s_gx, s_gy) = grid::lonlat_to_grid(slon, slat);
            let (e_gx, e_gy) = grid::lonlat_to_grid(slon + 0.001, slat + 0.001);
            kind.push(STRUCTURE_KIND_BARRIER);
            osm_id.push(id);
            seg.push(idx);
            height.push(3.0);
            geom.push(Some(grid::poly::encode_grid_poly(&[
                (s_gx, s_gy),
                (e_gx, e_gy),
            ])));
            cgx.push((s_gx + e_gx) / 2);
            cgy.push((s_gy + e_gy) / 2);
        }
        if with_building {
            kind.push(0);
            osm_id.push(42);
            seg.push(0);
            height.push(8.0);
            geom.push(None);
            cgx.push(0);
            cgy.push(0);
        }
        let schema = Schema::new(vec![
            Field::new("kind", DataType::UInt8, false),
            Field::new("osm_id", DataType::Int64, true),
            Field::new("segment_idx", DataType::Int16, true),
            Field::new("height_m", DataType::Float32, false),
            Field::new("geom", DataType::Binary, true),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(UInt8Array::from(kind)),
                Arc::new(Int64Array::from(osm_id)),
                Arc::new(Int16Array::from(seg)),
                Arc::new(Float32Array::from(height)),
                Arc::new(BinaryArray::from_opt_vec(
                    geom.iter().map(|g| g.as_deref()).collect(),
                )),
                Arc::new(Int32Array::from(cgx)),
                Arc::new(Int32Array::from(cgy)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn wall_listing_preserves_provenance_and_ignores_buildings() {
        let results = query_barriers_from_batches(
            &[wall_batch(
                &[(7, -3, 50.0, 14.0), (7, 4, 50.01, 14.01)],
                true,
            )],
            50.0,
            14.0,
            200_000.0,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].segment_idx, -3);
        assert_eq!(results[1].segment_idx, 4);
        assert!((results[0].start_lon - 14.0).abs() < 0.001);
    }

    #[test]
    fn identical_dupes_merge_conflicting_geometry_fails() {
        let identical = query_barriers_from_batches(
            &[wall_batch(
                &[(7, -3, 50.0, 14.0), (7, -3, 50.0, 14.0)],
                false,
            )],
            50.0,
            14.0,
            200_000.0,
        )
        .unwrap();
        assert_eq!(identical.len(), 1);

        // Same provenance rebuilt from identical inputs merges fine.
        let rebuilt_identical = query_barriers_from_batches(
            &[
                wall_batch(&[(7, -3, 50.0, 14.0)], false),
                wall_batch(&[(7, -3, 50.0, 14.0)], false),
            ],
            50.0,
            14.0,
            200_000.0,
        );
        assert_eq!(rebuilt_identical.unwrap().len(), 1);
    }

    #[test]
    fn conflicting_geometry_for_one_id_fails() {
        let batch = wall_batch(&[(7, -3, 50.0, 14.0)], false);
        // Rewrite the endpoint cells so the same (7, -3) names another shape.
        let (e_gx, e_gy) = grid::lonlat_to_grid(15.0, 51.0);
        let new_bytes = grid::poly::encode_grid_poly(&[(0, 0), (e_gx, e_gy)]);
        let new_geom = BinaryArray::from_opt_vec(vec![Some(new_bytes.as_slice())]);
        let idx = batch.schema().index_of("geom").unwrap();
        let mut columns = batch.columns().to_vec();
        columns[idx] = Arc::new(new_geom);
        let rebuilt = RecordBatch::try_new(batch.schema(), columns).unwrap();
        let error = query_barriers_from_batches(
            &[wall_batch(&[(7, -3, 50.0, 14.0)], false), rebuilt],
            50.0,
            14.0,
            20_000_000.0,
        )
        .unwrap_err();
        assert!(error.contains("names different geometry"), "got: {error}");
    }

    #[test]
    fn missing_segment_idx_fails_closed() {
        let batch = wall_batch(&[(7, -3, 50.0, 14.0)], false);
        let idx = batch.schema().index_of("segment_idx").unwrap();
        let mut columns = batch.columns().to_vec();
        columns.remove(idx);
        let fields: Vec<Field> = batch
            .schema()
            .fields()
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, f)| f.as_ref().clone())
            .collect();
        let rebuilt = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
        let error = query_barriers_from_batches(&[rebuilt], 50.0, 14.0, 1_000.0).unwrap_err();
        assert!(error.contains("missing required segment_idx"));
    }

    #[test]
    fn load_square_rejects_unstamped_structures() {
        let root = std::env::temp_dir().join(format!("square-store-test-{}", std::process::id()));
        let dir = root.join("z9").join("276").join("173");
        std::fs::create_dir_all(&dir).unwrap();
        // Unstamped (no contracts metadata) empty table.
        let schema = Arc::new(Schema::new(vec![Field::new(
            "kind",
            DataType::UInt8,
            false,
        )]));
        let f = File::create(dir.join("structures.arrow")).unwrap();
        let mut w = FileWriter::try_new(f, &schema).unwrap();
        w.finish().unwrap();
        let error = super::super::store::load_square(&dir)
            .err()
            .expect("unstamped table must fail");
        assert!(
            error.contains("structures_contract mismatch"),
            "got: {error}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
