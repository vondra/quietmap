//! `industrial.arrow` writer: one row per industrial/power site footprint with
//! source type + optional hub-height/rated-power. See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use grid::poly::{encode_grid_poly, ring_area_m2};
use std::path::Path;
use std::sync::Arc;

use super::{
    decode_tsv_ring, parse_grid_cell, polygon_row_bbox, write_arrow_spatially_batched,
};

pub(super) fn write_industrial(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("centroid_gx", DataType::Int32, false),
        Field::new("centroid_gy", DataType::Int32, false),
        Field::new("source_type", DataType::UInt8, false),
        Field::new("site_subtype", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("hub_height", DataType::Float32, true),
        Field::new("rated_power_kw", DataType::Float32, true),
        Field::new("geom", DataType::Binary, true),
        Field::new("area_m2", DataType::Float32, true),
        // Dataset provenance — 0 = unspecified, populated by `enrich-industrial-*.ts`
        // scripts writing directly to this Arrow file (paired with nace_4digit below).
        Field::new("source_id", DataType::UInt16, false),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut cgx = Int32Builder::with_capacity(n);
    let mut cgy = Int32Builder::with_capacity(n);
    let mut stype = UInt8Builder::with_capacity(n);
    let mut subtype = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 10);
    let mut hub_h = Float32Builder::with_capacity(n);
    let mut power = Float32Builder::with_capacity(n);
    let mut geom = BinaryBuilder::with_capacity(n, n * 100);
    let mut ind_area = Float32Builder::with_capacity(n);
    let mut source_id = UInt16Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) c_gx(2) c_gy(3) stype(4) subtype(5) name(6)
        //      hub_h(7) power(8) ring(9)
        if row.len() < 9 {
            continue;
        }
        let c_gx = parse_grid_cell(&row[2]);
        let c_gy = parse_grid_cell(&row[3]);
        let ring = decode_tsv_ring(row.get(9).map(|s| s.as_str()).unwrap_or(""));
        row_bboxes.push(polygon_row_bbox(ring.as_deref(), c_gx, c_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        cgx.append_value(c_gx);
        cgy.append_value(c_gy);
        stype.append_value(row[4].parse().unwrap_or(0));
        subtype.append_value(row[5].parse().unwrap_or(0));
        name.append_value(row.get(6).unwrap_or(&String::new()));
        let h: f32 = row.get(7).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if h > 0.0 {
            hub_h.append_value(h);
        } else {
            hub_h.append_null();
        }
        let p: f32 = row.get(8).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if p > 0.0 {
            power.append_value(p);
        } else {
            power.append_null();
        }
        match ring {
            Some(ring) => {
                match ring_area_m2(&ring) {
                    Some(a) => ind_area.append_value(a as f32),
                    None => ind_area.append_null(),
                }
                geom.append_value(encode_grid_poly(&ring));
            }
            None => {
                geom.append_null();
                ind_area.append_null();
            }
        }
        source_id.append_value(0);
    }

    write_arrow_spatially_batched(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(cgx.finish()),
            Arc::new(cgy.finish()),
            Arc::new(stype.finish()),
            Arc::new(subtype.finish()),
            Arc::new(name.finish()),
            Arc::new(hub_h.finish()),
            Arc::new(power.finish()),
            Arc::new(geom.finish()),
            Arc::new(ind_area.finish()),
            Arc::new(source_id.finish()),
        ],
        &row_bboxes,
    )
}
