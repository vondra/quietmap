//! Globally unique airport counts published into traffic-owner cells, and raw identity parts for reduction.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    Array, ArrayRef, FixedSizeListArray, ListArray, ListBuilder, StringArray, StringBuilder,
    UInt16Array, UInt16Builder, UInt32Array, UInt32Builder, UInt64Array, UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use crate::arrow_schemas;
use crate::stage_2c::airport_traffic_writer::AirportSummaryPartRow;

use super::{read_all_batches, write_record_batches};

/// Output of Stage 2C reduce phase. One row per `airport_key` carrying
/// truly unique counts across all z9s. v2 splits arr/dep/ops into a
/// non-GA window (existing fields) and a GA window
/// (`airport_unique_ga_*`); the popup divides `non_ga / n_days +
/// ga / ga_n_days`. GSE is
/// airline-pass only (unsplit).
#[derive(Clone, Debug, PartialEq)]
pub struct AirportSummaryRow {
    pub airport_key: String,
    pub airport_unique_arr_count: u32,
    pub airport_unique_dep_count: u32,
    pub airport_unique_gse_count_per_class: [u32; arrow_schemas::NUM_GSE_CLASSES as usize],
    pub airport_unique_ops_count_per_kind: [u32; arrow_schemas::NUM_OPS_KINDS as usize],
    pub airport_unique_ga_arr_count: u32,
    pub airport_unique_ga_dep_count: u32,
    pub airport_unique_ga_ops_count_per_kind: [u32; arrow_schemas::NUM_OPS_KINDS as usize],
}

pub fn write_airport_summary(path: &Path, rows: &[AirportSummaryRow]) -> Result<()> {
    let schema = arrow_schemas::airport_summary_schema();
    let n = rows.len();
    let mut airport_key = StringBuilder::with_capacity(n, 8 * n);
    let mut arr = UInt32Builder::with_capacity(n);
    let mut dep = UInt32Builder::with_capacity(n);
    let mut ga_arr = UInt32Builder::with_capacity(n);
    let mut ga_dep = UInt32Builder::with_capacity(n);
    let gse_classes = arrow_schemas::NUM_GSE_CLASSES as usize;
    let ops_kinds = arrow_schemas::NUM_OPS_KINDS as usize;
    let mut gse_values: Vec<u32> = Vec::with_capacity(n * gse_classes);
    let mut ops_values: Vec<u32> = Vec::with_capacity(n * ops_kinds);
    let mut ga_ops_values: Vec<u32> = Vec::with_capacity(n * ops_kinds);

    for r in rows {
        airport_key.append_value(&r.airport_key);
        arr.append_value(r.airport_unique_arr_count);
        dep.append_value(r.airport_unique_dep_count);
        gse_values.extend_from_slice(&r.airport_unique_gse_count_per_class);
        ops_values.extend_from_slice(&r.airport_unique_ops_count_per_kind);
        ga_arr.append_value(r.airport_unique_ga_arr_count);
        ga_dep.append_value(r.airport_unique_ga_dep_count);
        ga_ops_values.extend_from_slice(&r.airport_unique_ga_ops_count_per_kind);
    }

    let item = Arc::new(Field::new("item", DataType::UInt32, false));
    let gse_list = FixedSizeListArray::new(
        item.clone(),
        arrow_schemas::NUM_GSE_CLASSES,
        Arc::new(UInt32Array::from(gse_values)),
        None,
    );
    let ops_list = FixedSizeListArray::new(
        item.clone(),
        arrow_schemas::NUM_OPS_KINDS,
        Arc::new(UInt32Array::from(ops_values)),
        None,
    );
    let ga_ops_list = FixedSizeListArray::new(
        item,
        arrow_schemas::NUM_OPS_KINDS,
        Arc::new(UInt32Array::from(ga_ops_values)),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(airport_key.finish()),
        Arc::new(arr.finish()),
        Arc::new(dep.finish()),
        Arc::new(gse_list),
        Arc::new(ops_list),
        Arc::new(ga_arr.finish()),
        Arc::new(ga_dep.finish()),
        Arc::new(ga_ops_list),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)?;
    write_record_batches(path, &schema, &[batch])
}

