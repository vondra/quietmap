//! One aircraft-day trace to its flights (dev4 `trace_to_flight`): towers, obstacles, gliders and
//! balloons dropped, ground vehicles (by designator or emitter category) kept apart, sane points
//! only, one flight per rotation with its own identity and callsign. A rotation ends at a surface
//! rest of 5 minutes or more; the next starts at its takeoff roll (dev4 started it at the first
//! airborne sample, so the roll and the lift-off pair stayed behind and only 14 % of jet departures
//! knew their field).

use super::filters::point_is_sane;
use super::trace::{AircraftTrace, TracePoint, is_concrete_category, parse_address_hex};
use physics::doc29::npd::is_helicopter_class;
use physics::doc29::profiles_generated::{
    CLASS_REP_PROFILE_IDX, FALLBACK_PROFILE_IDX, IS_JET, NUM_CLASSES, doc8643_type,
    is_ground_vehicle_typecode, is_negligible_noise_typecode, is_tower_typecode, noise_class_of,
    profile_idx,
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

/// Whether more than half of a rotation's observed airborne speeds are below
/// [`LIGHT_MEDIAN_SPEED_KT`] (ignoring zero values from unreported ground-speed fields).
pub fn is_slow_airborne(points: &[TracePoint]) -> bool {
    let (mut total, mut under) = (0usize, 0usize);
    for point in points {
        if !point.is_surface_report()
            && point.ground_speed_kt.is_finite()
            && point.ground_speed_kt > 0.0
        {
            total += 1;
            if point.ground_speed_kt < LIGHT_MEDIAN_SPEED_KT {
                under += 1;
            }
        }
    }
    2 * under > total
}

/// The profile of a rotation whose designator is unresolved or contradicted by the aircraft's own
/// broadcasts: decisive ADS-B emitter categories first (`A7` helicopter, `A1`/`B4` C172,
/// `B1`/`B2` glider/balloon dropped, `A2` C56X, `A3` B738, `A4` B752, `A5` B763), else without a
/// flight number at a median airborne speed under [`LIGHT_MEDIAN_SPEED_KT`] the C172.
fn unknown_type_profile(emitter_category: u8, callsign: &str, slow: bool) -> Option<u8> {
    let light = profile_idx("C172");
    match emitter_category {
        0xA7 => {
            let class = (0..NUM_CLASSES).find(|&class| is_helicopter_class(class));
            return class.map(|class| CLASS_REP_PROFILE_IDX[class]);
        }
        0xA1 | 0xB4 => return Some(light),
        0xB1 | 0xB2 => return None,
        0xA2 => return Some(profile_idx("C56X")),
        0xA3 => return Some(profile_idx("B738")),
        0xA4 => return Some(profile_idx("B752")),
        0xA5 => return Some(profile_idx("B763")),
        _ => {}
    }
    Some(if slow && !flight_number(callsign) {
        light
    } else {
        FALLBACK_PROFILE_IDX
    })
}

/// The profile a rotation flies, decided once for all its segments: its designator's; for a
/// designator on the fallback, the transponder's emitter category (A7 rotorcraft the helicopter
/// class, A1 light and B4 ultralight the C172's, A2-A5 their jet class anchors) or, without a
/// flight number at a median airborne ground speed under [`LIGHT_MEDIAN_SPEED_KT`], the C172's.
/// `None` for a glider or a balloon (B1, B2), dropped as their designators are.
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
    unknown_type_profile(emitter_category, callsign, is_slow_airborne(points))
}

/// Whether `text` is a valid FAA US civil registration (`N` followed by 1-5 characters: first
/// digit 1-9, up to 4 more digits, and at most 2 trailing uppercase letters excluding `I` and `O`).
pub fn is_us_tail_number(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('N').map(str::as_bytes) else {
        return false;
    };
    if rest.is_empty() || rest.len() > 5 || !matches!(rest[0], b'1'..=b'9') {
        return false;
    }
    let digits = rest.iter().take_while(|&&b| b.is_ascii_digit()).count();
    let letters = &rest[digits..];
    letters.len() <= 2
        && letters
            .iter()
            .all(|&b| b.is_ascii_uppercase() && b != b'I' && b != b'O')
}

