//! Current prepared screening heights and their sources, shared by the obstacle index, the finalize step and the footprint overlay.

use arrow::array::{Array, Int16Array, UInt8Array};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

/// Mirrors the Python producer; actual IPC roundtrips protect this boundary.
pub const CONTRACT: &str = "structures_v5";

/// `height_source` codes, mirroring `scripts/structures/structure_contract.py`.
pub const HEIGHT_SOURCE_AREA_TYPOLOGY: u8 = 2;
pub const HEIGHT_SOURCE_GHSL: u8 = 4;
pub const HEIGHT_SOURCE_GROUND_ACTIVITY: u8 = 7;

/// A footprint-area typology or a 100 m cell average knows nothing about the
/// individual shed under it; every other source measured or mapped the building.
pub fn height_is_per_building(height_source: u8) -> bool {
    !matches!(
        height_source,
        HEIGHT_SOURCE_AREA_TYPOLOGY | HEIGHT_SOURCE_GHSL
    )
}

pub fn validate_schema(schema: &Schema) -> Result<(), String> {
    for (key, expected) in [
        ("structures_contract", CONTRACT),
        ("grid", crate::store::GRID_CONTRACT_Z30),
    ] {
        let actual = schema.metadata().get(key).map(String::as_str);
        if actual != Some(expected) {
            return Err(format!(
                "structures.arrow {key} mismatch (expected {expected}, got {actual:?})"
            ));
        }
    }
    let height = schema
        .field_with_name("height_m")
        .map_err(|_| "structures.arrow missing required height_m".to_string())?;
    if height.data_type() != &DataType::Int16 || height.is_nullable() {
        return Err("structures.arrow height_m must be non-null Int16 metres".to_string());
    }
    Ok(())
}

pub fn heights(batch: &RecordBatch) -> Result<&Int16Array, String> {
    validate_schema(batch.schema_ref())?;
    let heights = batch
        .column_by_name("height_m")
        .and_then(|column| column.as_any().downcast_ref::<Int16Array>())
        .ok_or_else(|| "structures.arrow height_m must be Int16 metres".to_string())?;
    if heights.null_count() != 0 || heights.values().iter().any(|height| *height < 0) {
        return Err("structures.arrow height_m must be non-null and nonnegative".to_string());
    }
    Ok(heights)
}

/// The builder marks a ground activity or underground source (no wall, zero
/// screening height) with its own height source. Mapped sub-metre building
/// heights and open roofs round to zero too, but are real structures.
pub fn is_emission_only_area(batch: &RecordBatch, row: usize) -> bool {
    batch.column_by_name("height_source")
        .and_then(|column| column.as_any().downcast_ref::<UInt8Array>())
        .is_some_and(|source| {
            !source.is_null(row) && source.value(row) == HEIGHT_SOURCE_GROUND_ACTIVITY
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Field;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn schema(stamp: &str, dtype: DataType, nullable: bool) -> Schema {
        Schema::new(vec![Field::new("height_m", dtype, nullable)]).with_metadata(HashMap::from([
            ("structures_contract".to_string(), stamp.to_string()),
            ("grid".to_string(), "z30".to_string()),
        ]))
    }

    #[test]
    fn current_stamp_never_certifies_float_or_nullable_heights() {
        assert!(validate_schema(&schema(CONTRACT, DataType::Int16, false)).is_ok());
        for (stamp, dtype, nullable) in [
            ("structures_v2", DataType::Int16, false),
            (CONTRACT, DataType::Float32, false),
            (CONTRACT, DataType::Int16, true),
        ] {
            assert!(validate_schema(&schema(stamp, dtype, nullable)).is_err());
        }
    }

    #[test]
    fn negative_heights_are_not_a_quiet_empty_layer() {
        let batch = RecordBatch::try_new(
            Arc::new(schema(CONTRACT, DataType::Int16, false)),
            vec![Arc::new(Int16Array::from(vec![-1]))],
        )
        .unwrap();
        assert!(heights(&batch).is_err());
    }
}
