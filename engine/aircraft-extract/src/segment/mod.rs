//! Per-sample segment geometry, phase gaps, and departure classification using dev1 physics.

use crate::filters;
use crate::flight::{segment_flags, FlightSegment, Phase};
use crate::geo::{flat_dist, midpoint};
use crate::period::period_from_timestamp;
use crate::trace::TracePoint;

/// Minimum continuous on-ground time (s) that registers as a turnaround
/// and ends one [`Flight`] leg. Civil-aviation gate-to-gate turnaround
/// is ≥ 20 min for narrowbodies; 5 min is a conservative floor that
/// safely excludes runway holds, touch-and-go, and line-up-and-wait.
pub const MIN_TURNAROUND_S: f64 = 5.0 * 60.0;

/// Maximum within-flight gap (s) when both endpoints classify as
/// [`Phase::Cruise`]. Oceanic, polar, and other sparse-coverage transits
/// at FL300+ can legitimately have ADS-B dropouts up to ~1 h; rejecting
/// them would drop real cruise traffic over coverage holes.
pub const GAP_S_CRUISE: f64 = 3600.0;
/// Maximum within-flight gap (s) when the later endpoint is
/// [`Phase::Airborne`]. Terminal-area ops have dense ADS-B; gaps > 2 min
/// usually mean lost coverage that would interpolate through unknown
/// climb/descent geometry.
pub const GAP_S_AIRBORNE: f64 = 120.0;
/// Maximum within-flight gap (s) when the later endpoint is
/// [`Phase::Ground`]. Gate / taxi data is usually continuous; > 1 min
/// means lost coverage between known ground states.
pub const GAP_S_GROUND: f64 = 60.0;

/// Half-window radius (samples) for ROCD median smoothing in
/// [`classify_is_departure_per_sample`]. ±5 at typical ADS-B cadence
/// (5-15 s) spans ~30-90 s — long enough to median out single-sample
/// baro noise, short enough to resolve climb→descent transitions.
const ROCD_SMOOTH_HALF: usize = 5;

/// Index ranges of distinct flights in one aircraft's daily trace.
/// A flight leg ends when motion-validated raw ground
/// samples cover ≥ [`MIN_TURNAROUND_S`] of time; the next leg starts
/// at the first non-ground sample after the rest. Signal dropouts in
/// the air — even hours-long oceanic gaps — preserve flight identity
/// because no ground bit is set. Single-point ranges are dropped —
/// every emitted range has `len() >= 2`.
pub fn split_flights(points: &[TracePoint]) -> Vec<std::ops::Range<usize>> {
    if points.len() < 2 {
        return Vec::new();
    }
    // Typical commercial aircraft does 1-4 rotations per day; capacity 4
    // avoids the first reallocation on every aircraft trace.
    let mut ranges = Vec::with_capacity(4);
    let mut leg_start = 0usize;
    let mut ground_run_start: Option<usize> = None;
    // A trace that begins with a long ground rest (parked overnight,
    // signal acquired pre-takeoff) has no prior airborne content to
    // emit as its own leg; absorb that rest into the first real leg.
    let mut leg_has_airborne = false;
    for i in 0..points.len() {
        if crate::ground_inference::raw_ground_motion(&points[i]) {
            ground_run_start.get_or_insert(i);
            continue;
        }
        if let Some(gs) = ground_run_start.take() {
            let run_dt = points[i - 1].timestamp - points[gs].timestamp;
            if run_dt >= MIN_TURNAROUND_S && leg_has_airborne {
                // Ground samples up to i-1 stay with the previous leg
                // (taxi-in / park); the new leg starts at this lift-off.
                ranges.push(leg_start..i);
                leg_start = i;
            }
        }
        leg_has_airborne = true;
    }
    if points.len() - leg_start >= 2 {
        ranges.push(leg_start..points.len());
    }
    ranges
}

/// Fixed-per-flight metadata copied onto every segment this flight
/// emits. Bundled to avoid threading 7 individual params through
/// [`build_segments`].
pub struct SegmentMeta<'a> {
    pub flight_id: u64,
    pub callsign: &'a str,
    pub aircraft_type: [u8; 4],
    pub profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    pub veh_kind: u8,
    pub gse_class: u8,
    pub date_id: i16,
}

