//! Split mapped pieces at physical allocation boundaries and write exact prepared road columns.

use super::{allocation, input, model::Model, spatial::RoadIndex};
use anyhow::Result;
use arrow_array::RecordBatch;
use arrow_array::{
    ArrayRef, Float32Array, Float64Array, Int32Array, UInt8Array, UInt16Array, UInt32Array,
};
use arrow_ipc::writer::FileWriter;
use arrow_schema::{DataType, Field, Schema};
use arrow_select::{concat::concat_batches, take::take};
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

fn expand(
    batch: &RecordBatch,
    roads: &[input::Road],
    index: &RoadIndex,
    model: &Model,
) -> Result<Vec<Child>> {
    let coordinates = ["start_gx", "start_gy", "end_gx", "end_gy"]
        .map(|name| input::column::<Int32Array>(batch, name))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let lengths = input::column::<Float32Array>(batch, "length_m")?;
    let mut children = Vec::new();
    for (parent, road) in roads.iter().enumerate() {
        let candidates = index.alternatives(road);
        for (from, to, counts, estimated, cross_section) in
            allocation::intervals(road, &candidates, model)
        {
            let at = |axis: usize, t: f64| {
                let start = coordinates[axis].value(parent) as f64;
                let end = coordinates[axis + 2].value(parent) as f64;
                let world = f64::from(1_u32 << 30);
                let delta = end - start;
                let delta = if axis == 0 {
                    delta - (delta / world).round() * world
                } else {
                    delta
                };
                let value = (start + delta * t).round();
                (if axis == 0 {
                    value.rem_euclid(world)
                } else {
                    value
                }) as i32
            };
            let grid = [at(0, from), at(1, from), at(0, to), at(1, to)];
            if grid[0] == grid[2] && grid[1] == grid[3] {
                continue;
            }
            children.push(Child {
                parent: u32::try_from(parent)?,
                grid,
                length: lengths.value(parent) * (to - from) as f32,
                counts,
                estimated,
                cross_section,
            });
        }
    }
    Ok(children)
}

pub fn stage(
    path: &Path,
    batches: &[RecordBatch],
    roads: &[input::Road],
    index: &RoadIndex,
    model: &Model,
) -> Result<()> {
    let original = batches
        .first()
        .ok_or_else(|| anyhow::anyhow!("road file has no schema"))?
        .schema();
    let merged = concat_batches(&original, batches)?;
    let children = expand(&merged, roads, index, model)?;
    let indices = UInt32Array::from(children.iter().map(|c| c.parent).collect::<Vec<_>>());
    let dropped = [
        "traffic_estimated",
        "traffic_count_basis",
        "traffic_observation_id",
        "traffic_observation_source",
        "source_id",
        "built_up",
        "speed_taper",
    ];
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in original.fields() {
        let name = field.name().as_str();
        if input::COUNTS.contains(&name) || dropped.contains(&name) {
            continue;
        }
        let column: ArrayRef = if let Some(axis) = ["start_gx", "start_gy", "end_gx", "end_gy"]
            .iter()
            .position(|v| *v == name)
        {
            Arc::new(Int32Array::from(
                children.iter().map(|c| c.grid[axis]).collect::<Vec<_>>(),
            ))
        } else if name == "length_m" {
            Arc::new(Float32Array::from(
                children.iter().map(|c| c.length).collect::<Vec<_>>(),
            ))
        } else {
            take(
                merged
                    .column_by_name(name)
                    .ok_or_else(|| anyhow::anyhow!("road column missing"))?
                    .as_ref(),
                &indices,
                None,
            )?
        };
        fields.push(field.as_ref().clone());
        columns.push(column);
    }
    for (category, name) in input::COUNTS.iter().enumerate() {
        fields.push(Field::new(*name, DataType::Float64, false));
        columns.push(Arc::new(Float64Array::from(
            children
                .iter()
                .map(|c| c.counts[category])
                .collect::<Vec<_>>(),
        )));
    }
    fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
    columns.push(Arc::new(UInt8Array::from(
        children.iter().map(|c| c.estimated).collect::<Vec<_>>(),
    )));
    fields.push(Field::new(
        input::CROSS_SECTION_AADT,
        DataType::Float64,
        false,
    ));
    columns.push(Arc::new(Float64Array::from(
        children.iter().map(|c| c.cross_section).collect::<Vec<_>>(),
    )));
    for (name, column) in [
        (
            "source_id",
            Arc::new(UInt16Array::from(
                children
                    .iter()
                    .map(|c| roads[c.parent as usize].source_id)
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            "built_up",
            Arc::new(UInt8Array::from(
                children
                    .iter()
                    .map(|c| roads[c.parent as usize].built_up)
                    .collect::<Vec<_>>(),
            )),
        ),
    ] {
        fields.push(Field::new(name, column.data_type().clone(), false));
        columns.push(column);
    }
    if roads.iter().any(|road| road.speed_taper != 0) {
        fields.push(Field::new("speed_taper", DataType::UInt8, false));
        columns.push(Arc::new(UInt8Array::from(
            children
                .iter()
                .map(|c| roads[c.parent as usize].speed_taper)
                .collect::<Vec<_>>(),
        )));
    }
    let mut metadata = original.metadata().clone();
    metadata.insert(input::CONTRACT.to_owned(), "1".to_owned());
    metadata.remove("qm_blocks");
    let schema = Schema::new_with_metadata(fields, metadata);
    let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns)?;
    let batches = (0..batch.num_rows())
        .step_by(8192)
        .map(|offset| batch.slice(offset, 8192.min(batch.num_rows() - offset)));
    let file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut writer = FileWriter::try_new(file, &schema)?;
    for batch in batches {
        writer.write(&batch)?;
    }
    writer.finish()?;
    writer.into_inner()?.sync_all()?;
    Ok(())
}
