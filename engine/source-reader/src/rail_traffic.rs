//! Strict prepared railway traffic decoding shared by popup and surface loaders.
use arrow::array::{Array, Float64Array, UInt16Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::normalize::{RailCategoryTraffic, RailTraffic};

pub struct RailTrafficColumns<'a> {
    counts: [&'a Float64Array; 6],
    status: [&'a UInt8Array; 2],
    source: [&'a UInt16Array; 2],
    matching: [&'a UInt8Array; 2],
}

fn required<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    let value = batch
        .column_by_name(name)
        .and_then(|value| value.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("railway traffic column {name} missing or wrong Arrow type"))?;
    if value.null_count() != 0 {
        return Err(format!("railway traffic column {name} contains nulls"));
    }
    Ok(value)
}

impl<'a> RailTrafficColumns<'a> {
    pub fn read(batch: &'a RecordBatch) -> Result<Self, String> {
        if batch
            .schema()
            .metadata()
            .get("rail_traffic_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err("railways.arrow requires rail_traffic_contract=1".to_owned());
        }
        let result = Self {
            counts: [
                required(batch, "trains_passenger_day")?,
                required(batch, "trains_passenger_evening")?,
                required(batch, "trains_passenger_night")?,
                required(batch, "trains_freight_day")?,
                required(batch, "trains_freight_evening")?,
                required(batch, "trains_freight_night")?,
            ],
            status: [
                required(batch, "passenger_status")?,
                required(batch, "freight_status")?,
            ],
            source: [
                required(batch, "passenger_source_id")?,
                required(batch, "freight_source_id")?,
            ],
            matching: [
                required(batch, "passenger_matching")?,
                required(batch, "freight_matching")?,
            ],
        };
        for row in 0..batch.num_rows() {
            if result
                .counts
                .iter()
                .any(|column| !column.value(row).is_finite() || column.value(row) < 0.0)
                || result.status.iter().any(|column| column.value(row) > 2)
                || result.matching.iter().any(|column| column.value(row) > 3)
            {
                return Err(format!("invalid prepared railway traffic at row {row}"));
            }
        }
        Ok(result)
    }

    pub fn row(&self, row: usize) -> RailTraffic {
        let category = |index: usize| RailCategoryTraffic {
            periods: std::array::from_fn(|period| self.counts[index * 3 + period].value(row)),
            status: self.status[index].value(row),
            source_id: self.source[index].value(row),
            matching: self.matching[index].value(row),
        };
        RailTraffic {
            passenger: category(0),
            freight: category(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn rail_traffic_contract_rejects_missing_typed_null_and_invalid_values() {
        let names = [
            "trains_passenger_day",
            "trains_passenger_evening",
            "trains_passenger_night",
            "trains_freight_day",
            "trains_freight_evening",
            "trains_freight_night",
        ];
        let mut fields: Vec<_> = names
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect();
        let mut columns: Vec<Arc<dyn Array>> = (0..6)
            .map(|_| Arc::new(Float64Array::from(vec![0.125])) as Arc<dyn Array>)
            .collect();
        for category in ["passenger", "freight"] {
            for (suffix, data_type, column) in [
                (
                    "status",
                    DataType::UInt8,
                    Arc::new(UInt8Array::from(vec![2])) as Arc<dyn Array>,
                ),
                (
                    "source_id",
                    DataType::UInt16,
                    Arc::new(UInt16Array::from(vec![7])) as Arc<dyn Array>,
                ),
                (
                    "matching",
                    DataType::UInt8,
                    Arc::new(UInt8Array::from(vec![3])) as Arc<dyn Array>,
                ),
            ] {
                fields.push(Field::new(format!("{category}_{suffix}"), data_type, false));
                columns.push(column);
            }
        }
        let schema = Schema::new(fields).with_metadata(std::collections::HashMap::from([(
            "rail_traffic_contract".to_owned(),
            "1".to_owned(),
        )]));
        let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns.clone()).unwrap();
        assert_eq!(
            RailTrafficColumns::read(&batch)
                .unwrap()
                .row(0)
                .passenger
                .periods,
            [0.125; 3]
        );
        assert!(RailTrafficColumns::read(
            &RecordBatch::try_new(
                Arc::new(Schema::new(schema.fields().clone())),
                columns.clone()
            )
            .unwrap()
        )
        .is_err());
        assert!(
            RailTrafficColumns::read(&batch.project(&(1..12).collect::<Vec<_>>()).unwrap())
                .is_err()
        );
        for value in [Some(f64::NAN), Some(f64::INFINITY), Some(-0.25), None] {
            let mut invalid = columns.clone();
            invalid[0] = Arc::new(Float64Array::from(vec![value]));
            let mut fields = schema.fields().to_vec();
            fields[0] = Arc::new(Field::new(names[0], DataType::Float64, true));
            let invalid_schema = Schema::new(fields).with_metadata(schema.metadata().clone());
            let invalid = RecordBatch::try_new(Arc::new(invalid_schema), invalid).unwrap();
            assert!(RailTrafficColumns::read(&invalid).is_err());
        }
        let mut fields = schema.fields().to_vec();
        fields[0] = Arc::new(Field::new(names[0], DataType::Int32, false));
        columns[0] = Arc::new(arrow::array::Int32Array::from(vec![1]));
        let invalid = RecordBatch::try_new(
            Arc::new(Schema::new(fields).with_metadata(schema.metadata().clone())),
            columns,
        )
        .unwrap();
        assert!(RailTrafficColumns::read(&invalid).is_err());
    }
}