/// Build [`FlightSegment`] rows for one flight by emitting one segment
/// per consecutive ADS-B sample-pair. Phase classification and
/// per-point AGL have already been computed upstream; this layer
/// applies phase-aware gap filtering, per-pair `is_departure`, and
/// receiver-independent validity checks.
pub fn build_segments(
    points: &[TracePoint],
    agl_m: &[f32],
    elev_m: &[f32],
    phases: &[Phase],
    meta: &SegmentMeta<'_>,
) -> Vec<FlightSegment> {
    debug_assert_eq!(points.len(), agl_m.len());
    debug_assert_eq!(points.len(), elev_m.len());
    debug_assert_eq!(points.len(), phases.len());
    if points.len() < 2 {
        return Vec::new();
    }
    // Ground points carry NaN alt_ft; cache the Option once per point so
    // downstream reads share one branch.
    let alts_ft: Vec<Option<f32>> = points.iter().map(|p| p.airborne_alt_ft()).collect();
    let is_dep_per_sample = classify_is_departure_per_sample(points, &alts_ft, phases);
    let mut out = Vec::with_capacity(points.len() - 1);
    for i in 1..points.len() {
        let dt = points[i].timestamp - points[i - 1].timestamp;
        if dt <= 0.0 {
            continue;
        }
        let Some(phase) = segment_phase(phases[i - 1], phases[i]) else {
            continue;
        };
        if dt > gap_budget_for(phases[i - 1], phases[i]) {
            continue;
        }
        let on_ground = phase == Phase::Ground;
        let prev = &points[i - 1];
        let curr = &points[i];
        let length_m = flat_dist(prev.lat, prev.lon, curr.lat, curr.lon);
        let avg_speed = (prev.speed_kt + curr.speed_kt) * 0.5;
        if !filters::segment_is_keepable(
            length_m,
            dt as f32,
            agl_m[i - 1],
            agl_m[i],
            avg_speed,
            meta.profile_idx,
            !on_ground,
        ) {
            continue;
        }
        let mid_time = (prev.timestamp + curr.timestamp) * 0.5;
        let (mid_lat, mid_lon) = midpoint(prev.lat, prev.lon, curr.lat, curr.lon);
        let period = period_from_timestamp(mid_time, mid_lat as f64, mid_lon as f64);
        // is_dep semantics differ by phase: airborne uses ROCD trend
        // (Doc 29 §A.3.2), ground uses speed acceleration trend
        // (takeoff roll vs. landing rollout / steady taxi). Both
        // branches live in classify_is_departure_per_sample so the
        // build_segments loop is phase-blind here.
        let is_dep = is_dep_per_sample[i];
        let mut flags = 0u8;
        if is_dep {
            flags |= segment_flags::IS_DEPARTURE;
        }
        if on_ground {
            flags |= segment_flags::ON_GROUND;
        }
        // Ground endpoint inherits the airborne endpoint's altitude so
        // elevated-airport lift-off / flare segments pass the downstream
        // terrain-vs-altitude airborne validation. Both-ground pairs
        // emit 0 m alt — segments.arrow consumers gate on the
        // on_ground flag and read elevation from the raster.
        let start_alt_m = alts_ft[i - 1].or(alts_ft[i]).unwrap_or(0.0) * 0.3048;
        let end_alt_m = alts_ft[i].or(alts_ft[i - 1]).unwrap_or(0.0) * 0.3048;
        out.push(FlightSegment {
            flight_id: meta.flight_id,
            callsign: meta.callsign.to_string(),
            aircraft_type: meta.aircraft_type,
            profile_idx: meta.profile_idx,
            source_id: meta.source_id,
            origin: meta.origin,
            veh_kind: meta.veh_kind,
            gse_class: meta.gse_class,
            period,
            date_id: meta.date_id,
            phase,
            flags,
            start_lat: prev.lat,
            start_lon: prev.lon,
            start_alt_m,
            end_lat: curr.lat,
            end_lon: curr.lon,
            end_alt_m,
            speed_kt: avg_speed,
            length_m,
            agl_avg_m: (agl_m[i - 1] + agl_m[i]) * 0.5,
            start_elev_m: elev_m[i - 1],
            end_elev_m: elev_m[i],
        });
    }
    out
}

/// Direct Cruise↔Ground observations contain no measured climb/descent geometry.
/// Reject that hole; otherwise Airborne wins to preserve takeoff/flare and approach NPD.
#[inline]
fn segment_phase(prev: Phase, curr: Phase) -> Option<Phase> {
    match (prev, curr) {
        (Phase::Cruise, Phase::Ground) | (Phase::Ground, Phase::Cruise) => None,
        (Phase::Airborne, _) | (_, Phase::Airborne) => Some(Phase::Airborne),
        _ => Some(curr),
    }
}

/// Maximum allowed time gap (s) for a sample-pair, keyed to the more
/// permissive endpoint phase. A cruise dropout reappearing in airborne
/// gets the cruise budget so the descent transition isn't lost; a true
/// airborne→ground gap stays bounded at the airborne ceiling.
#[inline]
fn gap_budget_for(prev: Phase, curr: Phase) -> f64 {
    let max_phase = if (prev as u8) >= (curr as u8) {
        prev
    } else {
        curr
    };
    match max_phase {
        Phase::Cruise => GAP_S_CRUISE,
        Phase::Airborne => GAP_S_AIRBORNE,
        Phase::Ground => GAP_S_GROUND,
    }
}

