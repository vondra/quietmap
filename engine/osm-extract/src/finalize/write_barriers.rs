//! `barriers.arrow` writer: one row per barrier microsegment (walls/fences/noise
//! barriers) with geometry + height + material + height tier (0 mapped /
//! 2 defaulted — the structure-table ladder tier the merge carries through).
//! See `finalize_bucket` dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use std::path::Path;
use std::sync::Arc;

use super::{segment_row_bbox, write_arrow_spatially_batched};

pub(super) fn write_barriers(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_lat", DataType::Float64, false),
        Field::new("start_lon", DataType::Float64, false),
        Field::new("end_lat", DataType::Float64, false),
        Field::new("end_lon", DataType::Float64, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("height", DataType::Float32, false),
        Field::new("material", DataType::UInt8, false),
        Field::new("height_tier", DataType::UInt8, false),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut slat = Float64Builder::with_capacity(n);
    let mut slon = Float64Builder::with_capacity(n);
    let mut elat = Float64Builder::with_capacity(n);
    let mut elon = Float64Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut height = Float32Builder::with_capacity(n);
    let mut material = UInt8Builder::with_capacity(n);
    let mut height_tier = UInt8Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        if row.len() < 10 {
            continue;
        }
        let s_lat: f64 = row[3].parse().unwrap_or(0.0);
        let s_lon: f64 = row[4].parse().unwrap_or(0.0);
        let e_lat: f64 = row[5].parse().unwrap_or(0.0);
        let e_lon: f64 = row[6].parse().unwrap_or(0.0);
        row_bboxes.push(segment_row_bbox(s_lat, s_lon, e_lat, e_lon));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        seg_idx.append_value(row[2].parse().unwrap_or(0));
        slat.append_value(s_lat);
        slon.append_value(s_lon);
        elat.append_value(e_lat);
        elon.append_value(e_lon);
        len.append_value(row[7].parse().unwrap_or(0.0));
        height.append_value(row[8].parse().unwrap_or(3.0));
        material.append_value(row[9].parse().unwrap_or(0));
        // The tier is spilled with the row; a pre-tier TSV (a finalize rerun
        // over an old spill) marks the defaulted height as tier 2.
        height_tier.append_value(row.get(10).and_then(|v| v.parse().ok()).unwrap_or(2));
    }

    write_arrow_spatially_batched(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(seg_idx.finish()),
            Arc::new(slat.finish()),
            Arc::new(slon.finish()),
            Arc::new(elat.finish()),
            Arc::new(elon.finish()),
            Arc::new(len.finish()),
            Arc::new(height.finish()),
            Arc::new(material.finish()),
            Arc::new(height_tier.finish()),
        ],
        &row_bboxes,
    )
}
