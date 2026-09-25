//! Write one z9 control-point table with one row per exact node-to-way incidence.

use anyhow::{ensure, Result};
use arrow::{array::*, datatypes::*};
use std::{path::Path, sync::Arc};

pub(super) fn write_transport_nodes(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let mut id = Int64Builder::new();
    let mut gx = Int32Builder::new();
    let mut gy = Int32Builder::new();
    let mut way = Int64Builder::new();
    let mut family = StringBuilder::new();
    let mut vertex = UInt32Builder::new();
    let mut metres = Float64Builder::new();
    let mut tags = StringBuilder::new();
    let mut boxes = Vec::new();
    for row in rows {
        ensure!(row.len() == 9, "malformed transport control point");
        id.append_value(row[1].parse::<i64>()?);
        let (x, y) = (row[2].parse::<i32>()?, row[3].parse::<i32>()?);
        gx.append_value(x);
        gy.append_value(y);
        boxes.push(super::polygon_row_bbox(None, x, y));
        way.append_option(if row[4].is_empty() {
            None
        } else {
            Some(row[4].parse::<i64>()?)
        });
        family.append_value(&row[5]);
        vertex.append_option(if row[6].is_empty() {
            None
        } else {
            Some(row[6].parse::<u32>()?)
        });
        metres.append_option(if row[7].is_empty() {
            None
        } else {
            Some(row[7].parse::<f64>()?)
        });
        let _: crate::classify::Tags = serde_json::from_str(&row[8])?;
        tags.append_value(&row[8]);
    }
    let schema = super::schema_with_contract(
        vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("gx", DataType::Int32, false),
            Field::new("gy", DataType::Int32, false),
            Field::new("way_id", DataType::Int64, true),
            Field::new("family", DataType::Utf8, false),
            Field::new("vertex_index", DataType::UInt32, true),
            Field::new("way_m", DataType::Float64, true),
            Field::new("osm_tags", DataType::Utf8, false),
        ],
        "transport_nodes_contract",
        square_store::osm_contract::contract("transport_nodes")
            .unwrap()
            .1,
    );
    super::write_arrow_z14_blocked(
        path,
        schema,
        vec![
            Arc::new(id.finish()),
            Arc::new(gx.finish()),
            Arc::new(gy.finish()),
            Arc::new(way.finish()),
            Arc::new(family.finish()),
            Arc::new(vertex.finish()),
            Arc::new(metres.finish()),
            Arc::new(tags.finish()),
        ],
        &boxes,
    )
}
