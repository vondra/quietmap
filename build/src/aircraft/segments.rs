//! One segment per joinable pair of consecutive samples (dev4 `segment/mod.rs`): phase, gap budget,
//! keepable geometry, departure or approach, helicopter descent, period and the departure field.

use super::altitude::{M_PER_FT, Sample};
use super::filters::segment_is_keepable;
use super::flat::{flat_distance_m, interpolate};
use super::flights::{Airframe, takeoff_roll_start};
use super::phases::Phase;
use crate::period::period;

/// Gap budgets by the more permissive endpoint phase: oceanic cruise dropouts of up to an hour
/// are real, terminal areas have dense coverage, ground data is continuous.
const GAP_S_CRUISE: f64 = 3600.0;
const GAP_S_AIRBORNE: f64 = 120.0;
const GAP_S_GROUND: f64 = 60.0;
/// An airborne endpoint may lie this far below the terrain (DEM error near runways).
const AIRBORNE_MIN_HEIGHT_M: f32 = -30.0;
/// Half window (samples) of the median-smoothed climb rate: 30-90 s at 5-15 s cadence.
const RATE_SMOOTHING_HALF_WINDOW: usize = 5;
/// Doc 29 A.3.2: a climb faster than 500 ft/min departs; above 10,000 ft a descent shallower
/// than 500 ft/min still flies departure (cruise) thrust.
const DEPARTURE_CLIMB_FPM: f32 = 500.0;
const CRUISE_THRUST_ABOVE_FT: f32 = 10_000.0;
/// A takeoff roll accelerates at 180-250 kt/min; 60 kt/min separates it from taxi and rollouts.
const GROUND_DEPARTURE_ACCELERATION_KT_PER_MIN: f32 = 60.0;
/// A helicopter descending faster than this carries the descent (blade-vortex) correction: about
/// a 3 degree path at 60 kt; shallower descents keep the level curve (their BVI is small).
pub const HELICOPTER_DESCENT_FPM: f32 = -300.0;
/// The helicopter descent rate spans the samples within this time of a pair's middle, at least
/// 10 s of them: 25 ft altitude steps then quantize the rate to 150 ft/min or finer.
const DESCENT_WINDOW_HALF_S: f64 = 15.0;
const DESCENT_WINDOW_MIN_SPAN_S: f64 = 10.0;

/// Segment flags (the v16 `flags` column).
pub const IS_DEPARTURE: u8 = 1 << 0;
pub const ON_GROUND: u8 = 1 << 1;
pub const SECONDARY_ONLY: u8 = 1 << 6;
pub const HELICOPTER_DESCENT: u8 = 1 << 7;

/// One segment; per-flight fields live in the flight.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub period: u8,
    pub phase: Phase,
    pub flags: u8,
    pub start_lat: f32,
    pub start_lon: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    /// Pressure altitude, metres; a ground endpoint takes the airborne one's, both ground 0.
    pub start_barometric_m: f32,
    pub end_barometric_m: f32,
    /// Altitude above EGM2008 ([`super::altitude`]); a ground endpoint is the terrain.
    pub start_altitude_m: f32,
    pub end_altitude_m: f32,
    pub speed_kt: f32,
    pub length_m: f32,
    pub mean_height_m: f32,
    pub start_terrain_m: f32,
    pub end_terrain_m: f32,
}

/// Airborne wins a mixed pair (take-off, flare); a direct cruise-ground pair has no observed climb
/// or descent and joins nothing.
fn segment_phase(a: Phase, b: Phase) -> Option<Phase> {
    match (a, b) {
        (Phase::Cruise, Phase::Ground) | (Phase::Ground, Phase::Cruise) => None,
        (Phase::Airborne, _) | (_, Phase::Airborne) => Some(Phase::Airborne),
        _ => Some(b),
    }
}

fn gap_budget_s(a: Phase, b: Phase) -> f64 {
    match a.max(b) {
        Phase::Cruise => GAP_S_CRUISE,
        Phase::Airborne => GAP_S_AIRBORNE,
        Phase::Ground => GAP_S_GROUND,
    }
}

/// The whole join decision for the pair (a, b): its phase when it makes a segment.
pub fn pair_phase(
    samples: &[Sample],
    phases: &[Phase],
    airframe: Airframe,
    a: usize,
    b: usize,
) -> Option<Phase> {
    let (first, second) = (&samples[a], &samples[b]);
    let dt = second.point.timestamp - first.point.timestamp;
    if dt <= 0.0 {
        return None;
    }
    let phase = segment_phase(phases[a], phases[b])?;
    if dt > gap_budget_s(phases[a], phases[b]) {
        return None;
    }
    let airborne = phase != Phase::Ground;
    let length = flat_distance_m(
        first.point.lat,
        first.point.lon,
        second.point.lat,
        second.point.lon,
    );
    let speed = (first.point.ground_speed_kt + second.point.ground_speed_kt) * 0.5;
    if !segment_is_keepable(
        length,
        dt as f32,
        first.height_m,
        second.height_m,
        speed,
        airframe,
        airborne,
    ) {
        return None;
    }
    // NaN heights fail `>=`.
    let above = |s: &Sample| s.height_m >= AIRBORNE_MIN_HEIGHT_M;
    (!airborne || (above(first) && above(second))).then_some(phase)
}

