//! Append retained OSM evidence and whole-way endpoints to extraction rows.

use super::write_arrow_z14_blocked;
use anyhow::{ensure, Result};
use arrow::array::{ArrayRef, Int32Array, Int64Array, StringArray, UInt16Array};
use arrow::datatypes::{DataType, Field, Schema};
use std::{path::Path, sync::Arc};

pub(super) fn write_with_evidence(
    path: &Path,
    schema: Schema,
    mut columns: Vec<ArrayRef>,
    boxes: &[arrow_batching::RowBbox],
    rows: &[Vec<String>],
    tag_index: usize,
) -> Result<()> {
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    let family = path.file_stem().unwrap().to_str().unwrap();
    let mut metadata = schema.metadata().clone();
    let (key, value) = square_store::osm_contract::contract(family).unwrap();
    metadata.insert(key.to_string(), value.to_string());
    let mut hgv = Vec::new();
    for row in rows {
        ensure!(row.len() > tag_index, "old or truncated {family} spill row");
        let parsed: crate::classify::Tags = serde_json::from_str(&row[tag_index])?;
        if family == "roads" {
            hgv.push(
                parsed
                    .get("maxspeed:hgv")
                    .filter(|s| !s.contains(':'))
                    .map(|s| crate::classify::parse_maxspeed_kmh(s))
                    .unwrap_or(0),
            );
        }
    }
    fields.push(Field::new("osm_tags", DataType::Utf8, false));
    columns.push(Arc::new(StringArray::from_iter_values(
        rows.iter().map(|r| r[tag_index].as_str()),
    )));
    if matches!(family, "industrial" | "leisure") {
        let mut kinds = Vec::with_capacity(rows.len());
        for row in rows {
            ensure!(
                row.len() > tag_index + 1
                    && matches!(row[tag_index + 1].as_str(), "node" | "way" | "relation"),
                "missing OSM object kind"
            );
            kinds.push(row[tag_index + 1].as_str());
        }
        fields.push(Field::new("osm_kind", DataType::Utf8, false));
        columns.push(Arc::new(StringArray::from(kinds)));
    }
    if matches!(family, "roads" | "railways") {
        let endpoints = rows
            .iter()
            .map(|row| -> Result<Vec<Option<i64>>> {
                ensure!(row.len() > tag_index + 1, "missing way extent");
                let result: Vec<Option<i64>> = serde_json::from_str(&row[tag_index + 1])?;
                ensure!(result.len() == 6, "invalid way extent");
                Ok(result)
            })
            .collect::<Result<Vec<_>>>()?;
        for (index, name) in [
            "way_start_node",
            "way_end_node",
            "way_start_gx",
            "way_start_gy",
            "way_end_gx",
            "way_end_gy",
        ]
        .iter()
        .enumerate()
        {
            if index < 2 {
                fields.push(Field::new(*name, DataType::Int64, true));
                columns.push(Arc::new(Int64Array::from(
                    endpoints.iter().map(|e| e[index]).collect::<Vec<_>>(),
                )));
            } else {
                fields.push(Field::new(*name, DataType::Int32, true));
                columns.push(Arc::new(Int32Array::from(
                    endpoints
                        .iter()
                        .map(|e| e[index].map(|v| v as i32))
                        .collect::<Vec<_>>(),
                )));
            }
        }
    }
    if family == "roads" {
        fields.push(Field::new("maxspeed_hgv", DataType::UInt16, false));
        columns.push(Arc::new(UInt16Array::from(hgv)));
    }
    write_arrow_z14_blocked(
        path,
        Schema::new_with_metadata(fields, metadata),
        columns,
        boxes,
    )
}
