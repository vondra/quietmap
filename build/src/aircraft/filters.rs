//! Receiver-independent data filters of Stage 0/1 (dev4 `filters.rs`): sane points, the bogus tail
//! of a trajectory, teleports, spikes (r051) and unkeepable segments.

use super::altitude::Sample;
use super::flat::{M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR, signed_longitude_delta};
use super::flights::Airframe;
use super::trace::TracePoint;

/// About 4 x the combined error of barometric altitude, the DEM and pressure offsets: below this
/// height the whole tail is fabricated (a receiver tracking returns into the ground).
pub const HARD_HEIGHT_FLOOR_M: f32 = -300.0;
/// Sustained descents beyond this rate are not flight; three samples (about 15 s) make it so.
const ANOMALY_DESCENT_RATE_FPM: f32 = 8_000.0;
const ANOMALY_SUSTAINED_SAMPLES: usize = 3;
/// Shorter segments are taxi remnants.
pub const MIN_SEGMENT_LENGTH_M: f32 = 10.0;
/// Below this speed a jet is not flying.
const JET_STALL_SPEED_KT: f32 = 80.0;
/// Concorde's ceiling was 60,000 ft; beyond +10,000 ft is corrupt data.
const MAX_PLAUSIBLE_ALTITUDE_FT: f32 = 70_000.0;
/// Sub-sea-level aerodromes report negative altitudes; -2,000 ft leaves margin.
const MIN_PLAUSIBLE_ALTITUDE_FT: f32 = -2_000.0;
/// About Mach 3 at FL600; beyond is bad data.
pub const MAX_PLAUSIBLE_SPEED_KT: f32 = 1_500.0;
/// Metres per second to knots, exact (1 nm = 1,852 m).
pub const MPS_TO_KT: f32 = 3600.0 / 1852.0;
/// A helicopter 5 km above the terrain is a decode error (civil ceilings reach 4-6 km altitude).
const HELICOPTER_HEIGHT_CEILING_M: f32 = 5_000.0;
/// A spike: a position further off the chord of its neighbours (all three within `SPIKE_WINDOW_S`)
/// than this and than any turn could carry it: turning at most `MAX_TURN_RATE_DEG_S` between the
/// neighbours (airliners turn at 3 deg/s, aerobatics near 15), an arc of turn theta stands at most
/// chord / 2 * tan(theta / 4) off its chord. A multilateration or decoding glitch jumping off the
/// track and back does not keep to that (one world day: 0.045 % of points off by over 500 m, a
/// third of them by over 1 km).
const SPIKE_OFFSET_M: f32 = 300.0;
const MAX_TURN_RATE_DEG_S: f32 = 15.0;
const SPIKE_WINDOW_S: f32 = 30.0;

/// Finite time and position (not the 0,0 no-fix sentinel), a plausible airborne altitude and a
/// finite, possible speed. A surface report carries no altitude to check.
pub fn point_is_sane(point: &TracePoint) -> bool {
    if !point.timestamp.is_finite() || !(0.0..=f64::from(u32::MAX)).contains(&point.timestamp) {
        return false;
    }
    if !point.lat.is_finite() || !point.lon.is_finite() {
        return false;
    }
    if point.lat.abs() > 90.0 || point.lon.abs() > 180.0 || (point.lat == 0.0 && point.lon == 0.0) {
        return false;
    }
    if let Some(altitude) = point.airborne_altitude_ft()
        && !(MIN_PLAUSIBLE_ALTITUDE_FT..=MAX_PLAUSIBLE_ALTITUDE_FT).contains(&altitude)
    {
        return false;
    }
    point.ground_speed_kt.is_finite()
        && (0.0..=MAX_PLAUSIBLE_SPEED_KT).contains(&point.ground_speed_kt)
        && point.track_deg.is_finite()
        && point.vertical_rate_fpm.is_finite()
}

/// Cut the trajectory at its first implausible point (below the hard height floor, or the start of
/// a sustained anomalous descent), walking back over the negative heights that led there, then drop
/// teleports. Cutting the whole tail keeps the fabricated approach before it out as well.
pub fn validate_trajectory(samples: &mut Vec<Sample>) {
    let underground = samples
        .iter()
        .position(|s| s.height_m < HARD_HEIGHT_FLOOR_M);
    let descent = sustained_descent_start(samples);
    let cut = match (underground, descent) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if let Some(mut keep) = cut {
        while keep > 0 && samples[keep - 1].height_m < 0.0 {
            keep -= 1;
        }
        samples.truncate(keep);
    }
    drop_teleports(samples);
    drop_spikes(samples);
}

fn sustained_descent_start(samples: &[Sample]) -> Option<usize> {
    if samples.len() < ANOMALY_SUSTAINED_SAMPLES + 1 {
        return None;
    }
    let mut run = 0;
    for (index, sample) in samples.iter().enumerate() {
        if sample.point.vertical_rate_fpm < -ANOMALY_DESCENT_RATE_FPM {
            run += 1;
            if run >= ANOMALY_SUSTAINED_SAMPLES {
                return Some(index + 1 - run);
            }
        } else {
            run = 0;
        }
    }
    None
}

