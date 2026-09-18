//! `{roads,railways}.pieces.arrow`: each acoustic piece's place on its original OSM way, beside the layer file.

use anyhow::{bail, ensure, Context, Result};
use arrow::array::{
    ArrayRef, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, ListArray,
    UInt16Array,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

fn parsed<T: FromStr>(text: &str, what: &str, row: &[String]) -> Result<T> {
    text.parse()
        .ok()
        .with_context(|| format!("malformed {what} {text:?} in transport spill row {row:?}"))
}

/// Rows are one square's spill rows in layer order and stay untouched: the layer writer's row
/// order is part of `roads.arrow`. Only a permutation is sorted by `(way_id, segment_idx)`.
/// A malformed row fails the build; a silently zeroed value would become a wrong restored parent.
pub fn write_source_pieces(
    family: &str,
    rows: &[Vec<String>],
    aliases: &HashMap<i64, i64>,
    path: &Path,
) -> Result<()> {
    let railway = family == "railways";
    let mut keys = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        ensure!(row.len() >= 9, "short transport spill row {row:?}");
        let way_id: i64 = parsed(&row[1], "way id", row)?;
        let segment_idx: i16 = parsed(&row[2], "segment index", row)?;
        keys.push((way_id, segment_idx, index));
    }
    keys.sort_unstable();
    if let Some(pair) = keys
        .windows(2)
        .find(|pair| pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1)
    {
        bail!(
            "repeated {family} piece {}:{} in {}",
            pair[0].0,
            pair[0].1,
            path.display()
        );
    }

    let count = rows.len();
    let (mut start_vertex, mut end_vertex) = (Vec::with_capacity(count), Vec::with_capacity(count));
    let (mut start_fraction, mut end_fraction) =
        (Vec::with_capacity(count), Vec::with_capacity(count));
    let (mut start_node, mut end_node) = (Vec::with_capacity(count), Vec::with_capacity(count));
    let mut cells: [Vec<i32>; 4] = Default::default();
    let mut length_m = Vec::with_capacity(count);
    let mut metres: [Vec<Option<f64>>; 3] = Default::default();
    let (mut chain_lat, mut chain_lon) = (Vec::<i32>::new(), Vec::<i32>::new());
    let mut chain_lengths = Vec::new();
    for &(_, _, index) in &keys {
        let row = &rows[index];
        for (cell, text) in cells.iter_mut().zip(&row[3..7]) {
            cell.push(parsed::<i32>(text, "grid cell", row)?);
        }
        length_m.push(parsed::<f32>(&row[7], "length", row)?);
        let tail: Vec<&str> = row[row.len() - 1].split(',').collect();
        ensure!(
            tail.len() == if railway { 10 } else { 6 },
            "malformed piece tail in transport spill row {row:?}"
        );
        start_vertex.push(parsed::<u16>(tail[0], "start vertex", row)?);
        start_fraction.push(parsed::<f64>(tail[1], "start fraction", row)?);
        end_vertex.push(parsed::<u16>(tail[2], "end vertex", row)?);
        end_fraction.push(parsed::<f64>(tail[3], "end fraction", row)?);
        for (nodes, text) in [(&mut start_node, tail[4]), (&mut end_node, tail[5])] {
            let raw: i64 = parsed(text, "node id", row)?;
            nodes.push(aliases.get(&raw).copied().unwrap_or(raw));
        }
        if !railway {
            continue;
        }
        for (column, text) in metres.iter_mut().zip(&tail[6..9]) {
            column.push(if text.is_empty() {
                None
            } else {
                Some(parsed::<f64>(text, "metres", row)?)
            });
        }
        let chain: Vec<i32> = tail[9]
            .split(';')
            .map(|text| parsed::<i32>(text, "chain coordinate", row))
            .collect::<Result<_>>()?;
        ensure!(
            chain.len() >= 4 && chain.len().is_multiple_of(2),
            "malformed chain in transport spill row {row:?}"
        );
        chain_lengths.push(chain.len() / 2);
        chain_lat.extend(chain.iter().step_by(2));
        chain_lon.extend(chain.iter().skip(1).step_by(2));
    }

    let required = |name: &str, kind: DataType| Field::new(name, kind, false);
    let mut fields = vec![
        required("way_id", DataType::Int64),
        required("segment_idx", DataType::Int16),
        required("start_vertex", DataType::UInt16),
        required("start_fraction", DataType::Float64),
        required("end_vertex", DataType::UInt16),
        required("end_fraction", DataType::Float64),
        required("start_node", DataType::Int64),
        required("end_node", DataType::Int64),
    ];
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from_iter_values(keys.iter().map(|key| key.0))),
        Arc::new(Int16Array::from_iter_values(keys.iter().map(|key| key.1))),
        Arc::new(UInt16Array::from(start_vertex)),
        Arc::new(Float64Array::from(start_fraction)),
        Arc::new(UInt16Array::from(end_vertex)),
        Arc::new(Float64Array::from(end_fraction)),
        Arc::new(Int64Array::from(start_node)),
        Arc::new(Int64Array::from(end_node)),
    ];
    for (name, cell) in ["start_gx", "start_gy", "end_gx", "end_gy"]
        .into_iter()
        .zip(cells)
    {
        fields.push(required(name, DataType::Int32));
        columns.push(Arc::new(Int32Array::from(cell)));
    }
    fields.push(required("length_m", DataType::Float32));
    columns.push(Arc::new(Float32Array::from(length_m)));
    if railway {
        for (name, column) in ["first_vertex_m", "from_m", "to_m"].into_iter().zip(metres) {
            fields.push(Field::new(name, DataType::Float64, true));
            columns.push(Arc::new(Float64Array::from(column)));
        }
        let item = Arc::new(Field::new("item", DataType::Int32, false));
        for (name, values) in [("chain_lat_e7", chain_lat), ("chain_lon_e7", chain_lon)] {
            fields.push(required(name, DataType::List(item.clone())));
            columns.push(Arc::new(ListArray::new(
                item.clone(),
                OffsetBuffer::from_lengths(chain_lengths.iter().copied()),
                Arc::new(Int32Array::from(values)),
                None,
            )));
        }
    }
    let schema = Arc::new(Schema::new(fields).with_metadata(HashMap::from([
        ("transport_pieces_contract".to_owned(), "1".to_owned()),
        ("family".to_owned(), family.to_owned()),
    ])));
    super::write_single_batch_arrow(path, RecordBatch::try_new(schema, columns)?)
}
