//! Union of two ADS-B providers per aircraft address and UTC day: every primary point, a secondary point only outside primary coverage, anonymous (`~`) echoes of an address track suppressed.
//!
//! The union feeds a difference estimator: segments touching a kept
//! secondary point carry `SECONDARY_ONLY` and are normalised by the
//! increment days, everything else by the baseline days. A secondary copy of
//! the primary therefore keeps no point and changes no byte.

use std::collections::{HashMap, HashSet};

use crate::filters::point_is_sane;
use crate::geo::flat_dist;
use crate::trace::{trace_identity, AircraftTrace, CallsignChange, TracePoint, FLAG_SECONDARY_PROVIDER};

/// Two providers' samples closer than this in time are one observation.
/// Overlap pilot 2026-09-24: adsb.lol merged with itself keeps 0 of
/// 99,908,590 points at 1 s.
pub const SAME_OBSERVATION_TOLERANCE_S: f64 = 1.0;

/// Anonymous-track coincidence, from the same pilot's track pairing (10 s ×
/// 0.004° × 0.006° × 400 ft bins; a pair coincides on ≥ 6 bins = 60 s): 172
/// adsb.lol `~` tracks duplicated an ADSBX address track and 53 `~` echoes
/// duplicated an adsb.lol address track on 2026-05-01.
const ANONYMOUS_MATCH_DISTANCE_M: f32 = 450.0;
const ANONYMOUS_MATCH_ALTITUDE_FT: f32 = 400.0;
const ANONYMOUS_MATCH_BRACKET_S: f64 = 30.0;
const ANONYMOUS_MATCH_BIN_S: f64 = 10.0;
const ANONYMOUS_MATCH_MIN_BINS: usize = 6;
/// Coarse candidate cells (60 s × 0.05°) bound the pairwise comparison; a
/// match needs a neighbouring cell at most.
const CANDIDATE_CELL_S: f64 = 60.0;
const CANDIDATE_CELL_DEG: f32 = 0.05;

/// What one provider-day union did, recorded in the day's receipt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeCounts {
    pub secondary_points_kept: u64,
    pub secondary_points_covered: u64,
    pub secondary_only_addresses: u64,
    pub anonymous_points_suppressed: u64,
}

/// Merge one UTC day of two providers. Secondary points that survive carry
/// [`FLAG_SECONDARY_PROVIDER`]; primary traces are never shortened.
pub fn merge_provider_traces(
    primary: Vec<AircraftTrace>,
    secondary: Vec<AircraftTrace>,
) -> (Vec<AircraftTrace>, MergeCounts) {
    let mut counts = MergeCounts::default();
    let mut merged = primary;
    let mut by_address: HashMap<(bool, u32), usize> = HashMap::with_capacity(merged.len());
    for (index, trace) in merged.iter().enumerate() {
        if let Some(identity) = trace_identity(&trace.icao24) {
            by_address.insert(identity, index);
        }
    }
    for mut trace in secondary {
        let known = trace_identity(&trace.icao24).and_then(|id| by_address.get(&id).copied());
        match known {
            Some(index) => {
                let kept = merge_one_address(&mut merged[index], trace);
                counts.secondary_points_kept += kept.0;
                counts.secondary_points_covered += kept.1;
            }
            None => {
                trace.retain_points(|_, point| point_is_sane(point));
                if trace.points.len() < 2 {
                    continue;
                }
                for point in &mut trace.points {
                    point.flags |= FLAG_SECONDARY_PROVIDER;
                }
                counts.secondary_only_addresses += 1;
                counts.secondary_points_kept += trace.points.len() as u64;
                merged.push(trace);
            }
        }
    }
    counts.anonymous_points_suppressed = suppress_anonymous_echoes(&mut merged);
    merged.retain(|trace| trace.points.len() >= 2);
    (merged, counts)
}

/// Disjoint, time-ordered intervals where the primary provider observed the
/// aircraft: ±1 s around every sane sample. Gap judgement belongs to Stage 1,
/// which classifies phases from DEM AGL with ground inference and hysteresis;
/// a barometric guess here would delete secondary samples from gaps Stage 1
/// will not bridge and keep ones inside gaps it will bridge.
fn primary_coverage(points: &[TracePoint]) -> Vec<(f64, f64)> {
    let mut sane: Vec<&TracePoint> = points.iter().filter(|p| point_is_sane(p)).collect();
    sane.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    let mut intervals: Vec<(f64, f64)> = Vec::with_capacity(sane.len());
    for point in sane {
        let (start, end) = (
            point.timestamp - SAME_OBSERVATION_TOLERANCE_S,
            point.timestamp + SAME_OBSERVATION_TOLERANCE_S,
        );
        match intervals.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => intervals.push((start, end)),
        }
    }
    intervals
}

