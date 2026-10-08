//! One day's flight segments from Stage 1's scratch (`segments/<day>.arrow`): the columns the
//! boxes need, one struct per airborne or cruise segment (ground segments go to ground operations).

use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt8Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use rayon::prelude::*;
use std::path::Path;

pub use crate::aircraft::{
    HELICOPTER_DESCENT as FLAG_HELICOPTER_DESCENT, IS_DEPARTURE as FLAG_DEPARTURE,
    ON_GROUND as FLAG_ON_GROUND, SECONDARY_ONLY as FLAG_SECONDARY_ONLY,
};
/// Stage 1 phases: ground segments are not boxed.
const PHASE_GROUND: u8 = 0;

/// One segment as the boxes read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlightSegment {
    /// ICAO address in bits 63-40 and the flight's start (Unix seconds) in bits 31-0 (dev4).
    pub flight_id: u64,
    pub callsign: [u8; 8],
    pub designator: [u8; 4],
    /// The profile Stage 1 decided for the flight: its class (`AircraftType::of`).
    pub profile: u8,
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
    /// Terrain under both ends (m above sea level): whether the kernel screens the segment.
    pub ground_m: [f64; 2],
    /// Along-track acceleration (m/s^2) from the flight's neighbouring airborne segments.
    pub acceleration_ms2: Option<f64>,
}

/// Knots to metres per second.
const KNOT_MS: f64 = 0.514_444;
/// Accelerations beyond this are ADS-B speed glitches, not flight (m/s^2; a takeoff roll reaches
/// about 2.5).
const ACCELERATION_MAX_MS2: f64 = 3.0;

/// Each row's along-track acceleration from its flight's neighbours in the day file, which holds
/// a flight's segments in order: (v_next^2 - v_prev^2) / 2s over the path between their middles,
/// one-sided at a flight's ends or gaps; a neighbour shares the flight, touches the row and is
/// airborne (the landing roll is no deceleration of the segment above it: a 777-300 read 3.2 dB
/// low over the threshold). Unknown alone, and where the result is not finite or beyond
/// [`ACCELERATION_MAX_MS2`] (an ADS-B speed glitch, not the cap). Speeds are ground speeds (the
/// wind cancels between neighbours).
pub fn accelerations(
    flight_id: &[u64],
    ends: &[([f32; 2], [f32; 2])],
    speed_kt: &[f32],
    length_m: &[f32],
    phase: &[u8],
) -> Vec<Option<f64>> {
    let n = flight_id.len();
    let joined = |a: usize, b: usize| {
        flight_id[a] == flight_id[b]
            && ends[a].1 == ends[b].0
            && phase[a] != PHASE_GROUND
            && phase[b] != PHASE_GROUND
    };
    (0..n)
        .map(|i| {
            let previous = (i > 0 && joined(i - 1, i)).then(|| i - 1);
            let next = (i + 1 < n && joined(i, i + 1)).then_some(i + 1);
            let speed = |k: usize| f64::from(speed_kt[k]) * KNOT_MS;
            let half = |k: usize| 0.5 * f64::from(length_m[k]);
            let (from, to, path) = match (previous, next) {
                (Some(p), Some(q)) => (p, q, half(p) + 2.0 * half(i) + half(q)),
                (Some(p), None) => (p, i, half(p) + half(i)),
                (None, Some(q)) => (i, q, half(i) + half(q)),
                (None, None) => return None,
            };
            let acceleration = (speed(to).powi(2) - speed(from).powi(2)) / (2.0 * path);
            (acceleration.abs() <= ACCELERATION_MAX_MS2).then_some(acceleration)
        })
        .collect()
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

/// The airborne and cruise segments of the day file at `path` whose ends (latitude, longitude)
/// `keep` accepts (none when the file is absent). Batches are read one at a time: a world day is
/// 86 M segments, of which a region keeps a small part.
pub fn read_segments(
    path: &Path,
    keep: &(dyn Fn([f64; 2], [f64; 2]) -> bool + Sync),
) -> Result<Vec<FlightSegment>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let reader = arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut segments = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|error| format!("{}: {error}", path.display()))?;
        let batch = &batch;
        let bytes = |name| column(batch, name).map(|c| c.as_primitive::<UInt8Type>());
        let floats = |name| column(batch, name).map(|c| c.as_primitive::<Float32Type>());
        let (profile, source_id, period, phase, flags) = (
            bytes("profile_idx")?,
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
            "length_m",
        ];
        let values = names
            .iter()
            .map(|name| floats(name))
            .collect::<Result<Vec<_>, String>>()?;
        let ends: Vec<([f32; 2], [f32; 2])> = (0..batch.num_rows())
            .map(|row| {
                (
                    [values[0].value(row), values[1].value(row)],
                    [values[3].value(row), values[4].value(row)],
                )
            })
            .collect();
        let acceleration = accelerations(
            flight_id.values(),
            &ends,
            values[8].values(),
            values[13].values(),
            phase.values(),
        );
        let kept: Vec<FlightSegment> = (0..batch.num_rows())
            .into_par_iter()
            .filter(|&row| phase.value(row) != PHASE_GROUND)
            .flat_map_iter(|row| {
                let value = |index: usize| f64::from(values[index].value(row));
                split_at_antimeridian(FlightSegment {
                    flight_id: flight_id.value(row),
                    callsign: padded(callsign.value(row).as_bytes()),
                    designator: padded(designator.value(row)),
                    profile: profile.value(row),
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
                    acceleration_ms2: acceleration[row],
                })
                .into_iter()
                .filter(|segment| {
                    keep(
                        [segment.start[0], segment.start[1]],
                        [segment.end[0], segment.end[1]],
                    )
                })
            })
            .collect();
        segments.extend(kept);
    }
    Ok(segments)
}

