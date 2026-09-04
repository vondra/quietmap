//! Required aircraft column validation and nullable OSM metadata access.

use arrow::array::*;
use arrow::record_batch::RecordBatch;

pub fn required_array<'a, T: Array + 'static>(
    array: Option<&'a ArrayRef>,
    name: &str,
) -> Result<&'a T, String> {
    let array = array
        .and_then(|array| array.as_any().downcast_ref::<T>())
        .ok_or_else(|| {
            format!(
                "aircraft column `{name}` is missing or has the wrong Arrow type (expected {})",
                std::any::type_name::<T>()
            )
        })?;
    if array.null_count() != 0 {
        return Err(format!("aircraft column `{name}` contains null values"));
    }
    Ok(array)
}

pub fn col_i64<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}

pub fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch.column_by_name(name)?.as_any().downcast_ref()
}
