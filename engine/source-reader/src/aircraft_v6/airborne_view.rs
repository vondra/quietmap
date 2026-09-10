//! Borrow prepared airborne sub-segment rows and each file's flight dictionary without copying.

use super::columns::required_array;
use arrow::{array::*, datatypes::Int32Type, record_batch::RecordBatch};
use noise_compute::compute::aircraft_v6::{AirborneFlightTable, AirborneSegmentBatch};

pub struct AirborneRowAccum<'a> {
    batches: Vec<AirborneSegmentBatch<'a>>,
}

impl<'a> AirborneRowAccum<'a> {
    pub fn new(batches: &'a [RecordBatch]) -> Result<Self, String> {
        batches
            .iter()
            .map(decode_batch)
            .collect::<Result<_, _>>()
            .map(|batches| Self { batches })
    }

    pub fn views(&self) -> &[AirborneSegmentBatch<'a>] {
        &self.batches
    }
}

fn decode_batch(batch: &RecordBatch) -> Result<AirborneSegmentBatch<'_>, String> {
    let flight =
        required_array::<DictionaryArray<Int32Type>>(batch.column_by_name("flight"), "flight")?;
    let identity = required_array::<StructArray>(Some(flight.values()), "flight.values")?;
    let callsign =
        required_array::<StringArray>(identity.column_by_name("callsign"), "flight.callsign")?;
    let aircraft_type = required_array::<FixedSizeBinaryArray>(
        identity.column_by_name("aircraft_type"),
        "flight.aircraft_type",
    )?;
    if aircraft_type.value_length() != 4 {
        return Err("airborne flight.aircraft_type must be FixedSizeBinary(4)".into());
    }
    let flights = AirborneFlightTable {
        callsign_offsets: callsign.value_offsets(),
        callsign_bytes: callsign.value_data(),
        aircraft_type: aircraft_type.value_data(),
        profile_idx: required_array::<UInt8Array>(
            identity.column_by_name("profile_idx"),
            "flight.profile_idx",
        )?
        .values(),
        source_id: required_array::<UInt8Array>(
            identity.column_by_name("source_id"),
            "flight.source_id",
        )?
        .values(),
        origin: required_array::<UInt8Array>(identity.column_by_name("origin"), "flight.origin")?
            .values(),
    };
    // The kernel indexes the table by key without a bounds branch per row.
    let flight_key = flight.keys().values();
    if flight_key.iter().any(|&key| {
        usize::try_from(key)
            .is_ok_and(|key| key < flights.len())
            .not()
    }) {
        return Err("airborne flight key outside the file's flight dictionary".into());
    }
    let i32s = |name: &str| -> Result<&[i32], String> {
        Ok(required_array::<Int32Array>(batch.column_by_name(name), name)?.values())
    };
    let i16s = |name: &str| -> Result<&[i16], String> {
        Ok(required_array::<Int16Array>(batch.column_by_name(name), name)?.values())
    };
    let u8s = |name: &str| -> Result<&[u8], String> {
        Ok(required_array::<UInt8Array>(batch.column_by_name(name), name)?.values())
    };
    let f32s = |name: &str| -> Result<&[f32], String> {
        Ok(required_array::<Float32Array>(batch.column_by_name(name), name)?.values())
    };
    Ok(AirborneSegmentBatch {
        flight_id: required_array::<UInt64Array>(batch.column_by_name("flight_id"), "flight_id")?
            .values(),
        flight_key,
        flights,
        start_gy: i32s("start_gy")?,
        start_gx: i32s("start_gx")?,
        start_alt_m: i16s("start_alt_m")?,
        end_gy: i32s("end_gy")?,
        end_gx: i32s("end_gx")?,
        end_alt_m: i16s("end_alt_m")?,
        speed_kt: f32s("speed_kt")?,
        length_m: f32s("length_m")?,
        period: u8s("period")?,
        date_id: i16s("date_id")?,
        flags: u8s("flags")?,
        terrain_start_elev_m: i16s("terrain_start_elev_m")?,
        terrain_end_elev_m: i16s("terrain_end_elev_m")?,
    })
}

use std::ops::Not;
