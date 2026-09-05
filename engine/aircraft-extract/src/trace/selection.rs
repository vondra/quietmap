//! Select one intact observed aircraft-day trace before rotation IDs can hide export overlaps.

use super::AircraftTrace;
use crate::filters::point_is_sane;
use std::collections::HashMap;

pub(super) fn select_whole_traces(traces: Vec<AircraftTrace>) -> Vec<AircraftTrace> {
    let mut indices = HashMap::new();
    let mut selected: Vec<AircraftTrace> = Vec::with_capacity(traces.len());
    let mut alternatives = 0;
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
        report_unselected_boundary_points(&discarded, &selected[index]);
    }
    if alternatives > 0 {
        eprintln!(
            "{} [stage0] selected {} whole aircraft-day traces; discarded {alternatives} alternative exports (not a lossless point merge)",
            crate::progress::ts(),
            selected.len()
        );
    }
    selected
}

fn trace_identity(raw: &str) -> Option<(bool, u32)> {
    let anonymous = raw.strip_prefix('~');
    let icao = crate::profile::parse_icao24_hex(anonymous.unwrap_or(raw))?;
    // Upstream ~ IDs have their own namespace. Empty/invalid IDs and reserved
    // ordinary addresses cannot identify one aircraft and must not be grouped.
    if anonymous.is_none() && matches!(icao, 0 | 0xff_ffff) {
        return None;
    }
    Some((anonymous.is_some(), icao))
}

fn report_unselected_boundary_points(discarded: &AircraftTrace, selected: &AircraftTrace) {
    let mut times = selected
        .points
        .iter()
        .filter(|p| point_is_sane(p))
        .map(|p| p.timestamp);
    let Some(first) = times.next() else {
        return;
    };
    let (min, max) = times.fold((first, first), |(lo, hi), t| (lo.min(t), hi.max(t)));
    let outside = discarded
        .points
        .iter()
        .filter(|p| point_is_sane(p) && (p.timestamp < min || p.timestamp > max))
        .count();
    if outside > 0 {
        eprintln!(
            "{} [stage0] {}: unselected alternative has {outside} sane points outside selected [{min}, {max}]; original source retained, acoustic effect unmeasured",
            crate::progress::ts(),
            discarded.icao24
        );
    }
}