pub fn read_airport_summary(path: &Path) -> Result<Vec<AirportSummaryRow>> {
    let (schema, batches) = read_all_batches(path)?;
    arrow_schemas::assert_airport_summary_contract(schema.metadata())?;
    let gse_classes = arrow_schemas::NUM_GSE_CLASSES as usize;
    let ops_kinds = arrow_schemas::NUM_OPS_KINDS as usize;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    let mut out = Vec::with_capacity(total_rows);
    let fixed_buf = |b: &RecordBatch, name: &str| -> Result<Vec<u32>> {
        Ok(column::<FixedSizeListArray>(b, name)?
            .values()
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| anyhow::anyhow!("{name} inner type"))?
            .values()
            .to_vec())
    };
    for b in batches {
        let airport_key = column::<StringArray>(&b, "airport_key")?;
        let arr = column::<UInt32Array>(&b, "airport_unique_arr_count")?;
        let dep = column::<UInt32Array>(&b, "airport_unique_dep_count")?;
        let ga_arr = column::<UInt32Array>(&b, "airport_unique_ga_arr_count")?;
        let ga_dep = column::<UInt32Array>(&b, "airport_unique_ga_dep_count")?;
        let gse_buf = fixed_buf(&b, "airport_unique_gse_count_per_class")?;
        let ops_buf = fixed_buf(&b, "airport_unique_ops_count_per_kind")?;
        let ga_ops_buf = fixed_buf(&b, "airport_unique_ga_ops_count_per_kind")?;
        for i in 0..b.num_rows() {
            let lo_g = i * gse_classes;
            let mut gse = [0u32; arrow_schemas::NUM_GSE_CLASSES as usize];
            gse.copy_from_slice(&gse_buf[lo_g..lo_g + gse_classes]);
            let lo_o = i * ops_kinds;
            let mut ops = [0u32; arrow_schemas::NUM_OPS_KINDS as usize];
            ops.copy_from_slice(&ops_buf[lo_o..lo_o + ops_kinds]);
            let mut ga_ops = [0u32; arrow_schemas::NUM_OPS_KINDS as usize];
            ga_ops.copy_from_slice(&ga_ops_buf[lo_o..lo_o + ops_kinds]);
            out.push(AirportSummaryRow {
                airport_key: airport_key.value(i).to_string(),
                airport_unique_arr_count: arr.value(i),
                airport_unique_dep_count: dep.value(i),
                airport_unique_gse_count_per_class: gse,
                airport_unique_ops_count_per_kind: ops,
                airport_unique_ga_arr_count: ga_arr.value(i),
                airport_unique_ga_dep_count: ga_dep.value(i),
                airport_unique_ga_ops_count_per_kind: ga_ops,
            });
        }
    }
    Ok(out)
}

fn column<'a, T: arrow::array::Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("missing column {name}"))?
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "column {name} type mismatch (expected {})",
                std::any::type_name::<T>()
            )
        })
}

/// Internal scratch: one sorted flight identity and a union bitmask, without
/// repeating the same identity in each arrival/departure/class/operation list.
fn part_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("airport_key", DataType::Utf8, false),
        Field::new(
            "flight_ids",
            DataType::List(Arc::new(Field::new("item", DataType::UInt64, true))),
            false,
        ),
        Field::new(
            "membership_flags",
            DataType::List(Arc::new(Field::new("item", DataType::UInt16, true))),
            false,
        ),
    ]))
}

pub(crate) fn write_airport_summary_part(
    path: &Path,
    rows: &[AirportSummaryPartRow],
) -> Result<()> {
    let total = rows
        .iter()
        .try_fold(0usize, |n, row| n.checked_add(row.members.len()))
        .ok_or_else(|| anyhow::anyhow!("airport membership length overflow"))?;
    anyhow::ensure!(
        total <= i32::MAX as usize,
        "airport membership list exceeds Int32 offsets"
    );
    let mut keys = StringBuilder::new();
    let mut fids = ListBuilder::new(UInt64Builder::new());
    let mut flags = ListBuilder::new(UInt16Builder::new());
    for row in rows {
        keys.append_value(&row.airport_key);
        for &(fid, mask) in &row.members {
            fids.values().append_value(fid);
            flags.values().append_value(mask);
        }
        fids.append(true);
        flags.append(true);
    }
    let schema = part_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(keys.finish()),
            Arc::new(fids.finish()),
            Arc::new(flags.finish()),
        ],
    )?;
    write_record_batches(path, &schema, &[batch])
}

pub(crate) fn read_airport_summary_part(path: &Path) -> Result<Vec<AirportSummaryPartRow>> {
    let file = std::fs::File::open(path)?;
    let reader = arrow::ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch?;
        let keys = super::required_column::<StringArray>(&batch, "airport_key")?;
        let fids = super::required_column::<ListArray>(&batch, "flight_ids")?;
        let masks = super::required_column::<ListArray>(&batch, "membership_flags")?;
        anyhow::ensure!(
            fids.value_offsets() == masks.value_offsets(),
            "airport identity and mask offsets differ"
        );
        let ids = fids
            .values()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| anyhow::anyhow!("airport flight identity type"))?;
        let flags = masks
            .values()
            .as_any()
            .downcast_ref::<UInt16Array>()
            .ok_or_else(|| anyhow::anyhow!("airport membership type"))?;
        anyhow::ensure!(
            ids.null_count() == 0 && flags.null_count() == 0,
            "null airport membership"
        );
        for i in 0..batch.num_rows() {
            anyhow::ensure!(!keys.value(i).is_empty(), "empty airport membership key");
            let from = fids.value_offsets()[i] as usize;
            let to = fids.value_offsets()[i + 1] as usize;
            let members: Vec<_> = (from..to).map(|j| (ids.value(j), flags.value(j))).collect();
            anyhow::ensure!(
                members.iter().all(|&(fid, mask)| fid != 0
                    && mask != 0
                    && mask & !crate::stage_2c::movements::AIRPORT_FLAGS == 0),
                "invalid airport flight identity or flags"
            );
            out.push(AirportSummaryPartRow {
                airport_key: keys.value(i).to_string(),
                members,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "airport_summary_tests.rs"]
mod tests;