/// Longitude (deg) a half of a segment across the antimeridian ends at, short of +-180 by more than
/// an f32 step there (1.5e-5 deg), so that its stored end stays on its own side.
const ANTIMERIDIAN_EDGE_DEG: f64 = 180.0 - 2e-5;

/// A segment whose ends lie more than 180 degrees of longitude apart crossed the antimeridian:
/// its two halves, each on its own side (a gap of 4 m), so that no square, box or receiver sees a
/// line across the whole world (Codex, review of r055: the shuffle sent such a segment to every
/// square of its rows).
fn split_at_antimeridian(segment: FlightSegment) -> Vec<FlightSegment> {
    let (west, east) = (segment.start[1], segment.end[1]);
    if (east - west).abs() <= 180.0 {
        return vec![segment];
    }
    // Eastwards from near +180 to near -180, or westwards the other way.
    let eastwards = west > 0.0;
    let unwrapped = if eastwards {
        east + 360.0
    } else {
        east - 360.0
    };
    let edge = if eastwards { 180.0 } else { -180.0 };
    let t = (edge - west) / (unwrapped - west);
    let lerp = |a: f64, b: f64| a + t * (b - a);
    let (lat, altitude) = (
        lerp(segment.start[0], segment.end[0]),
        lerp(segment.start[2], segment.end[2]),
    );
    let (pressure, ground) = (
        lerp(
            segment.pressure_altitude_m[0],
            segment.pressure_altitude_m[1],
        ),
        lerp(segment.ground_m[0], segment.ground_m[1]),
    );
    let side = ANTIMERIDIAN_EDGE_DEG.copysign(edge);
    let first = FlightSegment {
        end: [lat, side, altitude],
        pressure_altitude_m: [segment.pressure_altitude_m[0], pressure],
        ground_m: [segment.ground_m[0], ground],
        ..segment.clone()
    };
    let second = FlightSegment {
        start: [lat, -side, altitude],
        pressure_altitude_m: [pressure, segment.pressure_altitude_m[1]],
        ground_m: [ground, segment.ground_m[1]],
        ..segment
    };
    vec![first, second]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A segment from 179.99 E to 179.99 W is two halves meeting at the antimeridian, each on its
    /// own side, its middle's latitude and altitude where they meet; a segment beside it is one.
    #[test]
    fn a_segment_across_the_antimeridian_is_cut_there() {
        let segment = FlightSegment {
            start: [10.0, 179.99, 10_000.0],
            end: [10.02, -179.99, 10_100.0],
            ..FlightSegment::default()
        };
        let halves = split_at_antimeridian(segment.clone());
        assert_eq!(halves.len(), 2);
        assert_eq!(halves[0].start, segment.start);
        assert_eq!(halves[1].end, segment.end);
        assert!(
            (halves[0].end[0] - 10.01).abs() < 1e-9 && (halves[0].end[2] - 10_050.0).abs() < 1e-6
        );
        assert!(halves[0].end[1] > 179.9999 && halves[1].start[1] < -179.9999);
        assert!((halves[0].end[1] as f32) < 180.0 && (halves[1].start[1] as f32) > -180.0);
        let beside = FlightSegment {
            start: [10.0, 179.0, 10_000.0],
            end: [10.0, 179.5, 10_000.0],
            ..FlightSegment::default()
        };
        assert_eq!(split_at_antimeridian(beside.clone()), vec![beside]);
    }

    /// Three touching segments of one flight speeding from 150 to 170 kt, then another flight: the
    /// middle one reads the central difference, the ends one-sided, the lone flight none; a gap
    /// leaves both sides unknown; the landing roll is no neighbour; a glitch and a missing speed
    /// are unknown, not the cap.
    #[test]
    fn accelerations_come_from_the_flights_airborne_neighbours() {
        let ends = [
            ([0.0, 0.0], [0.0, 1.0]),
            ([0.0, 1.0], [0.0, 2.0]),
            ([0.0, 2.0], [0.0, 3.0]),
            ([5.0, 5.0], [5.0, 6.0]),
        ];
        let flight = [1, 1, 1, 2];
        let speed = [150.0, 160.0, 170.0, 200.0];
        let length = [1_000.0, 1_000.0, 1_000.0, 1_000.0];
        let airborne = [1; 4];
        let a = accelerations(&flight, &ends, &speed, &length, &airborne);
        let v = |kt: f64| kt * KNOT_MS;
        let close = |got: Option<f64>, want: f64| (got.unwrap() - want).abs() < 1e-12;
        assert!(close(a[1], (v(170.0).powi(2) - v(150.0).powi(2)) / 4_000.0));
        assert!(close(a[0], (v(160.0).powi(2) - v(150.0).powi(2)) / 2_000.0));
        assert!(close(a[2], (v(170.0).powi(2) - v(160.0).powi(2)) / 2_000.0));
        assert_eq!(a[3], None);
        let gap = [ends[0], ([0.0, 1.5], [0.0, 2.0])];
        assert_eq!(
            accelerations(&[1, 1], &gap, &speed[..2], &length[..2], &[1, 1]),
            vec![None, None]
        );
        let landed = accelerations(
            &[1, 1, 1],
            &ends[..3],
            &[145.0, 140.0, 40.0],
            &[1_000.0, 1_000.0, 600.0],
            &[1, 1, PHASE_GROUND],
        );
        assert!(close(
            landed[1],
            (v(140.0).powi(2) - v(145.0).powi(2)) / 2_000.0
        ));
        let touching = &ends[..2];
        for glitch in [[95.0, 400.0], [140.0, f32::NAN]] {
            assert_eq!(
                accelerations(&[1, 1], touching, &glitch, &[1_000.0, 1_000.0], &[1, 1]),
                vec![None, None]
            );
        }
    }
}
