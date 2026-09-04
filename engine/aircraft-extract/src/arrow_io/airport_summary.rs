//! `airport_summary.arrow` — global single-row-per-airport with truly
//! unique counts UNIONed across all z9s. Produced by Stage 2C v5
//! reduce phase from per-airport per-z9 `airport_summary_parts/`
//! dumps. Loaded once at popup query time.
//!
//! `airport_summary_part.arrow` is the per-z9 intermediate carrying
//! raw fid sets that the reduce phase UNIONs to produce the global
//! `airport_summary.arrow`.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    Array, ArrayRef, FixedSizeListArray, ListArray, StringArray, StringBuilder, UInt32Array,
    UInt32Builder, UInt64Array, UInt64Builder,
};
use arrow::buffer::OffsetBuffer;
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
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
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

// ─── airport_summary_part: per-airport per-z9 raw fid sets for reduce ───

/// Schema for `airport_summary_parts/<square>/part.arrow`. One row per
/// airport_key carrying raw `List<UInt64>` fid sets per dimension. The
/// reduce loops every z9 part for one airport, UNIONs the HashSets, and
/// counts to produce the global summary row. v9 splits arr/dep/ops into
/// non-GA + GA windows (`ga_*` columns).
fn part_schema() -> Arc<Schema> {
    let u64_list = DataType::List(Arc::new(Field::new("item", DataType::UInt64, false)));
    let fixed_of =
        |n: i32| DataType::FixedSizeList(Arc::new(Field::new("item", u64_list.clone(), false)), n);
    Arc::new(Schema::new(vec![
        Field::new("airport_key", DataType::Utf8, false),
        Field::new("arr_fids", u64_list.clone(), false),
        Field::new("dep_fids", u64_list.clone(), false),
        Field::new(
            "gse_fids_per_class",
            fixed_of(arrow_schemas::NUM_GSE_CLASSES),
            false,
        ),
        Field::new(
            "ops_fids_per_kind",
            fixed_of(arrow_schemas::NUM_OPS_KINDS),
            false,
        ),
        Field::new("ga_arr_fids", u64_list.clone(), false),
        Field::new("ga_dep_fids", u64_list.clone(), false),
        Field::new(
            "ga_ops_fids_per_kind",
            fixed_of(arrow_schemas::NUM_OPS_KINDS),
            false,
        ),
    ]))
}

/// Encode one `Vec<u64>`-per-row column as `List<UInt64>`.
fn build_u64_list<'a>(rows: impl Iterator<Item = &'a [u64]>, n: usize) -> ListArray {
    let mut off: Vec<i32> = Vec::with_capacity(n + 1);
    off.push(0);
    let mut vals = UInt64Builder::new();
    let mut running = 0usize;
    for fids in rows {
        for &fid in fids {
            vals.append_value(fid);
        }
        running += fids.len();
        off.push(running as i32);
    }
    ListArray::new(
        Arc::new(Field::new("item", DataType::UInt64, false)),
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(off)),
        Arc::new(vals.finish()),
        None,
    )
}

/// Encode one `[Vec<u64>; K]`-per-row column as
/// `FixedSizeList<List<UInt64>, K>`.
fn build_fixed_list_of_u64_list<'a>(
    rows: impl Iterator<Item = &'a [Vec<u64>]>,
    n: usize,
    k: i32,
) -> FixedSizeListArray {
    let mut off: Vec<i32> = Vec::with_capacity(n * k as usize + 1);
    off.push(0);
    let mut vals = UInt64Builder::new();
    let mut running = 0usize;
    for per_kind in rows {
        for v in per_kind {
            for &fid in v {
                vals.append_value(fid);
            }
            running += v.len();
            off.push(running as i32);
        }
    }
    let inner = ListArray::new(
        Arc::new(Field::new("item", DataType::UInt64, false)),
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(off)),
        Arc::new(vals.finish()),
        None,
    );
    let item = DataType::List(Arc::new(Field::new("item", DataType::UInt64, false)));
    FixedSizeListArray::new(
        Arc::new(Field::new("item", item, false)),
        k,
        Arc::new(inner),
        None,
    )
}

