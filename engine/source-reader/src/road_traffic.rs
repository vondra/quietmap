//! Strict prepared road traffic decoding shared by popup and surface loaders.
use arrow::array::{Array, Float64Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::normalize::RoadTraffic;

pub struct RoadTrafficColumns<'a> {
    counts: [&'a Float64Array; 4],
    estimated: &'a UInt8Array,
}

fn required<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    let value = batch
        .column_by_name(name)
        .and_then(|value| value.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("road traffic column {name} missing or wrong Arrow type"))?;
    if value.null_count() != 0 {
        return Err(format!("road traffic column {name} contains nulls"));
    }
    Ok(value)
}

impl<'a> RoadTrafficColumns<'a> {
    pub fn read(batch: &'a RecordBatch) -> Result<Self, String> {
        if batch
            .schema()
            .metadata()
            .get("road_traffic_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err("roads.arrow requires road_traffic_contract=1".to_owned());
        }
        let result = Self {
            counts: [
                required(batch, "aadt_light")?,
                required(batch, "aadt_medium")?,
                required(batch, "aadt_heavy")?,
                required(batch, "aadt_moto")?,
            ],
            estimated: required(batch, "traffic_estimated")?,
        };
        for row in 0..batch.num_rows() {
            if result
                .counts
                .iter()
                .any(|column| !column.value(row).is_finite() || column.value(row) < 0.0)
                || result.estimated.value(row) > 15
            {
                return Err(format!("invalid prepared road traffic at row {row}"));
            }
        }
        Ok(result)
    }

    pub fn row(&self, row: usize) -> RoadTraffic {
        RoadTraffic {
            light: self.counts[0].value(row),
            medium: self.counts[1].value(row),
            heavy: self.counts[2].value(row),
            moto: self.counts[3].value(row),
            estimated: self.estimated.value(row),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn valid_batch() -> (Schema, Vec<Arc<dyn Array>>) {
        let names = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"];
        let mut fields: Vec<_> = names
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect();
        fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
        let columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![10_000.0])),
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(Float64Array::from(vec![500.0])),
            Arc::new(Float64Array::from(vec![12.5])),
            Arc::new(UInt8Array::from(vec![0b0101])),
        ];
        (
            Schema::new(fields).with_metadata(std::collections::HashMap::from([(
                "road_traffic_contract".to_owned(),
                "1".to_owned(),
            )])),
            columns,
        )
    }

    #[test]
    fn road_traffic_contract_rejects_invalid_final_arrow() {
        let (schema, columns) = valid_batch();
        let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns.clone()).unwrap();
        let traffic = RoadTrafficColumns::read(&batch).unwrap().row(0);
        assert_eq!(traffic.light, 10_000.0);
        assert_eq!(traffic.heavy, 500.0);
        assert_eq!(traffic.moto, 12.5);
        assert_eq!(traffic.estimated, 0b0101);
        // Missing contract metadata on an otherwise valid schema.
        assert!(RoadTrafficColumns::read(
            &RecordBatch::try_new(
                Arc::new(Schema::new(schema.fields().clone())),
                columns.clone()
            )
            .unwrap()
        )
        .is_err());
        // Dropped column.
        assert!(
            RoadTrafficColumns::read(&batch.project(&[0, 1, 2, 4]).unwrap()).is_err()
        );
        for value in [Some(f64::NAN), Some(f64::INFINITY), Some(-0.25), None] {
            let mut invalid = columns.clone();
            invalid[0] = Arc::new(Float64Array::from(vec![value]));
            let mut fields = schema.fields().to_vec();
            fields[0] = Arc::new(Field::new("aadt_light", DataType::Float64, true));
            let invalid_schema = Schema::new(fields).with_metadata(schema.metadata().clone());
            let invalid = RecordBatch::try_new(Arc::new(invalid_schema), invalid).unwrap();
            assert!(RoadTrafficColumns::read(&invalid).is_err());
        }
        // Out-of-domain bitmask.
        let mut invalid = columns.clone();
        invalid[4] = Arc::new(UInt8Array::from(vec![16]));
        let invalid = RecordBatch::try_new(Arc::new(schema.clone()), invalid).unwrap();
        assert!(RoadTrafficColumns::read(&invalid).is_err());
        // Wrong column type (the legacy Int32 traffic column).
        let mut fields = schema.fields().to_vec();
        fields[0] = Arc::new(Field::new("aadt_light", DataType::Int32, false));
        let mut typed = columns.clone();
        typed[0] = Arc::new(arrow::array::Int32Array::from(vec![1000]));
        let invalid = RecordBatch::try_new(
            Arc::new(Schema::new(fields).with_metadata(schema.metadata().clone())),
            typed,
        )
        .unwrap();
        assert!(RoadTrafficColumns::read(&invalid).is_err());
    }
}