fn covered(intervals: &[(f64, f64)], timestamp: f64) -> bool {
    let after = intervals.partition_point(|interval| interval.0 <= timestamp);
    after > 0 && intervals[after - 1].1 >= timestamp
}

/// Returns (kept, covered) secondary sample counts.
fn merge_one_address(primary: &mut AircraftTrace, secondary: AircraftTrace) -> (u64, u64) {
    let intervals = primary_coverage(&primary.points);
    let mut kept_points = Vec::new();
    let mut kept_callsigns = Vec::new();
    let mut covered_count = 0u64;
    let mut transitions = secondary.callsigns.iter().peekable();
    // A transition on a dropped sample carries to the next kept one.
    let mut pending_callsign: Option<String> = None;
    for (index, mut point) in secondary.points.into_iter().enumerate() {
        while let Some(change) = transitions.next_if(|c| c.point_idx <= index) {
            pending_callsign = Some(change.value.clone());
        }
        if !point_is_sane(&point) {
            continue;
        }
        if covered(&intervals, point.timestamp) {
            covered_count += 1;
            continue;
        }
        point.flags |= FLAG_SECONDARY_PROVIDER;
        if let Some(value) = pending_callsign.take() {
            kept_callsigns.push((kept_points.len(), value));
        }
        kept_points.push(point);
    }
    let kept = kept_points.len() as u64;
    if kept == 0 {
        return (0, covered_count);
    }
    if primary.aircraft_type.trim().is_empty() {
        primary.aircraft_type = secondary.aircraft_type;
    }
    // Interleave by time; the primary sample goes first on equal timestamps
    // (none survive the tolerance, but ordering stays total).
    let primary_points = std::mem::take(&mut primary.points);
    let mut order: Vec<(f64, bool, usize)> = primary_points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.timestamp, false, i))
        .chain(
            kept_points
                .iter()
                .enumerate()
                .map(|(i, p)| (p.timestamp, true, i)),
        )
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut primary_position = vec![0usize; primary_points.len()];
    let mut secondary_position = vec![0usize; kept_points.len()];
    let mut points = Vec::with_capacity(order.len());
    for (position, &(_, is_secondary, index)) in order.iter().enumerate() {
        if is_secondary {
            secondary_position[index] = position;
            points.push(kept_points[index].clone());
        } else {
            primary_position[index] = position;
            points.push(primary_points[index].clone());
        }
    }
    let mut callsigns: Vec<CallsignChange> = std::mem::take(&mut primary.callsigns)
        .into_iter()
        .filter(|c| c.point_idx < primary_position.len())
        .map(|c| CallsignChange {
            point_idx: primary_position[c.point_idx],
            value: c.value,
        })
        .collect();
    callsigns.extend(
        kept_callsigns
            .into_iter()
            .map(|(index, value)| CallsignChange {
                point_idx: secondary_position[index],
                value,
            }),
    );
    callsigns.sort_by_key(|c| c.point_idx);
    callsigns.dedup_by(|later, earlier| later.value == earlier.value);
    primary.points = points;
    primary.callsigns = callsigns;
    (kept, covered_count)
}

fn candidate_cell(point: &TracePoint) -> (i64, i32, i32) {
    (
        (point.timestamp / CANDIDATE_CELL_S).floor() as i64,
        (point.lat / CANDIDATE_CELL_DEG).floor() as i32,
        (point.lon / CANDIDATE_CELL_DEG).floor() as i32,
    )
}

fn neighbouring_cells(cell: (i64, i32, i32)) -> impl Iterator<Item = (i64, i32, i32)> {
    (-1..=1).flat_map(move |dt| {
        (-1..=1).flat_map(move |dy| (-1..=1).map(move |dx| (cell.0 + dt, cell.1 + dy, cell.2 + dx)))
    })
}

