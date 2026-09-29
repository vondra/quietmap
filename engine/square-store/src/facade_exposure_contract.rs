//! `facade_exposure.arrow`: per enclosed building a square owns, its noisiest façade receiver.
//!
//! One writer, the façade-exposure stage (`relevant-source-gpu`, bin
//! `facade-exposure`), after structures, sources, rasters and meteorology are
//! final. Readers: the popup (the receiver it recomputes exactly), the painter
//! (the per-layer powers it paints into the building's pixels) and validation.
//! A row per enclosed footprint of the square's `structures.arrow`, keyed by its
//! `screening_ordinal` (= the obstacle index id). `facade_points == 0` marks a
//! building with no exposed façade: its pixels and its popup are "not assessed".

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, UInt32Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

pub const FACADE_EXPOSURE_ARROW: &str = "facade_exposure.arrow";
pub const CONTRACT_KEY: &str = "facade_exposure_contract";
pub const CONTRACT: &str = "facade_exposure_v1";
/// Comma-separated source layer names of `layer_period_power`, in stored order.
pub const LAYER_ORDER_KEY: &str = "layer_order";
/// SHA-256 (hex) and byte length of the square's `structures.arrow` the receivers came from.
pub const STRUCTURES_SHA256_KEY: &str = "structures_sha256";
pub const STRUCTURES_BYTES_KEY: &str = "structures_bytes";
/// The surface format and physics generation of the kernels that evaluated the receivers.
pub const PHYSICS_GENERATION_KEY: &str = "physics_generation";
/// SHA-256 (hex) of the input manifest the stage read its inputs through.
pub const INPUT_MANIFEST_SHA256_KEY: &str = "input_manifest_sha256";
pub const PERIOD_COUNT: usize = 3;

/// One enclosed building's exposure.
#[derive(Clone, Debug, PartialEq)]
pub struct FacadeExposureRow {
    pub footprint_id: u32,
    /// Exposed façade receivers compared (§2.8 receivers not inside a building).
    pub facade_points: u32,
    /// The noisiest of them; `None` exactly when `facade_points == 0`.
    pub chosen: Option<ChosenFacadeReceiver>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChosenFacadeReceiver {
    /// Position in the building's canonical exposed-receiver order.
    pub canonical_index: u32,
    pub gx: i32,
    pub gy: i32,
    pub ground_altitude_m: f32,
    pub outward_bearing_deg: f32,
    /// Mean power per layer (in `layer_order`) and period (day, evening, night).
    pub layer_period_power: Vec<f32>,
    pub total_lden_db: f32,
    /// The second-loudest receiver's Lden (diagnostic); `None` with one receiver.
    pub runner_up_lden_db: Option<f32>,
}

fn power_field(layer_count: usize) -> Field {
    Field::new(
        "layer_period_power",
        DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, false)),
            (layer_count * PERIOD_COUNT) as i32,
        ),
        true,
    )
}

pub fn schema(layer_count: usize, metadata: HashMap<String, String>) -> Schema {
    Schema::new(vec![
        Field::new("footprint_id", DataType::UInt32, false),
        Field::new("facade_points", DataType::UInt32, false),
        Field::new("receiver_index", DataType::UInt32, true),
        Field::new("receiver_gx", DataType::Int32, true),
        Field::new("receiver_gy", DataType::Int32, true),
        Field::new("receiver_ground_m", DataType::Float32, true),
        Field::new("outward_bearing_deg", DataType::Float32, true),
        power_field(layer_count),
        Field::new("total_lden_db", DataType::Float32, true),
        Field::new("runner_up_lden_db", DataType::Float32, true),
    ])
    .with_metadata(metadata)
}

