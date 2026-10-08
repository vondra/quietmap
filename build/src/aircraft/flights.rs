//! One aircraft-day trace to its flights (dev4 `trace_to_flight`): towers, obstacles, gliders and
//! balloons dropped, ground vehicles (by designator or emitter category) kept apart, sane points only, one flight per rotation with its own
//! identity and callsign. A rotation ends at a surface rest of 5 minutes or more; the next starts at
//! its takeoff roll (PLAN-z13 fix: dev4 started it at the first airborne sample, so the roll and
//! the lift-off pair stayed behind and only 14 % of jet departures knew their field).

use super::filters::point_is_sane;
use super::trace::{AircraftTrace, TracePoint, parse_address_hex};
use physics::doc29::npd::is_helicopter_class;
use physics::doc29::profiles_generated::{
    CLASS_REP_PROFILE_IDX, FALLBACK_PROFILE_IDX, IS_JET, NUM_CLASSES, is_negligible_noise_typecode,
    noise_class_of, profile_idx,
};
use std::ops::Range;

/// A surface rest this long is a turnaround (gate-to-gate is 20 min or more; runway holds,
/// touch-and-go and line-up are far shorter).
pub const MIN_TURNAROUND_S: f64 = 5.0 * 60.0;
/// A takeoff roll starts at taxi speed or below (line-up, or a rolling start from the taxiway).
pub const TAXI_MAX_SPEED_KT: f32 = 30.0;
/// Provider ids of the segment `source_id` column.
pub const ADSB_LOL: u8 = 0;
pub const ADSB_EXCHANGE: u8 = 2;
/// Profile of a ground vehicle, which has none.
pub const NO_PROFILE: u8 = u8::MAX;
const SYNTHETIC_BIT: u64 = 1 << 32;

/// The median airborne ground speed under which a flight of unknown type is a light aircraft
/// (kt): most such flights (empty designators, homebuilts' codes) are light aircraft over the
/// countryside, 10-20 dB under the fallback jet; training singles fly 80-130 kt, while a jet in
/// the air flies 140 kt only on short final and faster everywhere else. A median, as ADS-B speeds
/// jump (a light single's day read 95 kt with one sample at 400).
pub const LIGHT_MEDIAN_SPEED_KT: f32 = 140.0;

/// Whether a callsign is an airline or military flight number: three letters, then a digit.
pub fn flight_number(callsign: &str) -> bool {
    let bytes = callsign.as_bytes();
    bytes.len() >= 4 && bytes[..3].iter().all(u8::is_ascii_alphabetic) && bytes[3].is_ascii_digit()
}

/// The profile a rotation flies, decided once for all its segments: its designator's; for a
/// designator on the fallback, the transponder's emitter category (A7 rotorcraft the helicopter
/// class, A1 light and B4 ultralight the C172's) or, without a flight number at a median airborne
/// ground speed under [`LIGHT_MEDIAN_SPEED_KT`], the C172's. `None` for a glider or a balloon (B1,
/// B2), dropped as their designators are. The boxes, the events and the checker fly it.
pub fn rotation_profile(
    designator: &str,
    emitter_category: u8,
    callsign: &str,
    points: &[TracePoint],
) -> Option<u8> {
    let profile = profile_idx(designator);
    if profile != FALLBACK_PROFILE_IDX {
        return Some(profile);
    }
    let light = profile_idx("C172");
    match emitter_category {
        0xA7 => {
            let class = (0..NUM_CLASSES).find(|&class| is_helicopter_class(class));
            return class.map(|class| CLASS_REP_PROFILE_IDX[class]);
        }
        0xA1 | 0xB4 => return Some(light),
        0xB1 | 0xB2 => return None,
        _ => {}
    }
    let mut speeds: Vec<f32> = points
        .iter()
        .filter(|point| !point.is_surface_report() && point.ground_speed_kt.is_finite())
        .map(|point| point.ground_speed_kt)
        .collect();
    speeds.sort_by(f32::total_cmp);
    let slow = speeds
        .get(speeds.len() / 2)
        .is_some_and(|median| *median < LIGHT_MEDIAN_SPEED_KT);
    Some(if slow && !flight_number(callsign) {
        light
    } else {
        profile
    })
}

/// What Stage 1's filters tell apart, from the kernel's noise class of the designator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Airframe {
    Jet,
    Propeller,
    Helicopter,
}

pub fn airframe(profile: u8) -> Airframe {
    let class = usize::from(noise_class_of(profile));
    if is_helicopter_class(class) {
        Airframe::Helicopter
    } else if IS_JET[class] {
        Airframe::Jet
    } else {
        Airframe::Propeller
    }
}

/// One rotation of one aircraft.
#[derive(Clone, Debug)]
pub struct Flight {
    /// Real: ICAO address << 40 | start time; synthetic (no real address): bit 32 set.
    pub flight_id: u64,
    pub address: String,
    /// The first callsign announced within the rotation; empty when none was.
    pub callsign: String,
    pub aircraft_type: String,
    pub profile: u8,
    pub airframe: Airframe,
    pub source_id: u8,
    /// 0 aircraft, 1 ground vehicle (designator `GND`).
    pub vehicle_kind: u8,
    /// Ground vehicle class by callsign (0 light, 1 medium, 2 heavy); 0 for aircraft.
    pub ground_vehicle_class: u8,
    pub emitter_category: u8,
    pub points: Vec<TracePoint>,
}

