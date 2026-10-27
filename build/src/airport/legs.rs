//! The ground legs of one day's Stage 1 file (`segments/<day>.arrow`): the phase-0 segments of
//! aircraft and airport vehicles with the columns ground operations need (the boxes' reader keeps
//! the airborne ones). Only these columns are decoded.

use super::low_ends::{Airborne, FlightTrack, LowEnd};
use crate::aircraft::{IS_DEPARTURE, SECONDARY_ONLY};
use crate::stage1::DayFile;
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt8Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use physics::doc29::profiles_generated::noise_class_of;
use physics::emission::airport::GroundVehicle;
use rayon::prelude::*;
use std::collections::HashSet;
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
    /// Which of the flight's stretches on the ground: one more after every time it was airborne,
    /// so each touch-and-go of a circuit rolls again (Stage 1 keeps them in one flight).
    pub episode: u16,
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

/// A day's ground legs, the low ends of its flights, and which of their stretches on the ground
/// an airborne segment followed (a take-off ended them; a stretch after an airborne one began
/// with a landing).
#[derive(Debug, Default)]
pub struct DayLegs {
    pub legs: Vec<GroundLeg>,
    pub low_ends: Vec<LowEnd>,
    pub lift_offs: HashSet<(u64, u16)>,
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
    let file = DayFile::open(path, &COLUMNS)?
        .ok_or_else(|| format!("{}: no such day file", path.display()))?;
    // Batches decoded at once a window of them at a time, then read in order: a flight's
    // segments run across batches.
    let window = 2 * rayon::current_num_threads();
    let indices: Vec<usize> = (0..file.batches()).collect();
    let mut day = DayLegs::default();
    let mut track: Option<FlightTrack> = None;
    let batches = indices.chunks(window).flat_map(|chunk| {
        let decoded: Vec<Result<RecordBatch, String>> =
            chunk.par_iter().map(|&index| file.batch(index)).collect();
        decoded
    });
    for batch in batches {
        let batch = batch?;
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
        let mut episodes = vec![0u16; batch.num_rows()];
        for (row, row_episode) in episodes.iter_mut().enumerate() {
            if kind.value(row) != KIND_AIRCRAFT {
                continue;
            }
            let id = flight_id.value(row);
            let flight = match track.as_mut() {
                Some(flight) if flight.flight_id() == id => flight,
                _ => {
                    if let Some(done) = track.take() {
                        done.close(&mut day.low_ends);
                    }
                    track.insert(FlightTrack::new(id))
                }
            };
            if phase.value(row) == PHASE_GROUND {
                *row_episode = flight.ground(&mut day.lift_offs, &mut day.low_ends);
                continue;
            }
            let segment = Airborne {
                start: [start_lat.value(row), start_lon.value(row)],
                end: [end_lat.value(row), end_lon.value(row)],
                height_m: height.value(row),
                departure: flags.value(row) & IS_DEPARTURE != 0,
                secondary_only: flags.value(row) & SECONDARY_ONLY != 0,
                class: noise_class_of(profile.value(row)),
                period: period.value(row),
            };
            flight.airborne(segment, &mut day.lift_offs, &mut day.low_ends);
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
                    episode: episodes[row],
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
    if let Some(done) = track {
        done.close(&mut day.low_ends);
    }
    day.low_ends.retain(|end| keep(end.start, end.end));
    Ok(day)
}
