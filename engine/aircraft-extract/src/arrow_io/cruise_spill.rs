//! Stage 2B disk-spill IPC format (v14).
//!
//! Spill rows persist a **raw `CruiseAccum`** (weighted numerators +
//! per-fid set + per-fid top entries) tagged with its target z9 — the
//! merge step reconstructs the same `(z9, CruiseKey) → CruiseAccum` map
//! the in-memory fold/reduce produced. Distinct from `cruise.arrow`,
//! which only stores finalised `CruiseBucket` (averages + top-K), because
//! finalisation is one-way: merging averages would corrupt the per-energy
//! means, and merging two already-finalised top-K lists discards the
//! per-fid Lmax history needed for re-entrant tracking.
//!
//! Rev 2: replaces v13's per-fid (typecode + callsign) metadata list
//! with two parallel sets — a sorted `fid_set: List<UInt64>` for
//! unique-count tracking and a per-fid top-entry struct list carrying
//! Lmax + altitude + identity.
//!
//! No `schema_version` metadata: spill files are intra-process scratch,
//! never read by a different binary. Bypasses
//! [`arrow_io::read_record_batches`] (which asserts the version) via a
//! local reader.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow::array::{
    Array, FixedSizeBinaryArray, FixedSizeBinaryBuilder, Float32Array, Float32Builder, ListArray,
    StringArray, StringBuilder, StructArray, UInt64Array, UInt64Builder, UInt8Array, UInt8Builder,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field, Fields, Schema};
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;

use crate::flight::CruiseTopCandidate;

/// One spilled `(z9, CruiseKey, CruiseAccum)` triple. Field order
/// mirrors the writer schema so column-by-column build/read stays
/// trivially auditable.
pub(crate) struct CruiseSpillRow {
    pub square: u64,
    pub cruise_cell_id: u64,
    pub class: u8,
    pub fl_bin: u8,
    pub period: u8,
    pub rep_profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    pub sum_length_m: f32,
    pub weight: f32,
    pub rep_alt_m: f32,
    pub rep_speed_kt: f32,
    pub rep_len_m: f32,
    pub rep_len_w: f32,
    /// Sorted ascending. `len()` = `unique_count` for the bucket.
    pub fid_set: Vec<u64>,
    /// Per-fid top entries sorted by Lmax descending (tiebreak fid
    /// ascending). At most CRUISE_TOP_K elements; tail fids beyond
    /// the cap are silently dropped at finalisation but still tracked
    /// in `fid_set` for `unique_count`.
    pub top_candidates: Vec<CruiseTopCandidate>,
}

fn top_struct_fields() -> Fields {
    Fields::from(vec![
        Field::new("flight_id", DataType::UInt64, false),
        Field::new("callsign", DataType::Utf8, false),
        Field::new("aircraft_type", DataType::FixedSizeBinary(4), false),
        Field::new("peak_lmax_25m_db", DataType::Float32, false),
        Field::new("altitude_m", DataType::Float32, false),
    ])
}

fn spill_schema() -> Arc<Schema> {
    let top_struct = DataType::Struct(top_struct_fields());
    Arc::new(Schema::new(vec![
        Field::new("square", DataType::UInt64, false),
        Field::new("cruise_cell_id", DataType::UInt64, false),
        Field::new("class", DataType::UInt8, false),
        Field::new("fl_bin", DataType::UInt8, false),
        Field::new("period", DataType::UInt8, false),
        Field::new("rep_profile_idx", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt8, false),
        Field::new("origin", DataType::UInt8, false),
        Field::new("sum_length_m", DataType::Float32, false),
        Field::new("weight", DataType::Float32, false),
        Field::new("rep_alt_m", DataType::Float32, false),
        Field::new("rep_speed_kt", DataType::Float32, false),
        Field::new("rep_len_m", DataType::Float32, false),
        Field::new("rep_len_w", DataType::Float32, false),
        Field::new(
            "fid_set",
            DataType::List(Arc::new(Field::new("item", DataType::UInt64, false))),
            false,
        ),
        Field::new(
            "top_candidates",
            DataType::List(Arc::new(Field::new("item", top_struct, false))),
            false,
        ),
    ]))
}

