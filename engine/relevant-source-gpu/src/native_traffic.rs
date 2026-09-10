//! Raw airport event energy becomes mean period power using its own immutable class calendar.
use super::*;
use arrow::array::{FixedSizeListArray, Float32Array};
use noise_compute::emission::aircraft::{
    ClassWeights, GROUND_OPS_SOURCE_HEIGHT_M, NUM_CLASSES, PERIOD_SECONDS, SAMPLE_DAYS_BY_CLASS_KEY,
};

pub(super) struct TrafficCalendar {
    days: u16,
    weights: ClassWeights,
}

impl TrafficCalendar {
    pub(super) fn read(batch: &RecordBatch) -> Result<Self> {
        source_reader::aircraft_v6::assert_airport_traffic_contract(
            "airport_traffic.arrow",
            std::slice::from_ref(batch),
        )
        .map_err(anyhow::Error::msg)?;
        let schema = batch.schema();
        let days = schema
            .metadata()
            .get("n_days")
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|v| *v > 0)
            .context("airport traffic needs n_days")?;
        let weights = ClassWeights::parse(
            schema
                .metadata()
                .get(SAMPLE_DAYS_BY_CLASS_KEY)
                .map(String::as_str),
            days,
        )
        .map_err(anyhow::Error::msg)?;
        Ok(Self { days, weights })
    }
}

pub(super) fn traffic_row(
    batch: &RecordBatch,
    row: usize,
    frame: &RegionMetricFrame,
    calendar: &TrafficCalendar,
) -> Result<Option<DeviceLineSource>> {
    for name in ["period", "veh_kind", "class_idx", "ops_kind"] {
        ensure!(
            col_u8(batch, name).is_some_and(|c| !c.is_null(row)),
            "missing traffic {name}"
        );
    }
    let period = usize::from(byte(batch, "period", row));
    let vehicle = byte(batch, "veh_kind", row);
    ensure!(
        period < 3 && vehicle <= 1,
        "invalid airport traffic period or vehicle kind"
    );
    ensure!(
        usize::from(byte(batch, "class_idx", row))
            < if vehicle == 0 {
                NUM_CLASSES
            } else {
                noise_compute::compute::aircraft_v6::NUM_GSE_CLASSES
            },
        "invalid airport vehicle class"
    );
    let bands = batch
        .column_by_name("band_energy_lin")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .context("missing airport band energy")?;
    ensure!(
        bands.value_length() == 8 && !bands.is_null(row),
        "invalid airport band width or null row"
    );
    let values = bands.value(row);
    let values = values
        .as_any()
        .downcast_ref::<Float32Array>()
        .context("invalid airport energy type")?;
    let weight = if vehicle == 0 {
        calendar.weights.get(byte(batch, "class_idx", row))
    } else {
        1.0
    };
    let mut emission_linear = [0.0; 24];
    for band in 0..8 {
        ensure!(
            !values.is_null(band) && values.value(band).is_finite() && values.value(band) >= 0.0,
            "invalid airport band value"
        );
        emission_linear[period * 8 + band] = (f64::from(values.value(band)) * weight
            / (f64::from(calendar.days) * PERIOD_SECONDS[period]))
            as f32;
    }
    let start = position(batch, row, "start")?;
    let end = position(batch, row, "end")?;
    let [start_x_m, start_y_m] = frame.encode(start[0], start[1]);
    let [end_x_m, end_y_m] = frame.encode(end[0], end[1]);
    Ok(Some(DeviceLineSource {
        start_x_m,
        start_y_m,
        end_x_m,
        end_y_m,
        extent_m: float(batch, "length_m", row)
            .filter(|v| *v > 0.0)
            .context("invalid airport segment length")?,
        max_distance_m: noise_compute::constants::ground_ops_max_radius(byte(
            batch, "ops_kind", row,
        )) as f32,
        source_height_m: GROUND_OPS_SOURCE_HEIGHT_M as f32,
        flags: if vehicle == 0 {
            SOURCE_FLAG_GROUND_OPS_AIRCRAFT
        } else {
            SOURCE_FLAG_GROUND_OPS_GSE
        },
        emission_linear,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Schema;
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn even_empty_airport_files_require_current_contract_and_valid_calendar() {
        use square_store::aircraft_contract::{AIRPORT_TRAFFIC_CONTRACT, SCHEMA_VERSION};
        let mut metadata: HashMap<String, String> = [
            ("schema_version".into(), SCHEMA_VERSION.into()),
            (
                "airport_traffic_contract".into(),
                AIRPORT_TRAFFIC_CONTRACT.into(),
            ),
            ("n_days".into(), "14".into()),
            (
                SAMPLE_DAYS_BY_CLASS_KEY.into(),
                (0..NUM_CLASSES).map(|_| "14").collect::<Vec<_>>().join(","),
            ),
        ]
        .into();
        let batch =
            |metadata| RecordBatch::new_empty(Arc::new(Schema::empty().with_metadata(metadata)));
        assert!(TrafficCalendar::read(&batch(metadata.clone())).is_ok());
        metadata.remove("airport_traffic_contract");
        assert!(TrafficCalendar::read(&batch(metadata.clone())).is_err());
        metadata.insert(
            "airport_traffic_contract".into(),
            AIRPORT_TRAFFIC_CONTRACT.into(),
        );
        metadata.insert("n_days".into(), "0".into());
        assert!(TrafficCalendar::read(&batch(metadata)).is_err());
    }
}
