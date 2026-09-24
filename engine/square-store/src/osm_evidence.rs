//! Strict readers for retained OSM tags, whole-way extents and control-point incidences.

use crate::grid_cols::*;
use arrow::{array::Array, record_batch::RecordBatch};

pub struct ControlPoint<'a> {
    pub node_id: i64,
    pub grid: (i32, i32),
    pub way_id: Option<i64>,
    pub family: &'a str,
    pub vertex_index: Option<u32>,
    pub way_m: Option<f64>,
    pub tags_json: &'a str,
}

pub fn control_points(batch: &RecordBatch) -> Result<Vec<ControlPoint<'_>>, String> {
    crate::osm_contract::validate(&batch.schema(), "transport_nodes")?;
    let id = col_i64(batch, "osm_id").ok_or("missing control node id")?;
    let x = col_i32(batch, "gx").ok_or("missing control gx")?;
    let y = col_i32(batch, "gy").ok_or("missing control gy")?;
    let way = col_i64(batch, "way_id").ok_or("missing control way")?;
    let family = col_str(batch, "family").ok_or("missing control family")?;
    let vertex = col_u32(batch, "vertex_index").ok_or("missing control vertex")?;
    let metres = col_f64(batch, "way_m").ok_or("missing control metres")?;
    let tags = col_str(batch, "osm_tags").ok_or("missing control tags")?;
    if [
        id.null_count(),
        x.null_count(),
        y.null_count(),
        family.null_count(),
        tags.null_count(),
    ]
    .iter()
    .any(|n| *n != 0)
    {
        return Err("null control point identity, coordinates or tags".into());
    }
    (0..batch.num_rows())
        .map(|row| {
            let linked = !way.is_null(row);
            if linked == vertex.is_null(row)
                || (linked && !matches!(family.value(row), "roads" | "railways"))
                || (!linked && (!family.value(row).is_empty() || !metres.is_null(row)))
                || (!metres.is_null(row)
                    && (!metres.value(row).is_finite() || metres.value(row) < 0.0))
            {
                return Err("invalid control point incidence".into());
            }
            Ok(ControlPoint {
                node_id: id.value(row),
                grid: (x.value(row), y.value(row)),
                way_id: linked.then(|| way.value(row)),
                family: family.value(row),
                vertex_index: linked.then(|| vertex.value(row)),
                way_m: (!metres.is_null(row)).then(|| metres.value(row)),
                tags_json: tags.value(row),
            })
        })
        .collect()
}

pub fn tags<'a>(batch: &'a RecordBatch, family: &str, row: usize) -> Result<&'a str, String> {
    crate::osm_contract::validate(&batch.schema(), family)?;
    let tags = col_str(batch, "osm_tags").ok_or("missing retained OSM tags")?;
    if row >= tags.len() || tags.is_null(row) {
        return Err("missing retained OSM row".into());
    }
    Ok(tags.value(row))
}
