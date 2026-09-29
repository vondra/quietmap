//! Split at physical allocation boundaries and rebuild the native z14 batch index.

use crate::{allocation, input, spatial::RoadIndex};
use arrow::array::{ArrayRef, Float32Array, Float64Array, Int32Array, UInt32Array, UInt8Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use std::path::Path;
use std::sync::Arc;

struct Child {
    parent: u32,
    grid: [i32; 4],
    length: f32,
    counts: [f64; 4],
    estimated: u8,
    cross_section: f64,
}

fn expand(batch: &RecordBatch, index: &RoadIndex) -> Result<Vec<Child>, String> {
    let coordinates = ["start_gx", "start_gy", "end_gx", "end_gy"].map(|name| input::column::<Int32Array>(batch, name))
        .into_iter().collect::<Result<Vec<_>, _>>()?;
    let lengths = input::column::<Float32Array>(batch, "length_m")?;
    let mut children = Vec::new();
    for (parent, road) in input::roads(batch)?.iter().enumerate() {
        let candidates = index.alternatives(road);
        for (from, to, counts, estimated, cross_section) in allocation::intervals(road, &candidates) {
            let at = |axis: usize, t: f64| {
                let start = coordinates[axis].value(parent) as f64;
                let end = coordinates[axis + 2].value(parent) as f64;
                let world = f64::from(1_u32 << 30);
                let delta = end - start;
                let delta = if axis == 0 { delta - (delta / world).round() * world } else { delta };
                let value = (start + delta * t).round();
                (if axis == 0 { value.rem_euclid(world) } else { value }) as i32
            };
            let grid = [at(0, from), at(1, from), at(0, to), at(1, to)];
            if grid[0] == grid[2] && grid[1] == grid[3] { continue; }
            children.push(Child { parent: u32::try_from(parent).map_err(|e| e.to_string())?, grid,
                length: lengths.value(parent) * (to - from) as f32, counts, estimated, cross_section });
        }
    }
    Ok(children)
}

fn bbox(child: &Child) -> arrow_batching::RowBbox {
    let lon_lat = |gx, gy| {
        let (x, y) = grid::grid_to_meters(gx, gy);
        ((x / grid::WEB_MERCATOR_RADIUS_M).to_degrees(),
            (2.0 * (y / grid::WEB_MERCATOR_RADIUS_M).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees())
    };
    let a = lon_lat(child.grid[0], child.grid[1]);
    let b = lon_lat(child.grid[2], child.grid[3]);
    [a.1.min(b.1), a.0.min(b.0), a.1.max(b.1), a.0.max(b.0)]
}

pub fn stage(path: &Path, batches: &[RecordBatch], index: &RoadIndex) -> Result<(), String> {
    let original = batches.first().ok_or("road file has no schema")?.schema();
    let merged = concat_batches(&original, batches).map_err(|e| e.to_string())?;
    let children = expand(&merged, index)?;
    let indices = UInt32Array::from(children.iter().map(|c| c.parent).collect::<Vec<_>>());
    let dropped = ["traffic_estimated", "traffic_count_basis", "traffic_observation_id", "traffic_observation_source"];
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in original.fields() {
        let name = field.name().as_str();
        if input::COUNTS.contains(&name) || dropped.contains(&name) { continue; }
        let column: ArrayRef = if let Some(axis) = ["start_gx", "start_gy", "end_gx", "end_gy"].iter().position(|v| *v == name) {
            Arc::new(Int32Array::from(children.iter().map(|c| c.grid[axis]).collect::<Vec<_>>()))
        } else if name == "length_m" {
            Arc::new(Float32Array::from(children.iter().map(|c| c.length).collect::<Vec<_>>()))
        } else {
            take(merged.column_by_name(name).ok_or("road column missing")?.as_ref(), &indices, None).map_err(|e| e.to_string())?
        };
        fields.push(field.as_ref().clone());
        columns.push(column);
    }
    for (category, name) in input::COUNTS.iter().enumerate() {
        fields.push(Field::new(*name, DataType::Float64, false));
        columns.push(Arc::new(Float64Array::from(children.iter().map(|c| c.counts[category]).collect::<Vec<_>>())));
    }
    fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
    columns.push(Arc::new(UInt8Array::from(children.iter().map(|c| c.estimated).collect::<Vec<_>>())));
    fields.push(Field::new(input::CROSS_SECTION_AADT, DataType::Float64, false));
    columns.push(Arc::new(Float64Array::from(children.iter().map(|c| c.cross_section).collect::<Vec<_>>())));
    let mut metadata = original.metadata().clone();
    metadata.insert(input::CONTRACT.to_owned(), "1".to_owned());
    metadata.remove(arrow_batching::QM_BLOCKS_KEY);
    let schema = Schema::new_with_metadata(fields, metadata);
    let boxes = children.iter().map(bbox).collect::<Vec<_>>();
    let (schema, batches) = arrow_batching::blocked_by_z14_cell(schema, columns, &boxes).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut writer = FileWriter::try_new(file, &schema).map_err(|e| e.to_string())?;
    for batch in batches { writer.write(&batch).map_err(|e| e.to_string())?; }
    writer.finish().map_err(|e| e.to_string())?;
    writer.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())
}
