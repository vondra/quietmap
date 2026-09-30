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
const COLUMNS: [&str; 12] = [
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
];

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

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|column| column.as_ref())
        .ok_or_else(|| format!("segments: no column {name}"))
}

/// The ground legs of the day file at `path` whose ends (latitude, longitude) `keep` accepts. A
/// missing day is an error, never a quiet day.
pub fn read_ground_legs(
    path: &Path,
    keep: &(dyn Fn([f64; 2], [f64; 2]) -> bool + Sync),
) -> Result<Vec<GroundLeg>, String> {
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
    let mut legs = Vec::new();
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
        legs.extend(kept);
    }
    Ok(legs)
}
