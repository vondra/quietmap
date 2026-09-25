//! `leisure_v4` writer: area and point sources plus retained motorsport lines,
//! shooting subtypes and indoor flags. Activity evidence never screens.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use grid::poly::{encode_grid_poly, ring_area_m2};
use std::path::Path;
use std::sync::Arc;

use super::{
    evidence::write_with_evidence, parse_grid_cell, polygon_row_bbox, schema_with_contract,
};
use square_store::osm_contract::LEISURE_CONTRACT_V4;

pub(super) fn write_leisure(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = schema_with_contract(
        vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            // emission leisure class id (PITCH/PADEL/…).
            Field::new("sport", DataType::UInt8, false),
            Field::new("opening_hours_frac", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("geom", DataType::Binary, true),
            Field::new("area_m2", DataType::Float32, true),
            Field::new("geometry_kind", DataType::UInt8, false),
            Field::new("length_m", DataType::Float32, true),
        ],
        "leisure_contract",
        LEISURE_CONTRACT_V4,
    );

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut cgx = Int32Builder::with_capacity(n);
    let mut cgy = Int32Builder::with_capacity(n);
    let mut sport = UInt8Builder::with_capacity(n);
    let mut opening = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 8);
    let mut geom = BinaryBuilder::with_capacity(n, n * 100);
    let mut area_m2 = Float32Builder::with_capacity(n);
    let mut geometry_kind = UInt8Builder::new();
    let mut length = Float32Builder::new();
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) c_gx(2) c_gy(3) sport(4) opening_hours(5)
        //      name(6) ring(7) osm_tags(8) osm_kind(9) line(10)
        if row.len() < 7 {
            continue;
        }
        let c_gx = parse_grid_cell(&row[2]);
        let c_gy = parse_grid_cell(&row[3]);
        anyhow::ensure!(row.len() > 10, "old or truncated leisure spill");
        let ring: Option<Vec<(i32, i32)>> = row[7]
            .split(';')
            .map(|point| {
                let (x, y) = point.split_once(',')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            })
            .collect();
        let line = match row[10].as_str() {
            "0" => false,
            "1" => true,
            _ => anyhow::bail!("invalid leisure line marker"),
        };
        geometry_kind.append_value(if ring.is_none() {
            0
        } else if line {
            2
        } else {
            1
        });
        length.append_option(ring.as_ref().filter(|_| line).map(|chain| {
            chain
                .windows(2)
                .map(|pair| {
                    let a = square_store::grid_cols::grid_cell_lonlat(pair[0].0, pair[0].1);
                    let b = square_store::grid_cols::grid_cell_lonlat(pair[1].0, pair[1].1);
                    grid::geo::flat_dist(a.1, a.0, b.1, b.0)
                })
                .sum::<f64>() as f32
        }));
        row_bboxes.push(polygon_row_bbox(ring.as_deref(), c_gx, c_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        cgx.append_value(c_gx);
        cgy.append_value(c_gy);
        sport.append_value(row[4].parse().unwrap_or(0));
        opening.append_value(row.get(5).and_then(|s| s.parse().ok()).unwrap_or(0));
        name.append_value(row.get(6).unwrap_or(&String::new()));
        match ring {
            Some(ring) => {
                match (!line).then(|| ring_area_m2(&ring)).flatten() {
                    Some(a) => area_m2.append_value(a as f32),
                    None => area_m2.append_null(),
                }
                geom.append_value(encode_grid_poly(&ring));
            }
            None => {
                geom.append_null();
                area_m2.append_null();
            }
        }
    }

    write_with_evidence(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(cgx.finish()),
            Arc::new(cgy.finish()),
            Arc::new(sport.finish()),
            Arc::new(opening.finish()),
            Arc::new(name.finish()),
            Arc::new(geom.finish()),
            Arc::new(area_m2.finish()),
            Arc::new(geometry_kind.finish()),
            Arc::new(length.finish()),
        ],
        &row_bboxes,
        rows,
        8,
    )
}