pub(crate) fn write_airport_summary_part(
    path: &Path,
    rows: &[AirportSummaryPartRow],
) -> Result<()> {
    let schema = part_schema();
    let n = rows.len();
    let mut airport_key = StringBuilder::with_capacity(n, 8 * n);
    for r in rows {
        airport_key.append_value(&r.airport_key);
    }
    let gse = arrow_schemas::NUM_GSE_CLASSES;
    let ops = arrow_schemas::NUM_OPS_KINDS;
    let columns: Vec<ArrayRef> = vec![
        Arc::new(airport_key.finish()),
        Arc::new(build_u64_list(
            rows.iter().map(|r| r.arr_fids.as_slice()),
            n,
        )),
        Arc::new(build_u64_list(
            rows.iter().map(|r| r.dep_fids.as_slice()),
            n,
        )),
        Arc::new(build_fixed_list_of_u64_list(
            rows.iter().map(|r| r.gse_fids_per_class.as_slice()),
            n,
            gse,
        )),
        Arc::new(build_fixed_list_of_u64_list(
            rows.iter().map(|r| r.ops_fids_per_kind.as_slice()),
            n,
            ops,
        )),
        Arc::new(build_u64_list(
            rows.iter().map(|r| r.ga_arr_fids.as_slice()),
            n,
        )),
        Arc::new(build_u64_list(
            rows.iter().map(|r| r.ga_dep_fids.as_slice()),
            n,
        )),
        Arc::new(build_fixed_list_of_u64_list(
            rows.iter().map(|r| r.ga_ops_fids_per_kind.as_slice()),
            n,
            ops,
        )),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // No schema-version stamp on parts — they're intra-process scratch
    // (analogous to `cruise_spill.rs`). Bypass the version-asserting
    // `write_record_batches` to avoid needing the
    // arrow_schemas::base_metadata header.
    use arrow::ipc::writer::FileWriter;
    use std::fs::File;
    use std::io::BufWriter;
    let f = File::create(path)?;
    let mut w = FileWriter::try_new(BufWriter::new(f), &schema)?;
    if batch.num_rows() > 0 {
        w.write(&batch)?;
    }
    w.finish()?;
    Ok(())
}

/// Decode a `List<UInt64>` column into per-row `Vec<u64>`.
fn read_u64_list(list: &ListArray) -> Result<Vec<Vec<u64>>> {
    let vals = list
        .values()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| anyhow::anyhow!("List<UInt64> inner type"))?;
    let off = list.value_offsets();
    Ok((0..list.len())
        .map(|i| vals.values()[off[i] as usize..off[i + 1] as usize].to_vec())
        .collect())
}

/// Decode a `FixedSizeList<List<UInt64>, K>` column into per-row
/// `[Vec<u64>; K]`.
fn read_fixed_list_of_u64_list<const K: usize>(
    outer: &FixedSizeListArray,
) -> Result<Vec<[Vec<u64>; K]>> {
    let inner = outer
        .values()
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or_else(|| anyhow::anyhow!("FixedSizeList inner List"))?;
    let vals = inner
        .values()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| anyhow::anyhow!("FixedSizeList inner UInt64"))?;
    let off = inner.value_offsets();
    Ok((0..outer.len())
        .map(|i| {
            std::array::from_fn(|k| {
                let outer_idx = i * K + k;
                vals.values()[off[outer_idx] as usize..off[outer_idx + 1] as usize].to_vec()
            })
        })
        .collect())
}

/// Reads one Stage 2C `part.arrow` shard back into rows; the reduce phase
/// (`stage_2c::airport_summary_reduce`) absorbs every z9 subdir's part.
/// Kept in lockstep with the writer so the round-trip test catches schema
/// drift early.
pub(crate) fn read_airport_summary_part(path: &Path) -> Result<Vec<AirportSummaryPartRow>> {
    use arrow::ipc::reader::FileReader;
    use std::fs::File;
    use std::io::BufReader;
    const GSE: usize = arrow_schemas::NUM_GSE_CLASSES as usize;
    const OPS: usize = arrow_schemas::NUM_OPS_KINDS as usize;
    let f = File::open(path)?;
    let r = FileReader::try_new(BufReader::new(f), None)?;
    let mut out = Vec::new();
    for batch in r {
        let batch = batch?;
        let airport_key = column::<StringArray>(&batch, "airport_key")?;
        let arr = read_u64_list(column::<ListArray>(&batch, "arr_fids")?)?;
        let dep = read_u64_list(column::<ListArray>(&batch, "dep_fids")?)?;
        let gse = read_fixed_list_of_u64_list::<GSE>(column::<FixedSizeListArray>(
            &batch,
            "gse_fids_per_class",
        )?)?;
        let ops = read_fixed_list_of_u64_list::<OPS>(column::<FixedSizeListArray>(
            &batch,
            "ops_fids_per_kind",
        )?)?;
        let ga_arr = read_u64_list(column::<ListArray>(&batch, "ga_arr_fids")?)?;
        let ga_dep = read_u64_list(column::<ListArray>(&batch, "ga_dep_fids")?)?;
        let ga_ops = read_fixed_list_of_u64_list::<OPS>(column::<FixedSizeListArray>(
            &batch,
            "ga_ops_fids_per_kind",
        )?)?;
        for i in 0..batch.num_rows() {
            out.push(AirportSummaryPartRow {
                airport_key: airport_key.value(i).to_string(),
                arr_fids: arr[i].clone(),
                dep_fids: dep[i].clone(),
                gse_fids_per_class: gse[i].clone(),
                ops_fids_per_kind: ops[i].clone(),
                ga_arr_fids: ga_arr[i].clone(),
                ga_dep_fids: ga_dep[i].clone(),
                ga_ops_fids_per_kind: ga_ops[i].clone(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "airport_summary_tests.rs"]
mod tests;
