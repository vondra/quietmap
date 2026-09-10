//! Stage 2B cruise writer (schema v14).
//!
//! Rev 2 of the cruise rewrite: replaces v13's per-fid lists with a
//! bounded top-K `top_candidates` struct list + scalar `unique_count`
//! so per-row size stays bounded regardless of `n_days`.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    ArrayRef, FixedSizeBinaryBuilder, Float32Builder, Float64Builder, ListArray, StringBuilder,
    StructArray, UInt32Builder, UInt64Builder, UInt8Builder,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field};

use crate::arrow_schemas;
use crate::flight::CruiseBucket;

use super::write_record_batches;

/// One owner z9's canonical buckets. Batches carry the synthetic cruise line
/// envelopes so a receiver prunes them by distance like airborne batches.
pub fn write_cruise(path: &Path, rows: &[CruiseBucket], n_days: u16) -> Result<()> {
    let (schema, columns, bboxes) = cruise_columns(rows, n_days)?;
    let (schema, batches) = arrow_batching::spatially_batched(schema, columns, &bboxes)?;
    write_record_batches(path, &schema, &batches)
}

/// Envelope of the synthetic line the popup centres on the bucket.
fn cruise_row_bbox(lat: f64, lon: f64, rep_len_m: f32) -> arrow_batching::RowBbox {
    let (lat_off, lon_off) = noise_compute::compute::aircraft_v6::cruise::cruise_synth_offsets(
        lat,
        f64::from(rep_len_m.max(noise_compute::compute::aircraft_v6::cruise::SLANT_FLOOR_M as f32))
            * 0.5,
    );
    let (west, east) = if lon - lon_off < -180.0 || lon + lon_off > 180.0 {
        (-180.0, 180.0)
    } else {
        (lon - lon_off, lon + lon_off)
    };
    [
        (lat - lat_off).max(-90.0),
        west,
        (lat + lat_off).min(90.0),
        east,
    ]
}

type CruiseColumns = (
    arrow::datatypes::Schema,
    Vec<ArrayRef>,
    Vec<arrow_batching::RowBbox>,
);