pub(crate) fn write_cruise_spill(path: &Path, rows: &[CruiseSpillRow]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let schema = spill_schema();
    let n = rows.len();
    let mut square = UInt64Builder::with_capacity(n);
    let mut cruise_cell = UInt64Builder::with_capacity(n);
    let mut class = UInt8Builder::with_capacity(n);
    let mut fl_bin = UInt8Builder::with_capacity(n);
    let mut period = UInt8Builder::with_capacity(n);
    let mut rep_pi = UInt8Builder::with_capacity(n);
    let mut source_id = UInt8Builder::with_capacity(n);
    let mut origin = UInt8Builder::with_capacity(n);
    let mut sum_len = Float32Builder::with_capacity(n);
    let mut weight = Float32Builder::with_capacity(n);
    let mut rep_alt = Float32Builder::with_capacity(n);
    let mut rep_speed = Float32Builder::with_capacity(n);
    let mut rep_len = Float32Builder::with_capacity(n);
    let mut rep_len_w = Float32Builder::with_capacity(n);

    let total_fids: usize = rows.iter().map(|r| r.fid_set.len()).sum();
    let mut fid_off: Vec<i32> = Vec::with_capacity(n + 1);
    fid_off.push(0);
    let mut fid_vals = UInt64Builder::with_capacity(total_fids);
    let mut running_fids = 0usize;

    let total_top: usize = rows.iter().map(|r| r.top_candidates.len()).sum();
    let mut top_off: Vec<i32> = Vec::with_capacity(n + 1);
    top_off.push(0);
    let mut top_fid = UInt64Builder::with_capacity(total_top);
    let mut top_callsign = StringBuilder::with_capacity(total_top, total_top * 8);
    let mut top_typecode = FixedSizeBinaryBuilder::with_capacity(total_top, 4);
    let mut top_lmax = Float32Builder::with_capacity(total_top);
    let mut top_alt = Float32Builder::with_capacity(total_top);
    let mut running_top = 0usize;

    for row in rows {
        square.append_value(row.square);
        cruise_cell.append_value(row.cruise_cell_id);
        class.append_value(row.class);
        fl_bin.append_value(row.fl_bin);
        period.append_value(row.period);
        rep_pi.append_value(row.rep_profile_idx);
        source_id.append_value(row.source_id);
        origin.append_value(row.origin);
        sum_len.append_value(row.sum_length_m);
        weight.append_value(row.weight);
        rep_alt.append_value(row.rep_alt_m);
        rep_speed.append_value(row.rep_speed_kt);
        rep_len.append_value(row.rep_len_m);
        rep_len_w.append_value(row.rep_len_w);
        for &fid in &row.fid_set {
            fid_vals.append_value(fid);
        }
        running_fids += row.fid_set.len();
        fid_off.push(running_fids as i32);
        for cand in &row.top_candidates {
            top_fid.append_value(cand.flight_id);
            top_callsign.append_value(&cand.callsign);
            top_typecode.append_value(cand.aircraft_type)?;
            top_lmax.append_value(cand.peak_lmax_25m_db);
            top_alt.append_value(cand.altitude_m);
        }
        running_top += row.top_candidates.len();
        top_off.push(running_top as i32);
    }

    let fid_list = ListArray::new(
        Arc::new(Field::new("item", DataType::UInt64, false)),
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(fid_off)),
        Arc::new(fid_vals.finish()),
        None,
    );
    let top_struct = StructArray::new(
        top_struct_fields(),
        vec![
            Arc::new(top_fid.finish()),
            Arc::new(top_callsign.finish()),
            Arc::new(top_typecode.finish()),
            Arc::new(top_lmax.finish()),
            Arc::new(top_alt.finish()),
        ],
        None,
    );
    let top_list = ListArray::new(
        Arc::new(Field::new(
            "item",
            DataType::Struct(top_struct_fields()),
            false,
        )),
        OffsetBuffer::new(arrow::buffer::ScalarBuffer::from(top_off)),
        Arc::new(top_struct),
        None,
    );

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(square.finish()),
            Arc::new(cruise_cell.finish()),
            Arc::new(class.finish()),
            Arc::new(fl_bin.finish()),
            Arc::new(period.finish()),
            Arc::new(rep_pi.finish()),
            Arc::new(source_id.finish()),
            Arc::new(origin.finish()),
            Arc::new(sum_len.finish()),
            Arc::new(weight.finish()),
            Arc::new(rep_alt.finish()),
            Arc::new(rep_speed.finish()),
            Arc::new(rep_len.finish()),
            Arc::new(rep_len_w.finish()),
            Arc::new(fid_list),
            Arc::new(top_list),
        ],
    )?;

    let f = File::create(path).with_context(|| format!("create spill {}", path.display()))?;
    // BufWriter coalesces FileWriter's many small writes into one
    // syscall per ~8 KB — meaningful at 1024 small files per flush.
    let mut w = FileWriter::try_new(BufWriter::new(f), &schema)?;
    if batch.num_rows() > 0 {
        w.write(&batch)?;
    }
    w.finish()?;
    Ok(())
}

