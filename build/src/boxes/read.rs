//! One day's flight segments from Stage 1's scratch (`segments/<day>.arrow`): the columns the
//! boxes need, one struct per airborne or cruise segment (ground segments go to ground operations).

use crate::dev4::read_table;
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt8Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use std::path::Path;

pub use crate::aircraft::{
    HELICOPTER_DESCENT as FLAG_HELICOPTER_DESCENT, IS_DEPARTURE as FLAG_DEPARTURE,
    ON_GROUND as FLAG_ON_GROUND, SECONDARY_ONLY as FLAG_SECONDARY_ONLY,
};
/// Stage 1 phases: ground segments are not boxed.
const PHASE_GROUND: u8 = 0;

/// One segment as the boxes read it.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightSegment {
    /// ICAO address in bits 63-40 and the flight's start (Unix seconds) in bits 31-0 (dev4).
    pub flight_id: u64,
    pub callsign: [u8; 8],
    pub designator: [u8; 4],
    pub source_id: u8,
    pub period: u8,
    pub flags: u8,
    /// Latitude, longitude (deg) and altitude above sea level (EGM2008, m) of both ends.
    pub start: [f64; 3],
    pub end: [f64; 3],
    /// Barometric (pressure) altitude of both ends (m): the thrust model's ISA state.
    pub pressure_altitude_m: [f64; 2],
    pub speed_kt: f64,
    /// Height above the ground under the segment (m) and the flight's departure field elevation
    /// (m; NaN when its takeoff roll was not seen).
    pub above_ground_m: f64,
    pub departure_field_m: f64,
    /// Terrain under both ends (m above sea level): the kernel's Filter D.
    pub ground_m: [f64; 2],
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|column| column.as_ref())
        .ok_or_else(|| format!("segments: no column {name}"))
}

fn padded<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [b' '; N];
    for (slot, byte) in out.iter_mut().zip(bytes.iter().filter(|byte| **byte != 0)) {
        *slot = *byte;
    }
    out
}

/// The airborne and cruise segments of the day file at `path` (none when it is absent).
pub fn read_segments(path: &Path) -> Result<Vec<FlightSegment>, String> {
    let Some(table) = read_table(path)? else {
        return Ok(Vec::new());
    };
    let mut segments = Vec::new();
    for batch in &table.batches {
        let bytes = |name| column(batch, name).map(|c| c.as_primitive::<UInt8Type>());
        let floats = |name| column(batch, name).map(|c| c.as_primitive::<Float32Type>());
        let (source_id, period, phase, flags) = (
            bytes("source_id")?,
            bytes("period")?,
            bytes("phase")?,
            bytes("flags")?,
        );
        let flight_id = column(batch, "flight_id")?.as_primitive::<UInt64Type>();
        let callsign = column(batch, "callsign")?.as_string::<i32>();
        let designator = column(batch, "aircraft_type")?.as_fixed_size_binary();
        let names = [
            "start_lat",
            "start_lon",
            "start_alt_msl_m",
            "end_lat",
            "end_lon",
            "end_alt_msl_m",
            "start_alt_m",
            "end_alt_m",
            "speed_kt",
            "agl_avg_m",
            "departure_field_elev_m",
            "start_elev_m",
            "end_elev_m",
        ];
        let values = names
            .iter()
            .map(|name| floats(name))
            .collect::<Result<Vec<_>, String>>()?;
        for row in 0..batch.num_rows() {
            if phase.value(row) == PHASE_GROUND {
                continue;
            }
            let value = |index: usize| f64::from(values[index].value(row));
            segments.push(FlightSegment {
                flight_id: flight_id.value(row),
                callsign: padded(callsign.value(row).as_bytes()),
                designator: padded(designator.value(row)),
                source_id: source_id.value(row),
                period: period.value(row),
                flags: flags.value(row),
                start: [value(0), value(1), value(2)],
                end: [value(3), value(4), value(5)],
                pressure_altitude_m: [value(6), value(7)],
                speed_kt: value(8),
                above_ground_m: value(9),
                departure_field_m: value(10),
                ground_m: [value(11), value(12)],
            });
        }
    }
    Ok(segments)
}