fn cruise_columns(rows: &[CruiseBucket], n_days: u16) -> Result<CruiseColumns> {
    let schema = arrow_schemas::with_n_days(arrow_schemas::cruise_schema(), n_days);
    let mut bboxes = Vec::with_capacity(rows.len());
    let n = rows.len();
    let mut lon = Float64Builder::with_capacity(n);
    let mut lat = Float64Builder::with_capacity(n);
    let mut class = UInt8Builder::with_capacity(n);
    let mut rep_pi = UInt8Builder::with_capacity(n);
    let mut fl_bin = UInt8Builder::with_capacity(n);
    let mut period = UInt8Builder::with_capacity(n);
    let mut sum_len = Float32Builder::with_capacity(n);
    let mut rep_len = Float32Builder::with_capacity(n);
    let mut rep_alt = Float32Builder::with_capacity(n);
    let mut rep_speed = Float32Builder::with_capacity(n);
    let mut unique_count = UInt32Builder::with_capacity(n);
    let mut source_id = UInt8Builder::with_capacity(n);
    let mut origin = UInt8Builder::with_capacity(n);

    // Flatten top_candidates lists. The list-of-struct uses parallel
    // child arrays sharing one offset buffer.
    let total_cands: usize = rows.iter().map(|r| r.top_candidates.len()).sum();
    let mut cand_off: Vec<i32> = Vec::with_capacity(n + 1);
    cand_off.push(0);
    let mut cand_fid = UInt64Builder::with_capacity(total_cands);
    let mut cand_callsign = StringBuilder::with_capacity(total_cands, total_cands * 8);
    let mut cand_typecode = FixedSizeBinaryBuilder::with_capacity(total_cands, 4);
    let mut cand_lmax = Float32Builder::with_capacity(total_cands);
    let mut cand_alt = Float32Builder::with_capacity(total_cands);
    let mut running = 0usize;

    for r in rows {
        anyhow::ensure!(
            r.rep_len_m.is_finite()
                && r.rep_len_m <= noise_compute::emission::aircraft::CRUISE_MAX_REP_LEN_M,
            "cruise rep_len_m {} exceeds the query radius contract",
            r.rep_len_m
        );
        let center = grid::cruise::cruise_centroid(r.cruise_cell_id);
        bboxes.push(cruise_row_bbox(center.1, center.0, r.rep_len_m));
        lon.append_value(center.0);
        lat.append_value(center.1);
        class.append_value(r.class);
        rep_pi.append_value(r.rep_profile_idx);
        fl_bin.append_value(r.fl_bin);
        period.append_value(r.period);
        sum_len.append_value(r.sum_length_m);
        rep_len.append_value(r.rep_len_m);
        rep_alt.append_value(r.rep_alt_m);
        rep_speed.append_value(r.rep_speed_kt);
        unique_count.append_value(r.unique_count);
        source_id.append_value(r.source_id);
        origin.append_value(r.origin);
        for cand in &r.top_candidates {
            cand_fid.append_value(cand.flight_id);
            cand_callsign.append_value(&cand.callsign);
            cand_typecode.append_value(cand.aircraft_type)?;
            cand_lmax.append_value(cand.peak_lmax_25m_db);
            cand_alt.append_value(cand.altitude_m);
        }
        running += r.top_candidates.len();
        cand_off.push(running as i32);
    }

    let cand_struct_fields = arrow_schemas::cruise_top_candidate_fields();
    let cand_struct = StructArray::new(
        cand_struct_fields.clone(),
        vec![
            Arc::new(cand_fid.finish()),
            Arc::new(cand_callsign.finish()),
            Arc::new(cand_typecode.finish()),
            Arc::new(cand_lmax.finish()),
            Arc::new(cand_alt.finish()),
        ],
        None,
    );
    let cand_list = ListArray::new(
        Arc::new(Field::new(
            "item",
            DataType::Struct(cand_struct_fields),
            false,
        )),
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(cand_off)),
        Arc::new(cand_struct),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(lon.finish()),
        Arc::new(lat.finish()),
        Arc::new(class.finish()),
        Arc::new(rep_pi.finish()),
        Arc::new(fl_bin.finish()),
        Arc::new(period.finish()),
        Arc::new(sum_len.finish()),
        Arc::new(rep_len.finish()),
        Arc::new(rep_alt.finish()),
        Arc::new(rep_speed.finish()),
        Arc::new(unique_count.finish()),
        Arc::new(cand_list),
        Arc::new(source_id.finish()),
        Arc::new(origin.finish()),
    ];
    Ok((schema.as_ref().clone(), columns, bboxes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::read_record_batches;
    use crate::flight::CruiseTopCandidate;
    use tempfile::tempdir;

    fn sample_bucket() -> CruiseBucket {
        CruiseBucket {
            cruise_cell_id: 0xABC,
            class: 5,
            rep_profile_idx: 7,
            fl_bin: 3,
            period: 0,
            sum_length_m: 5000.0,
            rep_len_m: 1500.0,
            rep_alt_m: 11_000.0,
            rep_speed_kt: 460.0,
            unique_count: 3,
            top_candidates: vec![
                CruiseTopCandidate {
                    flight_id: 1,
                    callsign: "CSA001".into(),
                    aircraft_type: *b"A320",
                    peak_lmax_25m_db: 95.0,
                    altitude_m: 11_000.0,
                },
                CruiseTopCandidate {
                    flight_id: 2,
                    callsign: "RYR123".into(),
                    aircraft_type: *b"B738",
                    peak_lmax_25m_db: 93.5,
                    altitude_m: 10_500.0,
                },
                CruiseTopCandidate {
                    flight_id: 3,
                    callsign: "LH1234".into(),
                    aircraft_type: *b"E190",
                    peak_lmax_25m_db: 90.2,
                    altitude_m: 11_200.0,
                },
            ],
            source_id: 0,
            origin: 0,
        }
    }

    #[test]
    fn cruise_round_trip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("cruise.arrow");
        let cs = vec![sample_bucket()];
        write_cruise(&p, &cs, 1).unwrap();
        let (_, batches) = read_record_batches(&p).unwrap();
        assert_eq!(batches[0].num_rows(), 1);
    }

    /// Batches carry synthetic-line envelopes, large owners split into
    /// several batches, and a row beyond the query radius contract is refused.
    #[test]
    fn cruise_batches_carry_line_envelopes_and_reject_overlong_rows() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("cruise.arrow");
        let rows: Vec<_> = (0..(arrow_batching::TARGET_ROWS_PER_BATCH + 1))
            .map(|i| CruiseBucket {
                cruise_cell_id: grid::cruise::cruise_cell_id(50.0 + i as f64 * 1e-4, 14.25),
                rep_len_m: 50_000.0,
                ..sample_bucket()
            })
            .collect();
        write_cruise(&p, &rows, 12).unwrap();
        let (schema, batches) = read_record_batches(&p).unwrap();
        assert_eq!(batches.len(), 2);
        let bboxes = arrow_batching::parse_batch_bboxes(
            schema.metadata().get(arrow_batching::QM_BATCH_BBOXES_KEY).unwrap(),
        )
        .unwrap();
        assert_eq!(bboxes.len(), 2);
        // A 50 km NE–SW line spans 25 km / √2 ≈ 17.7 km ≈ 0.159° on each side.
        for bb in &bboxes {
            assert!(bb[2] - bb[0] >= 0.3 && bb[2] - bb[0] < 1.0, "{bb:?}");
        }
        assert_eq!(schema.metadata().get("n_days").map(String::as_str), Some("12"));
        assert_eq!(
            cruise_row_bbox(0.0, 179.999, 5_000.0),
            [
                cruise_row_bbox(0.0, 0.0, 5_000.0)[0],
                -180.0,
                cruise_row_bbox(0.0, 0.0, 5_000.0)[2],
                180.0
            ]
        );
        let overlong = CruiseBucket {
            rep_len_m: 50_000.5,
            ..sample_bucket()
        };
        assert!(write_cruise(&p, &[overlong], 12).is_err());
    }

    #[test]
    fn cruise_schema_has_no_flags_column() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("cruise.arrow");
        let cs = vec![sample_bucket()];
        write_cruise(&p, &cs, 1).unwrap();
        let (schema, _) = read_record_batches(&p).unwrap();
        assert!(schema.field_with_name("unique_count").is_ok());
        assert!(schema.field_with_name("top_candidates").is_ok());
        assert!(schema.field_with_name("cruise_flight_ids").is_err());
        // v16 invariant: `flags` is gone (was always IS_DEPARTURE per Doc 29 §A.3.2).
        assert!(schema.field_with_name("flags").is_err());
        assert_eq!(
            schema.metadata().get("schema_version").map(String::as_str),
            Some(crate::SCHEMA_VERSION)
        );
    }

    /// Multi-row write: row 0 has 2 candidates, row 1 has 4; verify
    /// list offsets keep child arrays aligned (offset arithmetic bug
    /// would cross-contaminate rows).
    #[test]
    fn cruise_top_candidates_per_row_offsets() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("cruise.arrow");
        let row_a = CruiseBucket {
            unique_count: 2,
            top_candidates: vec![
                CruiseTopCandidate {
                    flight_id: 10,
                    callsign: "A10".into(),
                    aircraft_type: *b"A320",
                    peak_lmax_25m_db: 95.0,
                    altitude_m: 10_000.0,
                },
                CruiseTopCandidate {
                    flight_id: 11,
                    callsign: "A11".into(),
                    aircraft_type: *b"B738",
                    peak_lmax_25m_db: 90.0,
                    altitude_m: 9_500.0,
                },
            ],
            ..sample_bucket()
        };
        let row_b = CruiseBucket {
            cruise_cell_id: 0xDEF,
            unique_count: 4,
            top_candidates: (0..4)
                .map(|i| CruiseTopCandidate {
                    flight_id: 100 + i,
                    callsign: format!("B{i:02}"),
                    aircraft_type: *b"E190",
                    peak_lmax_25m_db: 80.0 + i as f32,
                    altitude_m: 11_000.0,
                })
                .collect(),
            ..sample_bucket()
        };
        write_cruise(&p, &[row_a, row_b], 1).unwrap();
        let (_, batches) = read_record_batches(&p).unwrap();
        assert_eq!(batches[0].num_rows(), 2);
        use arrow::array::Array;
        let unique = batches[0]
            .column_by_name("unique_count")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::UInt32Array>()
            .unwrap();
        assert_eq!(unique.value(0), 2);
        assert_eq!(unique.value(1), 4);
        let list = batches[0]
            .column_by_name("top_candidates")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        // Row 0 = 2 entries, Row 1 = 4 entries.
        assert_eq!(list.value(0).len(), 2);
        assert_eq!(list.value(1).len(), 4);
    }
}
