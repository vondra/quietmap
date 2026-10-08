//! The ground legs of one day's Stage 1 file (`segments/<day>.arrow`): the phase-0 segments of
//! aircraft and airport vehicles with the columns ground operations need (the boxes' reader keeps
//! the airborne ones). Only these columns are decoded.

use crate::aircraft::{IS_DEPARTURE, SECONDARY_ONLY};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt8Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use physics::doc29::profiles_generated::noise_class_of;
use physics::emission::airport::GroundVehicle;
use rayon::prelude::*;
use std::path::Path;

/// Stage 1 codes: the ground phase and the vehicle kinds.
const PHASE_GROUND: u8 = 0;
const KIND_AIRCRAFT: u8 = 0;
const KIND_GROUND_VEHICLE: u8 = 1;
const COLUMNS: [&str; 13] = [
    "flight_id",
    "profile_idx",
    "veh_kind",
    "gse_class",
    "period",
    "phase",
    "flags",
    "start_lat",
    "start_lon",
    "end_lat",
    "end_lon",
    "speed_kt",
    "agl_avg_m",
];
/// A flight's first or last airborne segment is a low end below this height above the ground (m).
pub const LOW_END_M: f32 = 150.0;

/// What moves along a leg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mover {
    /// An aircraft of a Doc 29 noise class.
    Aircraft {
        class: u8,
    },
    Vehicle(GroundVehicle),
}

/// One ground leg.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundLeg {
    /// ICAO address and start time of the flight (the unit of a movement).
    pub flight_id: u64,
    pub mover: Mover,
    pub period: u8,
    /// A take-off roll (Stage 1: accelerating faster than 60 kt/min).
    pub departure: bool,
    /// Only the secondary provider saw it: it counts on the increment days.
    pub secondary_only: bool,
    /// Latitude and longitude of both ends (deg).
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub speed_kt: f64,
}

/// A flight's first or last airborne segment when it is low: where it left or reached the
/// ground, seen from the air (a roll the ground receivers may have missed).
#[derive(Debug, Clone, PartialEq)]
pub struct LowEnd {
    pub flight_id: u64,
    pub class: u8,
    pub period: u8,
    /// The flight's first airborne segment (else its last).
    pub first: bool,
    /// Stage 1 took the segment for a departure (climbing).
    pub departure: bool,
    pub secondary_only: bool,
    /// Latitude and longitude of both ends (deg).
    pub start: [f64; 2],
    pub end: [f64; 2],
}

/// A day's ground legs and the low ends of its flights.
#[derive(Debug, Default)]
pub struct DayLegs {
    pub legs: Vec<GroundLeg>,
    pub low_ends: Vec<LowEnd>,
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|column| column.as_ref())
        .ok_or_else(|| format!("segments: no column {name}"))
}