/// Drop `~` samples that coincide with one address track for at least
/// [`ANONYMOUS_MATCH_MIN_BINS`] ten-second bins; address–address pairs stay.
fn suppress_anonymous_echoes(traces: &mut [AircraftTrace]) -> u64 {
    let anonymous: Vec<usize> = (0..traces.len())
        .filter(|&i| trace_identity(&traces[i].icao24).is_some_and(|(tilde, _)| tilde))
        .collect();
    if anonymous.is_empty() {
        return 0;
    }
    let mut wanted: HashSet<(i64, i32, i32)> = HashSet::new();
    for &index in &anonymous {
        for point in traces[index].points.iter().filter(|p| point_is_sane(p)) {
            wanted.extend(neighbouring_cells(candidate_cell(point)));
        }
    }
    let mut address_cells: HashMap<(i64, i32, i32), Vec<usize>> = HashMap::new();
    for (index, trace) in traces.iter().enumerate() {
        if trace_identity(&trace.icao24).is_none_or(|(tilde, _)| tilde) {
            continue;
        }
        for point in trace.points.iter().filter(|p| point_is_sane(p)) {
            let cell = candidate_cell(point);
            if wanted.contains(&cell) {
                let list = address_cells.entry(cell).or_default();
                if list.last() != Some(&index) {
                    list.push(index);
                }
            }
        }
    }
    let mut suppressed = 0u64;
    for index in anonymous {
        let mut candidates: Vec<usize> = Vec::new();
        let mut seen_cells = HashSet::new();
        for point in traces[index].points.iter().filter(|p| point_is_sane(p)) {
            if !seen_cells.insert(candidate_cell(point)) {
                continue;
            }
            for cell in neighbouring_cells(candidate_cell(point)) {
                if let Some(list) = address_cells.get(&cell) {
                    candidates.extend(list);
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let mut drop = vec![false; traces[index].points.len()];
        for candidate in candidates {
            let matched = coinciding_samples(&traces[index].points, &traces[candidate].points);
            let bins: HashSet<i64> = matched
                .iter()
                .map(|&i| (traces[index].points[i].timestamp / ANONYMOUS_MATCH_BIN_S).floor() as i64)
                .collect();
            if bins.len() >= ANONYMOUS_MATCH_MIN_BINS {
                for i in matched {
                    drop[i] = true;
                }
            }
        }
        let before = traces[index].points.len();
        traces[index].retain_points(|i, _| !drop[i]);
        suppressed += (before - traces[index].points.len()) as u64;
    }
    suppressed
}

/// Indices of `anonymous` samples lying on the `address` track: the address
/// position interpolated between samples at most 30 s apart (or a sample
/// within 1 s) within 450 m, and within 400 ft when both are airborne.
fn coinciding_samples(anonymous: &[TracePoint], address: &[TracePoint]) -> Vec<usize> {
    let mut track: Vec<&TracePoint> = address.iter().filter(|p| point_is_sane(p)).collect();
    track.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    let mut matched = Vec::new();
    for (index, point) in anonymous.iter().enumerate() {
        if !point_is_sane(point) {
            continue;
        }
        let t = point.timestamp;
        let after = track.partition_point(|p| p.timestamp < t);
        let reference = match (after.checked_sub(1).map(|i| track[i]), track.get(after)) {
            (Some(a), Some(b)) if b.timestamp - a.timestamp <= ANONYMOUS_MATCH_BRACKET_S => {
                interpolate(a, b, t)
            }
            (a, b) => {
                let nearest = [a, b.copied()]
                    .into_iter()
                    .flatten()
                    .filter(|p| (p.timestamp - t).abs() <= SAME_OBSERVATION_TOLERANCE_S)
                    .min_by(|x, y| (x.timestamp - t).abs().total_cmp(&(y.timestamp - t).abs()));
                match nearest {
                    Some(p) => (p.lat, p.lon, p.airborne_alt_ft()),
                    None => continue,
                }
            }
        };
        let (lat, lon, alt_ft) = reference;
        if flat_dist(point.lat, point.lon, lat, lon) > ANONYMOUS_MATCH_DISTANCE_M {
            continue;
        }
        let same_level = match (point.airborne_alt_ft(), alt_ft) {
            (Some(a), Some(b)) => (a - b).abs() <= ANONYMOUS_MATCH_ALTITUDE_FT,
            (None, None) => true,
            _ => false,
        };
        if same_level {
            matched.push(index);
        }
    }
    matched
}

fn interpolate(a: &TracePoint, b: &TracePoint, t: f64) -> (f32, f32, Option<f32>) {
    let span = b.timestamp - a.timestamp;
    let w = if span > 0.0 {
        ((t - a.timestamp) / span) as f32
    } else {
        0.0
    };
    let (lat, lon) = crate::geo::interp_along_path(a.lat, a.lon, b.lat, b.lon, w);
    let alt = match (a.airborne_alt_ft(), b.airborne_alt_ft()) {
        (Some(x), Some(y)) => Some(x + (y - x) * w),
        (x, y) => {
            if w < 0.5 {
                x
            } else {
                y
            }
        }
    };
    (lat, lon, alt)
}

#[cfg(test)]
#[path = "provider_merge_tests.rs"]
mod tests;