/// The rows as columns of [`schema`], in the given order.
pub fn columns(rows: &[FacadeExposureRow], layer_count: usize) -> Result<Vec<ArrayRef>, String> {
    let chosen = |f: &dyn Fn(&ChosenFacadeReceiver) -> f32| -> ArrayRef {
        Arc::new(rows.iter().map(|row| row.chosen.as_ref().map(f)).collect::<Float32Array>())
    };
    let width = layer_count * PERIOD_COUNT;
    let mut powers = Vec::with_capacity(rows.len() * width);
    for row in rows {
        match &row.chosen {
            Some(chosen) if chosen.layer_period_power.len() == width => {
                powers.extend_from_slice(&chosen.layer_period_power)
            }
            Some(_) => return Err(format!("footprint {}: wrong power count", row.footprint_id)),
            None => powers.extend(std::iter::repeat_n(0.0, width)),
        }
    }
    let power_nulls = arrow::buffer::NullBuffer::from(
        rows.iter().map(|row| row.chosen.is_some()).collect::<Vec<_>>(),
    );
    let DataType::FixedSizeList(item, size) = power_field(layer_count).data_type().clone() else {
        unreachable!("power field is a fixed-size list")
    };
    let power = FixedSizeListArray::try_new(
        item,
        size,
        Arc::new(Float32Array::from(powers)),
        Some(power_nulls),
    )
    .map_err(|error| error.to_string())?;
    Ok(vec![
        Arc::new(rows.iter().map(|row| row.footprint_id).collect::<UInt32Array>()),
        Arc::new(rows.iter().map(|row| row.facade_points).collect::<UInt32Array>()),
        Arc::new(
            rows.iter()
                .map(|row| row.chosen.as_ref().map(|c| c.canonical_index))
                .collect::<UInt32Array>(),
        ),
        Arc::new(rows.iter().map(|row| row.chosen.as_ref().map(|c| c.gx)).collect::<Int32Array>()),
        Arc::new(rows.iter().map(|row| row.chosen.as_ref().map(|c| c.gy)).collect::<Int32Array>()),
        chosen(&|c| c.ground_altitude_m),
        chosen(&|c| c.outward_bearing_deg),
        Arc::new(power),
        chosen(&|c| c.total_lden_db),
        Arc::new(
            rows.iter()
                .map(|row| row.chosen.as_ref().and_then(|c| c.runner_up_lden_db))
                .collect::<Float32Array>(),
        ),
    ])
}

/// Refuse another contract or grid before any row is read.
pub fn validate_schema(schema: &Schema) -> Result<(), String> {
    for (key, expected) in [
        (CONTRACT_KEY, CONTRACT),
        ("grid", crate::store::GRID_CONTRACT_Z30),
    ] {
        let found = schema.metadata().get(key).map(String::as_str);
        if found != Some(expected) {
            return Err(format!(
                "{FACADE_EXPOSURE_ARROW} {key} expected {expected}, found {found:?}"
            ));
        }
    }
    Ok(())
}

/// The layer names the power columns are stored in.
pub fn layer_order(schema: &Schema) -> Result<Vec<&str>, String> {
    schema
        .metadata()
        .get(LAYER_ORDER_KEY)
        .map(|order| order.split(',').collect())
        .ok_or_else(|| format!("{FACADE_EXPOSURE_ARROW} has no {LAYER_ORDER_KEY}"))
}