/// Drop secondary samples inside a primary pair that joins: they add no coverage and would only
/// move the stretch onto the noisier increment estimator. A secondary sample survives in a real
/// primary gap or outside the primary trace.
pub fn suppress_covered_secondary(
    samples: &mut Vec<Sample>,
    phases: &mut Vec<Phase>,
    airframe: Airframe,
) {
    let primary: Vec<usize> = (0..samples.len())
        .filter(|&i| !samples[i].point.is_secondary())
        .collect();
    let spans: Vec<(f64, f64)> = primary
        .windows(2)
        .filter(|pair| pair_phase(samples, phases, airframe, pair[0], pair[1]).is_some())
        .map(|pair| {
            (
                samples[pair[0]].point.timestamp,
                samples[pair[1]].point.timestamp,
            )
        })
        .collect();
    let covered = |sample: &Sample| {
        sample.point.is_secondary()
            && spans.iter().any(|&(start, end)| {
                start <= sample.point.timestamp && sample.point.timestamp <= end
            })
    };
    let keep: Vec<bool> = samples.iter().map(|s| !covered(s)).collect();
    let mut flags = keep.iter();
    phases.retain(|_| *flags.next().unwrap_or(&true));
    let mut flags = keep.iter();
    samples.retain(|_| *flags.next().unwrap_or(&true));
}

/// Per pair (index i is the pair i-1, i): departure or not. Airborne pairs climb by the secant over
/// +-15 s (`windowed_climb_fpm`; dev4's median of per-step rates over +-5 samples where the window
/// spans too little): at 1 s cadence 25 ft steps make most per-step rates zero, and the median then
/// found 47 % of the climbs faster than 500 ft/min against 84 % for the window (Doc 29 A.3.2 with
/// the FL100 cruise rule). Ground pairs depart when the median acceleration exceeds 60 kt/min (a
/// takeoff roll, not taxi or a rollout).
pub fn departures(samples: &[Sample], phases: &[Phase]) -> Vec<bool> {
    let n = samples.len();
    let (mut rate_step, mut acceleration_step) = (vec![f32::NAN; n], vec![f32::NAN; n]);
    for i in 1..n {
        let (a, b) = (&samples[i - 1].point, &samples[i].point);
        let dt_min = (b.timestamp - a.timestamp) as f32 / 60.0;
        if dt_min <= 0.0 {
            continue;
        }
        if let (Some(x), Some(y)) = (a.airborne_altitude_ft(), b.airborne_altitude_ft()) {
            rate_step[i] = (y - x) / dt_min;
        }
        acceleration_step[i] = (b.ground_speed_kt - a.ground_speed_kt) / dt_min;
    }
    let mut departure = vec![false; n];
    let mut window = Vec::with_capacity(2 * RATE_SMOOTHING_HALF_WINDOW + 1);
    let mut smoothed = |steps: &[f32]| {
        window.clear();
        window.extend(steps.iter().copied().filter(|v| v.is_finite()));
        (!window.is_empty()).then(|| {
            let middle = window.len() / 2;
            *window
                .select_nth_unstable_by(middle, |a, b| a.total_cmp(b))
                .1
        })
    };
    for i in 1..n {
        let (low, high) = (
            i.saturating_sub(RATE_SMOOTHING_HALF_WINDOW),
            (i + RATE_SMOOTHING_HALF_WINDOW + 1).min(n),
        );
        if phases[i - 1] == Phase::Ground || phases[i] == Phase::Ground {
            if let Some(acceleration) = smoothed(&acceleration_step[low..high]) {
                departure[i] = acceleration > GROUND_DEPARTURE_ACCELERATION_KT_PER_MIN;
            }
        } else if let Some(climb_fpm) = Some(windowed_climb_fpm(samples, i))
            .filter(|rate| rate.is_finite())
            .or_else(|| smoothed(&rate_step[low..high]))
        {
            let mean_ft = match (
                samples[i - 1].point.airborne_altitude_ft(),
                samples[i].point.airborne_altitude_ft(),
            ) {
                (Some(a), Some(b)) => (a + b) * 0.5,
                _ => 0.0,
            };
            departure[i] = climb_fpm > DEPARTURE_CLIMB_FPM
                || (mean_ft > CRUISE_THRUST_ABOVE_FT && climb_fpm > -DEPARTURE_CLIMB_FPM);
        }
    }
    departure
}