/// The flights of one merged trace; `secondary_source` tags a rotation made only of secondary
/// samples.
pub fn trace_to_flights(
    mut trace: AircraftTrace,
    primary_source: u8,
    secondary_source: u8,
) -> Vec<Flight> {
    let designator = trace.aircraft_type.trim().to_string();
    // Obstacles (emitter categories C3-C5) make no noise; surface vehicles (C1 emergency, C2
    // service) are ground vehicles whatever their designator.
    if designator.eq_ignore_ascii_case("TWR")
        || is_negligible_noise_typecode(&designator)
        || matches!(trace.emitter_category, 0xC3..=0xC5)
    {
        return Vec::new();
    }
    let ground_vehicle =
        designator.eq_ignore_ascii_case("GND") || matches!(trace.emitter_category, 0xC1 | 0xC2);
    trace.retain_points(|_, point| point_is_sane(point));
    if trace.points.len() < 2 {
        return Vec::new();
    }
    let address = parse_address_hex(&trace.address).unwrap_or(0);
    let real_address = address != 0 && address != 0xff_ffff;
    let mut flights = Vec::new();
    for range in split_rotations(&trace.points) {
        let points = trace.points[range.clone()].to_vec();
        let first_timestamp = points[0].timestamp as u32;
        let flight_id = if real_address && first_timestamp != 0 {
            (u64::from(address) << 40) | u64::from(first_timestamp)
        } else {
            synthetic_id(&trace.aircraft_type, first_timestamp, &points[0])
        };
        let callsign = trace
            .callsigns
            .iter()
            .find(|change| range.contains(&change.point_index))
            .map(|change| change.callsign.clone())
            .unwrap_or_default();
        let profile = if ground_vehicle {
            NO_PROFILE
        } else {
            let profile = rotation_profile(&designator, trace.emitter_category, &callsign, &points);
            let Some(profile) = profile else {
                continue;
            };
            profile
        };
        let source_id = if points.iter().all(TracePoint::is_secondary) {
            secondary_source
        } else {
            primary_source
        };
        flights.push(Flight {
            flight_id,
            address: trace.address.clone(),
            ground_vehicle_class: if ground_vehicle {
                ground_vehicle_class(&callsign)
            } else {
                0
            },
            callsign,
            aircraft_type: trace.aircraft_type.clone(),
            profile,
            airframe: airframe(profile),
            source_id,
            vehicle_kind: u8::from(ground_vehicle),
            emitter_category: trace.emitter_category,
            points,
        });
    }
    flights
}

/// Index ranges of an aircraft-day's rotations, each at least 2 samples. A surface rest of
/// [`MIN_TURNAROUND_S`] after airborne samples ends a rotation; the next begins at the takeoff
/// roll of that rest, so it keeps its roll and the lift-off pair. Airborne gaps of any length
/// keep the identity (a transoceanic flight with a 4 h hole stays one flight).
pub fn split_rotations(points: &[TracePoint]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let (mut start, mut surface_run, mut has_airborne) = (0, None, false);
    for index in 0..points.len() {
        if points[index].is_surface_report() {
            surface_run.get_or_insert(index);
            continue;
        }
        if let Some(run) = surface_run.take()
            && has_airborne
            && points[index - 1].timestamp - points[run].timestamp >= MIN_TURNAROUND_S
        {
            let roll = takeoff_roll_start(
                index,
                |i| points[i].is_surface_report(),
                |i| points[i].ground_speed_kt,
            );
            ranges.push(start..roll);
            start = roll;
        }
        has_airborne = true;
    }
    ranges.push(start..points.len());
    ranges.retain(|range| range.len() >= 2);
    ranges
}

/// The first sample of the takeoff roll ending at `lift_off`: within the ground run before it, the
/// last sample at taxi speed, or the run's first when the whole run is faster.
pub fn takeoff_roll_start(
    lift_off: usize,
    on_ground: impl Fn(usize) -> bool,
    speed_kt: impl Fn(usize) -> f32,
) -> usize {
    let mut run = lift_off;
    while run > 0 && on_ground(run - 1) {
        run -= 1;
    }
    (run..lift_off)
        .rev()
        .find(|&i| speed_kt(i) <= TAXI_MAX_SPEED_KT)
        .unwrap_or(run)
}

/// A deterministic id for traffic without a real address, from type, first time and position.
fn synthetic_id(designator: &str, first_timestamp: u32, first: &TracePoint) -> u64 {
    const MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut seed = u64::from(first_timestamp);
    for byte in designator.bytes() {
        seed = seed.wrapping_mul(MULTIPLIER).wrapping_add(u64::from(byte));
    }
    seed = seed
        .wrapping_mul(MULTIPLIER)
        .wrapping_add(u64::from(first.lat.to_bits()));
    seed = seed
        .wrapping_mul(MULTIPLIER)
        .wrapping_add(u64::from(first.lon.to_bits()));
    SYNTHETIC_BIT | (((seed >> 32) & 0x7fff_ffff) << 33) | (seed & 0xffff_ffff)
}

/// dev4's Prague ground-fleet callsign prefixes (`emission/gse.rs`); others are medium.
fn ground_vehicle_class(callsign: &str) -> u8 {
    const CLASSES: [(&str, u8); 7] = [
        ("POZAR", 2),
        ("PLET", 2),
        ("FOLLOW", 0),
        ("UDRZBA", 1),
        ("METEO", 0),
        ("PTACNIK", 0),
        ("EMIL", 0),
    ];
    let callsign = callsign.trim().to_ascii_uppercase();
    CLASSES
        .iter()
        .find(|(prefix, _)| callsign.starts_with(prefix))
        .map_or(1, |&(_, class)| class)
}

#[cfg(test)]
#[path = "flights_tests.rs"]
mod tests;