/// Every row of one batch.
pub fn rows(batch: &RecordBatch) -> Result<Vec<FacadeExposureRow>, String> {
    validate_schema(batch.schema_ref())?;
    fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
        batch
            .column_by_name(name)
            .and_then(|column| column.as_any().downcast_ref::<T>())
            .ok_or_else(|| format!("{FACADE_EXPOSURE_ARROW}: bad column {name}"))
    }
    let ids = column::<UInt32Array>(batch, "footprint_id")?;
    let points = column::<UInt32Array>(batch, "facade_points")?;
    let indexes = column::<UInt32Array>(batch, "receiver_index")?;
    let gxs = column::<Int32Array>(batch, "receiver_gx")?;
    let gys = column::<Int32Array>(batch, "receiver_gy")?;
    let grounds = column::<Float32Array>(batch, "receiver_ground_m")?;
    let bearings = column::<Float32Array>(batch, "outward_bearing_deg")?;
    let powers = column::<FixedSizeListArray>(batch, "layer_period_power")?;
    let totals = column::<Float32Array>(batch, "total_lden_db")?;
    let runners_up = column::<Float32Array>(batch, "runner_up_lden_db")?;
    let power_values = powers
        .values()
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| format!("{FACADE_EXPOSURE_ARROW}: powers are not f32"))?;
    let width = powers.value_length() as usize;
    (0..batch.num_rows())
        .map(|row| {
            let facade_points = points.value(row);
            let chosen = (!indexes.is_null(row)).then(|| ChosenFacadeReceiver {
                canonical_index: indexes.value(row),
                gx: gxs.value(row),
                gy: gys.value(row),
                ground_altitude_m: grounds.value(row),
                outward_bearing_deg: bearings.value(row),
                layer_period_power: power_values.values()[row * width..(row + 1) * width].to_vec(),
                total_lden_db: totals.value(row),
                runner_up_lden_db: (!runners_up.is_null(row)).then(|| runners_up.value(row)),
            });
            if chosen.is_some() != (facade_points > 0)
                || (chosen.is_some()
                    && (gxs.is_null(row)
                        || gys.is_null(row)
                        || powers.is_null(row)
                        || totals.is_null(row)))
            {
                return Err(format!(
                    "{FACADE_EXPOSURE_ARROW}: footprint {} is inconsistent",
                    ids.value(row)
                ));
            }
            Ok(FacadeExposureRow {
                footprint_id: ids.value(row),
                facade_points,
                chosen,
            })
        })
        .collect()
}

/// The row of one footprint among the given batches, `None` when absent.
pub fn find_row(batches: &[RecordBatch], footprint_id: u32) -> Result<Option<FacadeExposureRow>, String> {
    for batch in batches {
        let ids = batch
            .column_by_name("footprint_id")
            .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| format!("{FACADE_EXPOSURE_ARROW}: bad column footprint_id"))?;
        if let Some(row) = ids.values().iter().position(|id| *id == footprint_id) {
            return Ok(rows(&batch.slice(row, 1))?.pop());
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_roundtrip_through_columns_including_a_building_without_an_exposed_facade() {
        let rows = vec![
            FacadeExposureRow {
                footprint_id: 7,
                facade_points: 12,
                chosen: Some(ChosenFacadeReceiver {
                    canonical_index: 3,
                    gx: 10,
                    gy: -20,
                    ground_altitude_m: 201.5,
                    outward_bearing_deg: 135.0,
                    layer_period_power: (0..6).map(|v| v as f32).collect(),
                    total_lden_db: 61.2,
                    runner_up_lden_db: Some(58.0),
                }),
            },
            FacadeExposureRow {
                footprint_id: 9,
                facade_points: 0,
                chosen: None,
            },
        ];
        let metadata = HashMap::from([
            (CONTRACT_KEY.to_string(), CONTRACT.to_string()),
            ("grid".to_string(), crate::store::GRID_CONTRACT_Z30.to_string()),
            (LAYER_ORDER_KEY.to_string(), "road,rail".to_string()),
        ]);
        let batch =
            RecordBatch::try_new(Arc::new(schema(2, metadata)), columns(&rows, 2).unwrap()).unwrap();
        assert_eq!(super::rows(&batch).unwrap(), rows);
        assert_eq!(find_row(std::slice::from_ref(&batch), 9).unwrap(), Some(rows[1].clone()));
        assert_eq!(find_row(std::slice::from_ref(&batch), 8).unwrap(), None);
        assert_eq!(layer_order(batch.schema_ref()).unwrap(), ["road", "rail"]);
    }
}
