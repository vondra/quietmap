//! Raw per-owner airport flight identity parts that the Stage 2C reduce unions globally.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    Array, ListArray, ListBuilder, StringArray, StringBuilder, UInt16Array, UInt16Builder,
    UInt64Array, UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use crate::stage_2c::airport_traffic_writer::AirportSummaryPartRow;

use super::write_record_batches;

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
#[path = "airport_summary_parts_tests.rs"]
mod tests;