/// The climb rate (ft/min) around the pair (i-1, i): the barometric secant between the first and
/// last airborne samples within 15 s of the pair's middle; NaN when they span less than 10 s. PLAN-z13
/// fix: dev4 flagged a helicopter pair losing 10 m, which finds 4 % of the descents at 1 s cadence
/// and 82 % at 10 s; the median of per-step rates still finds only 11 % at 1 s (25 ft steps make
/// most 1 s steps zero); this window finds 82-87 % at every cadence up to 12 s (one day, 3,886
/// helicopters, against a +-60 s least-squares rate).
pub fn windowed_climb_fpm(samples: &[Sample], i: usize) -> f32 {
    let middle = (samples[i - 1].point.timestamp + samples[i].point.timestamp) * 0.5;
    let within = |s: &Sample| (s.point.timestamp - middle).abs() <= DESCENT_WINDOW_HALF_S;
    let airborne = |s: &&Sample| s.point.airborne_altitude_ft().is_some();
    let mut first = i - 1;
    while first > 0 && within(&samples[first - 1]) {
        first -= 1;
    }
    let mut last = i;
    while last + 1 < samples.len() && within(&samples[last + 1]) {
        last += 1;
    }
    let window = &samples[first..=last];
    match (
        window.iter().find(airborne),
        window.iter().rev().find(airborne),
    ) {
        (Some(a), Some(b))
            if b.point.timestamp - a.point.timestamp >= DESCENT_WINDOW_MIN_SPAN_S =>
        {
            (b.point.altitude_ft - a.point.altitude_ft)
                / ((b.point.timestamp - a.point.timestamp) / 60.0) as f32
        }
        _ => f32::NAN,
    }
}

/// The terrain under the takeoff roll of the flight's first lift-off; NaN when the flight was
/// first seen airborne (an overflight, or no coverage on the aerodrome).
pub fn departure_field_m(samples: &[Sample], on_ground: &[bool]) -> f32 {
    match on_ground.iter().position(|&g| !g) {
        Some(lift_off) if lift_off > 0 => {
            let roll = takeoff_roll_start(
                lift_off,
                |i| on_ground[i],
                |i| samples[i].point.ground_speed_kt,
            );
            samples[roll].terrain_m
        }
        _ => f32::NAN,
    }
}

/// The segments of one flight.
pub fn build_segments(samples: &[Sample], phases: &[Phase], airframe: Airframe) -> Vec<Segment> {
    let departure = departures(samples, phases);
    let mut segments = Vec::with_capacity(samples.len().saturating_sub(1));
    for i in 1..samples.len() {
        let Some(phase) = pair_phase(samples, phases, airframe, i - 1, i) else {
            continue;
        };
        let (a, b) = (&samples[i - 1], &samples[i]);
        let (mid_lat, mid_lon) =
            interpolate(a.point.lat, a.point.lon, b.point.lat, b.point.lon, 0.5);
        let mid_time = (a.point.timestamp + b.point.timestamp) * 0.5;
        let barometric = |x: &Sample, y: &Sample| {
            x.point
                .airborne_altitude_ft()
                .or(y.point.airborne_altitude_ft())
                .unwrap_or(0.0)
                * M_PER_FT
        };
        let mut flags = 0;
        if departure[i] {
            flags |= IS_DEPARTURE;
        }
        if phase == Phase::Ground {
            flags |= ON_GROUND;
        }
        if a.point.is_secondary() || b.point.is_secondary() {
            flags |= SECONDARY_ONLY;
        }
        if airframe == Airframe::Helicopter
            && windowed_climb_fpm(samples, i) < HELICOPTER_DESCENT_FPM
        {
            flags |= HELICOPTER_DESCENT;
        }
        segments.push(Segment {
            period: period(mid_time, f64::from(mid_lat), f64::from(mid_lon)),
            phase,
            flags,
            start_lat: a.point.lat,
            start_lon: a.point.lon,
            end_lat: b.point.lat,
            end_lon: b.point.lon,
            start_barometric_m: barometric(a, b),
            end_barometric_m: barometric(b, a),
            start_altitude_m: a.altitude_m,
            end_altitude_m: b.altitude_m,
            speed_kt: (a.point.ground_speed_kt + b.point.ground_speed_kt) * 0.5,
            length_m: flat_distance_m(a.point.lat, a.point.lon, b.point.lat, b.point.lon),
            mean_height_m: (a.height_m + b.height_m) * 0.5,
            start_terrain_m: a.terrain_m,
            end_terrain_m: b.terrain_m,
        });
    }
    segments
}

#[cfg(test)]
#[path = "segments_tests.rs"]
mod tests;