/// Speed-acceleration threshold separating takeoff-roll departures
/// from steady taxi and landing rollouts. Doc 29 4th Ed §A.3 implies
/// typical jet takeoff averages ~180–250 kt/min (V_R 140–180 kt reached
/// in 30–40 s of roll); 60 kt/min sits at ~25–35 % of that — well
/// above ADS-B speed-sample jitter and apron stop/start noise
/// (±30 kt over 10–15 s ≈ ±120–180 kt/min in a single pair but
/// median-smoothed out), well below even derated heavy-jet rolls.
/// Decelerating rollouts (~−200..−400 kt/min) and steady taxi (~0)
/// stay clearly below.
const GROUND_DEPARTURE_ACCEL_KT_PER_MIN: f32 = 60.0;

/// Per-sample `is_departure` classification. Index `i` applies to the
/// sample-pair `(i-1, i)`; index 0 is unused. Phase-aware:
///
/// * **Airborne pairs** (both endpoints have a barometric altitude) —
///   Doc 29 §A.3.2 routes en-route cruise to the Departure NPD because
///   cruise thrust ≈ T/O thrust, so the threshold is altitude-aware:
///   shallow descents at FL100+ stay Departure while steeper descents
///   flip to Approach. Per-step ROCD is medianed over
///   ±[`ROCD_SMOOTH_HALF`] samples to ride out baro jitter. Derived
///   from `Δalt/Δt` rather than `baro_rate_fpm` (some receivers
///   zero-fill that field).
///
/// * **Ground pairs** (at least one endpoint without a barometric
///   altitude — i.e. an on-ground ADS-B sample) — speed-trend
///   classification: a takeoff roll accelerates monotonically, a
///   landing rollout decelerates monotonically, and steady taxi
///   oscillates around zero. Per-step Δspeed/Δt is medianed over the
///   same window and thresholded at
///   [`GROUND_DEPARTURE_ACCEL_KT_PER_MIN`].
fn classify_is_departure_per_sample(
    points: &[TracePoint],
    alts_ft: &[Option<f32>],
    phases: &[Phase],
) -> Vec<bool> {
    let n = points.len();
    if n < 2 {
        return vec![false; n];
    }
    debug_assert_eq!(n, phases.len());
    // rocd_step[i] = ROCD across (i-1, i) pair, fpm. NaN for pairs
    // with corrupt timing or at least one ground-flagged endpoint.
    // speed_step[i] = pair acceleration in kt/min. NaN only for
    // corrupt timing — ground samples carry speed_kt natively.
    let mut rocd_step = vec![f32::NAN; n];
    let mut speed_step = vec![f32::NAN; n];
    for i in 1..n {
        let dt_s = points[i].timestamp - points[i - 1].timestamp;
        if dt_s <= 0.0 {
            continue;
        }
        let dt_min = dt_s as f32 / 60.0;
        if let (Some(a), Some(b)) = (alts_ft[i - 1], alts_ft[i]) {
            rocd_step[i] = (b - a) / dt_min;
        }
        speed_step[i] = (points[i].speed_kt - points[i - 1].speed_kt) / dt_min;
    }
    let mut result = vec![false; n];
    let mut window: Vec<f32> = Vec::with_capacity(2 * ROCD_SMOOTH_HALF + 1);
    let median = |w: &mut Vec<f32>| -> Option<f32> {
        if w.is_empty() {
            return None;
        }
        let mid = w.len() / 2;
        let (_, m, _) = w.select_nth_unstable_by(mid, |a, b| {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        });
        Some(*m)
    };
    for i in 1..n {
        let lo = i.saturating_sub(ROCD_SMOOTH_HALF);
        let hi = (i + ROCD_SMOOTH_HALF + 1).min(n);
        // Phase-based gate, not alt-sentinel-based: `ground_inference`
        // can stamp Phase::Ground on points whose `alt_ft` is a real
        // numeric ground-elevation (from baro + on_ground flag, or
        // surface-signature inference), so `alts_ft.is_none()` alone
        // would miss them and the speed-trend branch would never fire.
        let on_ground_pair = phases[i - 1] == Phase::Ground || phases[i] == Phase::Ground;
        if on_ground_pair {
            window.clear();
            window.extend(speed_step[lo..hi].iter().copied().filter(|v| v.is_finite()));
            if let Some(smoothed) = median(&mut window) {
                result[i] = smoothed > GROUND_DEPARTURE_ACCEL_KT_PER_MIN;
            }
        } else {
            window.clear();
            window.extend(rocd_step[lo..hi].iter().copied().filter(|v| v.is_finite()));
            if let Some(smoothed_rocd) = median(&mut window) {
                let avg_alt = match (alts_ft[i - 1], alts_ft[i]) {
                    (Some(a), Some(b)) => (a + b) * 0.5,
                    _ => 0.0,
                };
                result[i] = smoothed_rocd > 500.0 || (avg_alt > 10_000.0 && smoothed_rocd > -500.0);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod departure_tests;
