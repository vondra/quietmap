//! Union of two ADS-B providers per address and UTC day (dev4 `provider_merge.rs`): every primary
//! sample; a secondary sample only outside +-1 s of a primary one, flagged; anonymous (`~`) echoes
//! of an address track suppressed. Segments touching a kept secondary sample are normalised by the
//! increment days (sum P/B + S/I), so a secondary copy of the primary changes no byte.

use super::echoes::suppress_anonymous_echoes;
use super::filters::point_is_sane;
use super::trace::{
    AircraftTrace, CallsignChange, SECONDARY_PROVIDER, TracePoint, address_identity,
};
use std::collections::HashMap;

/// Two providers' samples closer than this are one observation (adsb.lol merged with itself
/// keeps 0 of 99,908,590 points at 1 s).
pub const SAME_OBSERVATION_S: f64 = 1.0;

/// What one provider-day union did (recorded in the day receipt).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeCounts {
    pub secondary_points_kept: u64,
    pub secondary_points_covered: u64,
    pub secondary_only_addresses: u64,
    pub anonymous_points_suppressed: u64,
}

pub fn merge_providers(
    primary: Vec<AircraftTrace>,
    secondary: Vec<AircraftTrace>,
) -> (Vec<AircraftTrace>, MergeCounts) {
    let mut counts = MergeCounts::default();
    let mut merged = primary;
    let by_address: HashMap<(bool, u32), usize> = merged
        .iter()
        .enumerate()
        .filter_map(|(index, trace)| Some((address_identity(&trace.address)?, index)))
        .collect();
    for mut trace in secondary {
        match address_identity(&trace.address).and_then(|id| by_address.get(&id).copied()) {
            Some(index) => {
                let (kept, covered) = merge_one_address(&mut merged[index], trace);
                counts.secondary_points_kept += kept;
                counts.secondary_points_covered += covered;
            }
            None => {
                trace.retain_points(|_, point| point_is_sane(point));
                if trace.points.len() < 2 {
                    continue;
                }
                for point in &mut trace.points {
                    point.flags |= SECONDARY_PROVIDER;
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

/// Disjoint time intervals the primary provider observed: +-1 s around every sane sample.
fn primary_coverage(points: &[TracePoint]) -> Vec<(f64, f64)> {
    let mut times: Vec<f64> = points
        .iter()
        .filter(|p| point_is_sane(p))
        .map(|p| p.timestamp)
        .collect();
    times.sort_by(f64::total_cmp);
    let mut intervals: Vec<(f64, f64)> = Vec::with_capacity(times.len());
    for time in times {
        let (start, end) = (time - SAME_OBSERVATION_S, time + SAME_OBSERVATION_S);
        match intervals.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => intervals.push((start, end)),
        }
    }
    intervals
}

/// Returns (kept, covered) secondary sample counts.
fn merge_one_address(primary: &mut AircraftTrace, secondary: AircraftTrace) -> (u64, u64) {
    let intervals = primary_coverage(&primary.points);
    let covered_at = |t: f64| {
        let after = intervals.partition_point(|interval| interval.0 <= t);
        after > 0 && intervals[after - 1].1 >= t
    };
    let (mut kept_points, mut kept_callsigns, mut covered) = (Vec::new(), Vec::new(), 0u64);
    let mut changes = secondary.callsigns.iter().peekable();
    // A callsign change on a dropped sample carries to the next kept one.
    let mut pending: Option<String> = None;
    for (index, mut point) in secondary.points.into_iter().enumerate() {
        while let Some(change) = changes.next_if(|c| c.point_index <= index) {
            pending = Some(change.callsign.clone());
        }
        if !point_is_sane(&point) {
            continue;
        }
        if covered_at(point.timestamp) {
            covered += 1;
            continue;
        }
        point.flags |= SECONDARY_PROVIDER;
        if let Some(callsign) = pending.take() {
            kept_callsigns.push((kept_points.len(), callsign));
        }
        kept_points.push(point);
    }
    let kept = kept_points.len() as u64;
    if kept == 0 {
        return (0, covered);
    }
    if primary.aircraft_type.trim().is_empty() {
        primary.aircraft_type = secondary.aircraft_type;
    }
    // Interleave by time; a primary sample goes first on equal times.
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
    let mut primary_position = vec![0; primary_points.len()];
    let mut secondary_position = vec![0; kept_points.len()];
    let mut points = Vec::with_capacity(order.len());
    for (position, &(_, from_secondary, index)) in order.iter().enumerate() {
        if from_secondary {
            secondary_position[index] = position;
            points.push(kept_points[index]);
        } else {
            primary_position[index] = position;
            points.push(primary_points[index]);
        }
    }
    let mut callsigns: Vec<CallsignChange> = std::mem::take(&mut primary.callsigns)
        .into_iter()
        .filter(|c| c.point_index < primary_position.len())
        .map(|c| CallsignChange {
            point_index: primary_position[c.point_index],
            callsign: c.callsign,
        })
        .chain(
            kept_callsigns
                .into_iter()
                .map(|(index, callsign)| CallsignChange {
                    point_index: secondary_position[index],
                    callsign,
                }),
        )
        .collect();
    callsigns.sort_by_key(|c| c.point_index);
    callsigns.dedup_by(|later, earlier| later.callsign == earlier.callsign);
    primary.points = points;
    primary.callsigns = callsigns;
    (kept, covered)
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