pub(crate) fn read_cruise_spill(path: &Path) -> Result<Vec<CruiseSpillRow>> {
    let f = File::open(path).with_context(|| format!("open spill {}", path.display()))?;
    let r = FileReader::try_new(BufReader::new(f), None)?;
    let mut out: Vec<CruiseSpillRow> = Vec::new();
    for batch in r {
        let batch = batch?;
        let square = downcast::<UInt64Array>(&batch, 0)?;
        let cruise_cell = downcast::<UInt64Array>(&batch, 1)?;
        let class = downcast::<UInt8Array>(&batch, 2)?;
        let fl_bin = downcast::<UInt8Array>(&batch, 3)?;
        let period = downcast::<UInt8Array>(&batch, 4)?;
        let rep_pi = downcast::<UInt8Array>(&batch, 5)?;
        let source_id = downcast::<UInt8Array>(&batch, 6)?;
        let origin = downcast::<UInt8Array>(&batch, 7)?;
        let sum_len = downcast::<Float32Array>(&batch, 8)?;
        let weight = downcast::<Float32Array>(&batch, 9)?;
        let rep_alt = downcast::<Float32Array>(&batch, 10)?;
        let rep_speed = downcast::<Float32Array>(&batch, 11)?;
        let rep_len = downcast::<Float32Array>(&batch, 12)?;
        let rep_len_w = downcast::<Float32Array>(&batch, 13)?;
        let fid_list = downcast::<ListArray>(&batch, 14)?;
        let top_list = downcast::<ListArray>(&batch, 15)?;
        let fid_vals = fid_list
            .values()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| anyhow::anyhow!("spill: fid_set inner UInt64"))?;
        let top_struct = top_list
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| anyhow::anyhow!("spill: top_candidates inner Struct"))?;
        let top_fid = top_struct
            .column_by_name("flight_id")
            .and_then(|a| a.as_any().downcast_ref::<UInt64Array>())
            .ok_or_else(|| anyhow::anyhow!("spill: top.flight_id"))?;
        let top_callsign = top_struct
            .column_by_name("callsign")
            .and_then(|a| a.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("spill: top.callsign"))?;
        let top_typecode = top_struct
            .column_by_name("aircraft_type")
            .and_then(|a| a.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .ok_or_else(|| anyhow::anyhow!("spill: top.aircraft_type"))?;
        let top_lmax = top_struct
            .column_by_name("peak_lmax_25m_db")
            .and_then(|a| a.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow::anyhow!("spill: top.peak_lmax_25m_db"))?;
        let top_alt = top_struct
            .column_by_name("altitude_m")
            .and_then(|a| a.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow::anyhow!("spill: top.altitude_m"))?;

        let n_rows = batch.num_rows();
        out.reserve(n_rows);
        let fid_offsets = fid_list.value_offsets();
        let top_offsets = top_list.value_offsets();
        for i in 0..n_rows {
            let f_lo = fid_offsets[i] as usize;
            let f_hi = fid_offsets[i + 1] as usize;
            let mut fid_set = Vec::with_capacity(f_hi - f_lo);
            for j in f_lo..f_hi {
                fid_set.push(fid_vals.value(j));
            }
            let t_lo = top_offsets[i] as usize;
            let t_hi = top_offsets[i + 1] as usize;
            let mut top_candidates = Vec::with_capacity(t_hi - t_lo);
            for j in t_lo..t_hi {
                let tc_bytes = top_typecode.value(j);
                let mut tc = [0u8; 4];
                tc.copy_from_slice(tc_bytes);
                top_candidates.push(CruiseTopCandidate {
                    flight_id: top_fid.value(j),
                    callsign: top_callsign.value(j).to_string(),
                    aircraft_type: tc,
                    peak_lmax_25m_db: top_lmax.value(j),
                    altitude_m: top_alt.value(j),
                });
            }
            out.push(CruiseSpillRow {
                square: square.value(i),
                cruise_cell_id: cruise_cell.value(i),
                class: class.value(i),
                fl_bin: fl_bin.value(i),
                period: period.value(i),
                rep_profile_idx: rep_pi.value(i),
                source_id: source_id.value(i),
                origin: origin.value(i),
                sum_length_m: sum_len.value(i),
                weight: weight.value(i),
                rep_alt_m: rep_alt.value(i),
                rep_speed_kt: rep_speed.value(i),
                rep_len_m: rep_len.value(i),
                rep_len_w: rep_len_w.value(i),
                fid_set,
                top_candidates,
            });
        }
    }
    Ok(out)
}

fn downcast<T: Array + 'static>(batch: &RecordBatch, col: usize) -> Result<&T> {
    batch
        .column(col)
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "spill: column {col} type mismatch (expected {})",
                std::any::type_name::<T>()
            )
        })
}

#[cfg(test)]
#[path = "cruise_spill_tests.rs"]
mod tests;