/// Whether `text` is a hyphenated civil registration `PREFIX-SUFFIX` (1-2 char alphanumeric
/// prefix with at least one letter, 3-4 char alphanumeric suffix; single-letter prefixes take a
/// 4-letter alphabetic suffix).
fn split_hyphenated_registration(text: &str) -> Option<(&str, &str)> {
    let (prefix, suffix) = text.split_once('-')?;
    let pb = prefix.as_bytes();
    let sb = suffix.as_bytes();
    if !(1..=2).contains(&pb.len())
        || !(3..=4).contains(&sb.len())
        || !(4..=5).contains(&(pb.len() + sb.len()))
        || !pb.iter().all(u8::is_ascii_alphanumeric)
        || !pb.iter().any(u8::is_ascii_uppercase)
        || !sb.iter().all(u8::is_ascii_alphanumeric)
    {
        return None;
    }
    if pb.len() == 1 && (sb.len() != 4 || !sb.iter().all(u8::is_ascii_uppercase)) {
        return None;
    }
    Some((prefix, suffix))
}

/// Whether the aircraft's broadcast callsign is a civil registration that contradicts the static
/// database registration `"r"`.
pub fn registration_contradicts(db_registration: &str, active_callsign: &str) -> bool {
    let db_reg: String = db_registration
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let raw_cs: String = active_callsign
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let cs: String = raw_cs.chars().filter(|&c| c != '-').collect();
    let reg_clean: String = db_reg.chars().filter(|&c| c != '-').collect();
    if reg_clean.is_empty()
        || cs.is_empty()
        || flight_number(&cs)
        || cs == reg_clean
        || (cs.len() >= 3 && reg_clean.ends_with(&cs))
    {
        return false;
    }
    if let Some((prefix, _)) = split_hyphenated_registration(&db_reg) {
        let same_country_reg = cs.starts_with(prefix)
            && cs.len() == reg_clean.len()
            && cs[prefix.len()..].bytes().all(|b| b.is_ascii_uppercase());
        return same_country_reg
            || split_hyphenated_registration(&raw_cs).is_some()
            || is_us_tail_number(&cs);
    }
    if is_us_tail_number(&reg_clean) {
        return is_us_tail_number(&cs) || split_hyphenated_registration(&raw_cs).is_some();
    }
    false
}

