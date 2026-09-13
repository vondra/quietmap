//! Allocate one physical cross-section once; longitudinal pieces retain through-flow.

use crate::input::Road;
use noise_compute::defaults::resolve_traffic_default;
use noise_compute::normalize::road::{access_factor, lane_ratio};

// Conservative matching envelope inherited from European source admission.
// A common corridor and a crossing perpendicular section are required; proximity alone never establishes a shared carriageway.
const MAX_CARRIAGEWAY_SEPARATION_M: f64 = 50.0;
const MIN_PARALLEL_COSINE: f64 = 0.9659258262890683; // cos(15 degrees)

pub(crate) fn compatible_alternative(road: &Road, candidate: &Road) -> bool {
    if road.way_id == candidate.way_id || road.direction == 0 || candidate.direction == 0
        || road.class != candidate.class || road.country != candidate.country
        || !((!road.corridor.is_empty() && road.corridor == candidate.corridor)
            || (!road.observation.is_empty() && road.observation_source_id == candidate.observation_source_id
                && road.observation == candidate.observation)) {
        return false;
    }
    let a = road.travel_vector();
    let b = candidate.travel_vector();
    let length_a = a.0.hypot(a.1);
    let length_b = b.0.hypot(b.1);
    if length_a == 0.0 || length_b == 0.0
        || (a.0 * b.0 + a.1 * b.1).abs() / (length_a * length_b) < MIN_PARALLEL_COSINE {
        return false;
    }
    true
}

fn projection(road: &Road, point: (f64, f64)) -> f64 {
    let axis = (road.end.0 - road.start.0, road.end.1 - road.start.1);
    ((point.0 - road.start.0) * axis.0 + (point.1 - road.start.1) * axis.1)
        / (axis.0 * axis.0 + axis.1 * axis.1)
}

pub(crate) fn longitudinal_overlap(road: &Road, candidate: &Road) -> bool {
    let from = projection(road, candidate.start);
    let to = projection(road, candidate.end);
    from.min(to) < 1.0 && from.max(to) > 0.0
}

fn lateral_at(road: &Road, candidate: &Road, position: f64) -> (f64, f64) {
    let axis = (road.end.0 - road.start.0, road.end.1 - road.start.1);
    let other = (candidate.end.0 - candidate.start.0, candidate.end.1 - candidate.start.1);
    let point = (road.start.0 + position * axis.0, road.start.1 + position * axis.1);
    let along = ((point.0 - candidate.start.0) * axis.0 + (point.1 - candidate.start.1) * axis.1)
        / (other.0 * axis.0 + other.1 * axis.1);
    let delta = (candidate.start.0 + along * other.0 - point.0, candidate.start.1 + along * other.1 - point.1);
    ((delta.0 * -axis.1 + delta.1 * axis.0) / axis.0.hypot(axis.1), along)
}

fn cross_section<'a>(road: &'a Road, candidates: impl IntoIterator<Item = &'a Road>) -> Vec<&'a Road> {
    let mut positions = vec![(0.0, road)];
    for candidate in candidates {
        if !compatible_alternative(road, candidate) { continue; }
        let (lateral, along) = lateral_at(road, candidate, 0.5);
        // Half-open intervals count one longitudinal piece at a shared endpoint.
        if (0.0..1.0).contains(&along) { positions.push((lateral, candidate)); }
    }
    positions.sort_by(|a, b| a.0.total_cmp(&b.0));
    let center = positions.iter().position(|(_, candidate)| std::ptr::eq(*candidate, road)).unwrap();
    let reach = MAX_CARRIAGEWAY_SEPARATION_M / road.mercator_scale();
    let mut left = center;
    let mut right = center;
    while left > 0 && positions[left].0 - positions[left - 1].0 <= reach { left -= 1; }
    while right + 1 < positions.len() && positions[right + 1].0 - positions[right].0 <= reach { right += 1; }
    let mut ways = std::collections::BTreeMap::new();
    for (_, candidate) in &positions[left..=right] { ways.entry(candidate.way_id).or_insert(*candidate); }
    ways.into_values().collect()
}

pub fn resolve<'a>(road: &'a Road, candidates: impl IntoIterator<Item = &'a Road>) -> ([f64; 4], u8) {
    if road.basis == 3 { return (road.counts, road.estimated); }
    let measured = road.provenance.is_measured() && road.counts.iter().any(|v| *v > 0.0);
    if road.tunnel || (matches!(road.access, 2 | 4) && !measured) {
        return ([0.0; 4], 15);
    }
    let alternatives = cross_section(road, candidates);
    let is_prior = road.source_id == 0;
    let mut estimated = road.estimated;
    let mut values = if is_prior {
        let prior = resolve_traffic_default(road.class, road.country);
        [prior.0, prior.1, prior.2, prior.3]
    } else { road.counts };
    let count = alternatives.len();
    let factor = if is_prior {
        estimated = 15;
        let lane_factor = alternatives.iter().map(|candidate|
            lane_ratio(candidate.class as usize, candidate.lanes, candidate.direction != 0))
            .fold(1.0, f64::max);
        // The standalone one-way estimate is a separately resolved directional
        // prior. Two evidenced carriageways instead share one section prior.
        let share = if count > 1 { 1.0 / count as f64 }
            else if road.direction != 0 { 0.5 } else { 1.0 };
        lane_factor * share * access_factor(road.access, road.provenance, road.class)
    } else if road.basis == 1 {
        // Published directional traffic is already on a directional basis,
        // including when the publisher's OSM tag disagrees with the count.
        1.0
    } else {
        // Neither unknown scope nor an estimated physical split is a measured
        // directional count. Keep that uncertainty in every class's status.
        if road.basis == 0 || road.direction != 0 || count > 1 { estimated = 15; }
        let share = if count > 1 { 1.0 / count as f64 }
            else if !road.provenance.is_measured() && road.direction != 0 { 0.5 }
            else { 1.0 };
        share * access_factor(road.access, road.provenance, road.class)
    };
    for value in &mut values { *value *= factor; }
    (values, estimated)
}


/// Cut where the physical alternative set changes, before resolving each child.
pub fn intervals<'a>(road: &'a Road, candidates: &[&'a Road]) -> Vec<(f64, f64, [f64; 4], u8)> {
    let mut cuts = vec![0.0, 1.0];
    let axis = (road.end.0 - road.start.0, road.end.1 - road.start.1);
    if road.basis != 1 && road.basis != 3 {
        let mut laterals = vec![(0.0, 0.0)];
        for candidate in candidates {
            if !compatible_alternative(road, candidate) { continue; }
            cuts.extend([projection(road, candidate.start), projection(road, candidate.end)]);
            laterals.push((lateral_at(road, candidate, 0.0).0, lateral_at(road, candidate, 1.0).0));
        }
        let limit = MAX_CARRIAGEWAY_SEPARATION_M / road.mercator_scale();
        for (i, a) in laterals.iter().enumerate() {
            for b in laterals.iter().skip(i + 1) {
                let initial = a.0 - b.0;
                let change = a.1 - b.1 - initial;
                if change != 0.0 { cuts.extend([(-limit - initial) / change, (limit - initial) / change]); }
            }
        }
    }
    cuts.retain(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0);
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let point = |t: f64| (road.start.0 + axis.0 * t, road.start.1 + axis.1 * t);
    cuts.windows(2).map(|pair| {
        let child = Road { start: point(pair[0]), end: point(pair[1]), ..road.clone() };
        let (counts, estimated) = resolve(&child, candidates.iter().copied());
        (pair[0], pair[1], counts, estimated)
    }).collect()
}
