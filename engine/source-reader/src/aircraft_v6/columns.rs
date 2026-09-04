//! Typed downcasts of `RecordBatch` columns by name. Each helper
//! returns `None` when the column is missing or has the wrong dtype,
//! matching the soft-fail behaviour of the v6 row builders (a malformed
//! batch contributes zero rows rather than panicking the popup).

use arrow::array::*;
use arrow::record_batch::RecordBatch;

pub fn col_u64<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a UInt64Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u32<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a UInt32Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_i64<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u8<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a UInt8Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_f32<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Float32Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_list<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a ListArray> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u16<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a UInt16Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_fixed_size_list<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a FixedSizeListArray> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
/// Returns the column iff it's a `FixedSizeBinary(width)` column with
/// the expected element width. Catches schema drift (e.g. someone
/// widening `aircraft_type` to 6 bytes) before the per-row
/// `copy_from_slice([u8; 4])` panics inside the popup worker.
pub fn col_fixed_size_binary<'a>(
    batch: &'a RecordBatch,
    name: &str,
    expected_width: i32,
) -> Option<&'a FixedSizeBinaryArray> {
    let arr = batch
        .column_by_name(name)?
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()?;
    if arr.value_length() == expected_width {
        Some(arr)
    } else {
        None
    }
}