/// The ground legs of the day file at `path` whose ends (latitude, longitude) `keep` accepts, and
/// the low ends of its aircraft flights there (a flight's segments follow one another in the
/// file). A missing day is an error, never a quiet day.
pub fn read_ground_legs(
    path: &Path,
    keep: &(dyn Fn([f64; 2], [f64; 2]) -> bool + Sync),
) -> Result<DayLegs, String> {
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let open = || {
        std::fs::File::open(path)
            .map(std::io::BufReader::new)
            .map_err(|e| failed(&e))
    };
    let schema = arrow_ipc::reader::FileReader::try_new(open()?, None)
        .map_err(|e| failed(&e))?
        .schema();
    let projection = COLUMNS
        .iter()
        .map(|name| schema.index_of(name).map_err(|e| failed(&e)))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = arrow_ipc::reader::FileReader::try_new(open()?, Some(projection))
        .map_err(|e| failed(&e))?;
    let mut day = DayLegs::default();
    // The current flight: its id, airborne segments so far, its first one, its last one and the
    // last one the primary provider saw (each kept only when low): where only the secondary
    // completed a landing, the primary's last low segment still witnesses it (Codex, review of
    // r055: one landing counted 697/354 times a day).
    type Flight = (u64, usize, Option<LowEnd>, Option<LowEnd>, Option<LowEnd>);
    let mut flight: Option<Flight> = None;
    let close = |flight: Option<Flight>, low: &mut Vec<LowEnd>| {
        if let Some((_, count, first, last, last_primary)) = flight {
            let last = last.filter(|_| count > 1);
            let last_primary = last_primary.filter(|end| count > 1 && Some(end) != last.as_ref());
            for end in [first, last, last_primary].into_iter().flatten() {
                if keep(end.start, end.end) {
                    low.push(end);
                }
            }
        }
    };
    for batch in reader {
        let batch = batch.map_err(|e| failed(&e))?;
        let bytes = |name| column(&batch, name).map(|c| c.as_primitive::<UInt8Type>());
        let (profile, kind, vehicle, period, phase, flags) = (
            bytes("profile_idx")?,
            bytes("veh_kind")?,
            bytes("gse_class")?,
            bytes("period")?,
            bytes("phase")?,
            bytes("flags")?,
        );
        let flight_id = column(&batch, "flight_id")?.as_primitive::<UInt64Type>();
        let floats = ["start_lat", "start_lon", "end_lat", "end_lon", "speed_kt"]
            .map(|name| column(&batch, name).map(|c| c.as_primitive::<Float32Type>()));
        let [start_lat, start_lon, end_lat, end_lon, speed] = floats;
        let (start_lat, start_lon, end_lat, end_lon, speed) =
            (start_lat?, start_lon?, end_lat?, end_lon?, speed?);
        let height = column(&batch, "agl_avg_m")?.as_primitive::<Float32Type>();
        for row in 0..batch.num_rows() {
            if phase.value(row) == PHASE_GROUND || kind.value(row) != KIND_AIRCRAFT {
                continue;
            }
            let id = flight_id.value(row);
            let end = |first: bool| {
                (height.value(row) < LOW_END_M).then(|| LowEnd {
                    flight_id: id,
                    class: noise_class_of(profile.value(row)),
                    period: period.value(row),
                    first,
                    departure: flags.value(row) & IS_DEPARTURE != 0,
                    secondary_only: flags.value(row) & SECONDARY_ONLY != 0,
                    start: [start_lat.value(row), start_lon.value(row)].map(f64::from),
                    end: [end_lat.value(row), end_lon.value(row)].map(f64::from),
                })
            };
            let primary = flags.value(row) & SECONDARY_ONLY == 0;
            match flight.as_mut() {
                Some((current, count, _, last, last_primary)) if *current == id => {
                    *count += 1;
                    *last = end(false);
                    if primary {
                        *last_primary = end(false);
                    }
                }
                _ => {
                    close(flight.take(), &mut day.low_ends);
                    let last_primary = if primary { end(false) } else { None };
                    flight = Some((id, 1, end(true), end(false), last_primary));
                }
            }
        }
        let kept: Vec<GroundLeg> = (0..batch.num_rows())
            .into_par_iter()
            .filter_map(|row| {
                if phase.value(row) != PHASE_GROUND {
                    return None;
                }
                let mover = match kind.value(row) {
                    KIND_AIRCRAFT => Mover::Aircraft {
                        class: noise_class_of(profile.value(row)),
                    },
                    KIND_GROUND_VEHICLE => {
                        Mover::Vehicle(GroundVehicle::from_code(vehicle.value(row))?)
                    }
                    _ => return None,
                };
                let start = [start_lat.value(row), start_lon.value(row)].map(f64::from);
                let end = [end_lat.value(row), end_lon.value(row)].map(f64::from);
                if !keep(start, end) {
                    return None;
                }
                Some(GroundLeg {
                    flight_id: flight_id.value(row),
                    mover,
                    period: period.value(row),
                    departure: flags.value(row) & IS_DEPARTURE != 0,
                    secondary_only: flags.value(row) & SECONDARY_ONLY != 0,
                    start,
                    end,
                    speed_kt: f64::from(speed.value(row)),
                })
            })
            .collect();
        day.legs.extend(kept);
    }
    close(flight.take(), &mut day.low_ends);
    Ok(day)
}