/// Whether the aircraft's broadcast ADS-B emitter category contradicts the static database
/// designator's ICAO Doc 8643 class or wake category, corroborated by the rotation's speed.
pub fn category_contradicts_type(
    designator: &str,
    emitter_category: u8,
    slow: bool,
    is_flight_number: bool,
) -> bool {
    let code = designator.trim();
    if code.is_empty() {
        return false;
    }
    if is_negligible_noise_typecode(code)
        || is_tower_typecode(code)
        || is_ground_vehicle_typecode(code)
    {
        return match emitter_category {
            0xA7 => slow,
            0xA1 | 0xB4 => slow && !is_flight_number && !code.eq_ignore_ascii_case("BALL"),
            0xA2..=0xA5 => !slow || is_flight_number,
            _ => false,
        };
    }
    let profile = profile_idx(code);
    let is_light_bizjet = profile == profile_idx("C56X") || profile == profile_idx("LJ60");
    let Some((desc, wtc)) = doc8643_type(code) else {
        if profile == FALLBACK_PROFILE_IDX {
            return false;
        }
        let class = usize::from(noise_class_of(profile));
        return match emitter_category {
            0xB1 | 0xB2 => slow && !is_flight_number,
            0xA7 => slow && !is_helicopter_class(class),
            0xA1 => slow && !is_flight_number && IS_JET[class] && !is_light_bizjet,
            0xB4 => slow && !is_flight_number && IS_JET[class],
            0xA3..=0xA5 => (!slow || is_flight_number) && is_helicopter_class(class),
            _ => false,
        };
    };
    let is_rotorcraft = matches!(desc[0], b'H' | b'G' | b'R');
    let is_fixed_wing = matches!(desc[0], b'L' | b'A' | b'S');
    let is_motor_glider_or_airship = desc == *b"L1M"
        || desc == *b"B0-"
        || matches!(code, "SF25" | "DIMO" | "G109" | "AS16" | "SF28");
    match emitter_category {
        0xB1 | 0xB2 => slow && !is_flight_number && !is_motor_glider_or_airship,
        0xA7 => slow && is_fixed_wing,
        0xA1 => {
            slow && !is_flight_number
                && !is_light_bizjet
                && ((is_fixed_wing && desc[2] == b'J' && matches!(wtc, b'M' | b'H' | b'J'))
                    || matches!(wtc, b'H' | b'J'))
        }
        0xB4 => {
            slow && !is_flight_number
                && ((is_fixed_wing && desc[2] == b'J') || matches!(wtc, b'M' | b'H' | b'J'))
        }
        0xA3..=0xA5 => {
            (!slow || is_flight_number)
                && matches!(wtc, b'L' | b'-')
                && (is_rotorcraft
                    || matches!(desc[2], b'P' | b'E' | b'M')
                    || matches!(desc, [b'L' | b'B' | b'P' | b'D', b'0', b'-']))
        }
        _ => false,
    }
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
    // Obstacles (emitter categories C3-C5) make no noise.
    if matches!(trace.emitter_category, 0xC3..=0xC5) {
        return Vec::new();
    }
    trace.retain_points(|_, point| point_is_sane(point));
    if trace.points.len() < 2 {
        return Vec::new();
    }
    let address = parse_address_hex(&trace.address).unwrap_or(0);
    let real_address = address != 0 && address != 0xff_ffff;
    let mut flights = Vec::new();
    for range in split_rotations(&trace.points) {
        let points = trace.points[range.clone()].to_vec();
        let callsign = trace
            .callsigns
            .iter()
            .find(|change| range.contains(&change.point_index))
            .map(|change| change.callsign.clone())
            .unwrap_or_default();
        let active_callsign = if !callsign.is_empty() {
            callsign.as_str()
        } else {
            trace
                .callsigns
                .iter()
                .rev()
                .find(|change| change.point_index < range.start)
                .map_or("", |change| change.callsign.as_str())
        };
        let slow = is_slow_airborne(&points);
        let reg_contradicted = registration_contradicts(&trace.registration, active_callsign);
        let cat_contradicted = category_contradicts_type(
            &designator,
            trace.emitter_category,
            slow,
            flight_number(active_callsign),
        );
        let header_contradicted = reg_contradicted || cat_contradicted;
        if !header_contradicted
            && (is_tower_typecode(&designator) || is_negligible_noise_typecode(&designator))
        {
            continue;
        }
        let ground_vehicle = matches!(trace.emitter_category, 0xC1 | 0xC2)
            || (!header_contradicted && is_ground_vehicle_typecode(&designator));
        let aircraft_type = if header_contradicted {
            String::new()
        } else {
            trace.aircraft_type.clone()
        };
        let first_timestamp = points[0].timestamp as u32;
        let flight_id = if real_address && first_timestamp != 0 {
            (u64::from(address) << 40) | u64::from(first_timestamp)
        } else {
            synthetic_id(&aircraft_type, first_timestamp, &points[0])
        };
        let profile = if ground_vehicle {
            NO_PROFILE
        } else if !header_contradicted {
            let Some(profile) =
                rotation_profile(&designator, trace.emitter_category, &callsign, &points)
            else {
                continue;
            };
            profile
        } else if reg_contradicted
            && !cat_contradicted
            && slow
            && !flight_number(active_callsign)
            && !matches!(trace.emitter_category, 0xA7 | 0xB1 | 0xB2)
        {
            profile_idx("C172")
        } else if reg_contradicted
            && !cat_contradicted
            && !is_concrete_category(trace.emitter_category)
            && !slow
        {
            profile_idx(&designator)
        } else {
            let Some(profile) = unknown_type_profile(trace.emitter_category, active_callsign, slow)
            else {
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
            aircraft_type,
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
