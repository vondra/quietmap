//! The day's flight table (`flights/<day>.arrow`), one row per flight: what the popup's top flights
//! show (address, callsign, type, start time), the provider, and what the altitudes rest on.

use super::flights::Flight;
use super::output::{designators, field, finished};
use arrow_array::ArrayRef;
use arrow_array::builder::{
    Float32Builder, StringBuilder, UInt8Builder, UInt32Builder, UInt64Builder,
};
use arrow_schema::{DataType, Schema};
use std::collections::HashMap;
use std::sync::Arc;

/// The flight table row of one flight.
pub struct FlightRow {
    pub flight_id: u64,
    pub address: String,
    pub callsign: String,
    pub aircraft_type: [u8; 4],
    pub start_time: u32,
    pub end_time: u32,
    pub source_id: u8,
    pub vehicle_kind: u8,
    pub profile: u8,
    pub ground_vehicle_class: u8,
    pub emitter_category: u8,
    pub altitude_source: u8,
    pub departure_field_m: f32,
    pub segments: u32,
}

impl FlightRow {
    pub fn of(
        flight: &Flight,
        altitude_source: u8,
        departure_field_m: f32,
        segments: usize,
    ) -> Self {
        let mut aircraft_type = [0u8; 4];
        let length = flight.aircraft_type.len().min(4);
        aircraft_type[..length].copy_from_slice(&flight.aircraft_type.as_bytes()[..length]);
        FlightRow {
            flight_id: flight.flight_id,
            address: flight.address.clone(),
            callsign: flight.callsign.clone(),
            aircraft_type,
            start_time: flight.points.first().map_or(0, |p| p.timestamp as u32),
            end_time: flight.points.last().map_or(0, |p| p.timestamp as u32),
            source_id: flight.source_id,
            vehicle_kind: flight.vehicle_kind,
            profile: flight.profile,
            ground_vehicle_class: flight.ground_vehicle_class,
            emitter_category: flight.emitter_category,
            altitude_source,
            departure_field_m,
            segments: segments as u32,
        }
    }
}

const FLIGHT_U8_COLUMNS: [&str; 6] = [
    "source_id",
    "veh_kind",
    "profile_idx",
    "gse_class",
    "emitter_category",
    "altitude_source",
];

pub fn flights_schema(metadata: HashMap<String, String>) -> Schema {
    let mut fields = vec![
        field("flight_id", DataType::UInt64),
        field("address", DataType::Utf8),
        field("callsign", DataType::Utf8),
        field("aircraft_type", DataType::FixedSizeBinary(4)),
        field("start_time", DataType::UInt32),
        field("end_time", DataType::UInt32),
    ];
    fields.extend(
        FLIGHT_U8_COLUMNS
            .iter()
            .map(|name| field(name, DataType::UInt8)),
    );
    fields.push(field("departure_field_elev_m", DataType::Float32));
    fields.push(field("segments", DataType::UInt32));
    Schema::new(fields).with_metadata(metadata)
}

#[derive(Default)]
pub struct FlightColumns {
    rows: usize,
    flight_id: UInt64Builder,
    address: StringBuilder,
    callsign: StringBuilder,
    aircraft_type: Vec<[u8; 4]>,
    times: [UInt32Builder; 2],
    bytes: [UInt8Builder; 6],
    departure_field: Float32Builder,
    segments: UInt32Builder,
}

impl FlightColumns {
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn push(&mut self, row: &FlightRow) {
        let f = self;
        f.flight_id.append_value(row.flight_id);
        f.address.append_value(&row.address);
        f.callsign.append_value(&row.callsign);
        f.aircraft_type.push(row.aircraft_type);
        f.times[0].append_value(row.start_time);
        f.times[1].append_value(row.end_time);
        let bytes = [
            row.source_id,
            row.vehicle_kind,
            row.profile,
            row.ground_vehicle_class,
            row.emitter_category,
            row.altitude_source,
        ];
        for (builder, value) in f.bytes.iter_mut().zip(bytes) {
            builder.append_value(value);
        }
        f.departure_field.append_value(row.departure_field_m);
        f.segments.append_value(row.segments);
        f.rows += 1;
    }

    pub fn finish(mut self) -> Result<Vec<ArrayRef>, String> {
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(self.flight_id.finish()),
            Arc::new(self.address.finish()),
            Arc::new(self.callsign.finish()),
            designators(&self.aircraft_type)?,
        ];
        columns.extend(finished(&mut self.times));
        columns.extend(finished(&mut self.bytes));
        columns.push(Arc::new(self.departure_field.finish()));
        columns.push(Arc::new(self.segments.finish()));
        Ok(columns)
    }
}