/// Drop a point jumping more than 10,000 ft or faster than the plausible speed from the last kept
/// point within 10 s.
fn drop_teleports(samples: &mut Vec<Sample>) {
    if samples.len() < 2 {
        return;
    }
    let mut keep = vec![true; samples.len()];
    let mut previous = 0;
    for index in 1..samples.len() {
        let (a, b) = (&samples[previous].point, &samples[index].point);
        let dt = (b.timestamp - a.timestamp).abs() as f32;
        if dt > 0.0 && dt < 10.0 {
            let jump = match (a.airborne_altitude_ft(), b.airborne_altitude_ft()) {
                (Some(x), Some(y)) => (y - x).abs() > 10_000.0,
                _ => false,
            };
            let cos_lat = f64::from(b.lat).to_radians().cos() as f32;
            let dx = signed_longitude_delta(a.lon, b.lon) * M_PER_DEG_LON_EQUATOR * cos_lat;
            let dy = (b.lat - a.lat) * M_PER_DEG_LAT;
            let speed_kt = (dx * dx + dy * dy).sqrt() / dt * MPS_TO_KT;
            keep[index] = !(jump || speed_kt > MAX_PLAUSIBLE_SPEED_KT);
        }
        if keep[index] {
            previous = index;
        }
    }
    let mut flags = keep.into_iter();
    samples.retain(|_| flags.next().unwrap_or(false));
}

/// Drop a point that jumps off the chord of its neighbours and back (judged against the last kept
/// point and the next one).
fn drop_spikes(samples: &mut Vec<Sample>) {
    if samples.len() < 3 {
        return;
    }
    let mut keep = vec![true; samples.len()];
    let mut previous = 0;
    for index in 1..samples.len() - 1 {
        let (a, b, c) = (
            &samples[previous].point,
            &samples[index].point,
            &samples[index + 1].point,
        );
        keep[index] = !is_spike(a, b, c);
        if keep[index] {
            previous = index;
        }
    }
    let mut flags = keep.into_iter();
    samples.retain(|_| flags.next().unwrap_or(false));
}

/// Whether `b` leaves the chord from `a` to `c` by more than [`SPIKE_OFFSET_M`] and more than the
/// fastest turn allows, the three within [`SPIKE_WINDOW_S`].
fn is_spike(a: &TracePoint, b: &TracePoint, c: &TracePoint) -> bool {
    let (before, after) = (
        (b.timestamp - a.timestamp) as f32,
        (c.timestamp - b.timestamp) as f32,
    );
    if !(before > 0.0 && after > 0.0 && before + after <= SPIKE_WINDOW_S) {
        return false;
    }
    let cos_lat = f64::from(b.lat).to_radians().cos() as f32;
    let metres = |p: &TracePoint| {
        [
            signed_longitude_delta(b.lon, p.lon) * M_PER_DEG_LON_EQUATOR * cos_lat,
            (p.lat - b.lat) * M_PER_DEG_LAT,
        ]
    };
    let (from, to) = (metres(a), metres(c));
    let chord_m = (to[0] - from[0]).hypot(to[1] - from[1]);
    let offset_m = if chord_m > 0.0 {
        (from[0] * to[1] - from[1] * to[0]).abs() / chord_m
    } else {
        from[0].hypot(from[1])
    };
    let turn = (MAX_TURN_RATE_DEG_S * (before + after))
        .min(180.0)
        .to_radians();
    offset_m > (0.5 * chord_m * (0.25 * turn).tan()).max(SPIKE_OFFSET_M)
}

/// A segment worth keeping: finite, above the floor, at least 10 m, a positive duration at a
/// possible speed; a jet flies at 80 kt or more, a helicopter below the height ceiling.
pub fn segment_is_keepable(
    length_m: f32,
    dt_s: f32,
    start_height_m: f32,
    end_height_m: f32,
    mean_speed_kt: f32,
    airframe: Airframe,
    airborne: bool,
) -> bool {
    if !length_m.is_finite()
        || !start_height_m.is_finite()
        || !end_height_m.is_finite()
        || !mean_speed_kt.is_finite()
    {
        return false;
    }
    if start_height_m.min(end_height_m) < HARD_HEIGHT_FLOOR_M || length_m < MIN_SEGMENT_LENGTH_M {
        return false;
    }
    if !dt_s.is_finite() || dt_s <= 0.0 || length_m / dt_s * MPS_TO_KT > MAX_PLAUSIBLE_SPEED_KT {
        return false;
    }
    if airborne && airframe == Airframe::Jet && mean_speed_kt < JET_STALL_SPEED_KT {
        return false;
    }
    !(airborne
        && airframe == Airframe::Helicopter
        && start_height_m.max(end_height_m) > HELICOPTER_HEIGHT_CEILING_M)
}

#[cfg(test)]
#[path = "filters_tests.rs"]
mod tests;
