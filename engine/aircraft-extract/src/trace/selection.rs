//! Select one intact observed aircraft-day trace before rotation IDs can hide export overlaps.

use super::AircraftTrace;
use crate::filters::point_is_sane;
use std::collections::HashMap;

pub(super) fn select_whole_traces(traces: Vec<AircraftTrace>) -> Vec<AircraftTrace> {
    let mut indices = HashMap::new();
    let mut selected: Vec<AircraftTrace> = Vec::with_capacity(traces.len());
    let mut alternatives = 0;
    let (mut clipped_traces, mut clipped_points) = (0usize, 0usize);
    for trace in traces {
        let identity = trace_identity(&trace.icao24);
        let Some(index) = identity.and_then(|id| indices.get(&id).copied()) else {
            if let Some(id) = identity {
                indices.insert(id, selected.len());
            }
            selected.push(trace);
            continue;
        };
        alternatives += 1;
        let sane_count = |tr: &AircraftTrace| tr.points.iter().filter(|p| point_is_sane(p)).count();
        let discarded = if sane_count(&trace) > sane_count(&selected[index]) {
            std::mem::replace(&mut selected[index], trace)
        } else {
            trace
        };
        let outside = unselected_boundary_points(&discarded, &selected[index]);
        clipped_traces += usize::from(outside > 0);
        clipped_points += outside;
    }
    if alternatives > 0 {
        eprintln!(
            "{} [stage0] selected {} whole aircraft-day traces; discarded {alternatives} alternative exports (not a lossless point merge); {clipped_traces} discarded exports hold {clipped_points} sane points outside their selected trace",
            crate::progress::ts(),
            selected.len()
        );
    }
    selected
}

pub(crate) fn trace_identity(raw: &str) -> Option<(bool, u32)> {
    let anonymous = raw.strip_prefix('~');
    let icao = crate::profile::parse_icao24_hex(anonymous.unwrap_or(raw))?;
    // Upstream ~ IDs have their own namespace. Empty/invalid IDs and reserved
    // ordinary addresses cannot identify one aircraft and must not be grouped.
    if anonymous.is_none() && matches!(icao, 0 | 0xff_ffff) {
        return None;
    }
    Some((anonymous.is_some(), icao))
}

/// Sane samples of the discarded export outside the selected trace's time span.
fn unselected_boundary_points(discarded: &AircraftTrace, selected: &AircraftTrace) -> usize {
    let mut times = selected
        .points
        .iter()
        .filter(|p| point_is_sane(p))
        .map(|p| p.timestamp);
    let Some(first) = times.next() else {
        return 0;
    };
    let (min, max) = times.fold((first, first), |(lo, hi), t| (lo.min(t), hi.max(t)));
    discarded
        .points
        .iter()
        .filter(|p| point_is_sane(p) && (p.timestamp < min || p.timestamp > max))
        .count()
}
