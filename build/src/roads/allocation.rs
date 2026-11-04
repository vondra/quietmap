//! Allocate one physical cross-section once; longitudinal pieces retain through-flow.

use super::{
    input::Road,
    model::{Model, TrafficDefault},
};

pub(super) const MAX_CARRIAGEWAY_SEPARATION_M: f64 = 50.0;
const MIN_PARALLEL_COSINE: f64 = 0.9659258262890683; // cos(15 degrees)

pub(crate) fn compatible_alternative(road: &Road, candidate: &Road) -> bool {
    if road.way_id == candidate.way_id
        || road.direction == 0
        || candidate.direction == 0
        || road.roundabout
        || candidate.roundabout
        || road.class != candidate.class
        || road.country != candidate.country
        || !((!road.corridor.is_empty() && road.corridor == candidate.corridor)
            || (!road.observation.is_empty()
                && road.observation_source_id == candidate.observation_source_id
                && road.observation == candidate.observation))
    {
        return false;
    }
    let a = road.travel_vector();
    let b = candidate.travel_vector();
    let length_a = a.0.hypot(a.1);
    let length_b = b.0.hypot(b.1);
    if length_a == 0.0
        || length_b == 0.0
        || (a.0 * b.0 + a.1 * b.1).abs() / (length_a * length_b) < MIN_PARALLEL_COSINE
    {
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
    let other = (
        candidate.end.0 - candidate.start.0,
        candidate.end.1 - candidate.start.1,
    );
    let point = (
        road.start.0 + position * axis.0,
        road.start.1 + position * axis.1,
    );
    let along = ((point.0 - candidate.start.0) * axis.0 + (point.1 - candidate.start.1) * axis.1)
        / (other.0 * axis.0 + other.1 * axis.1);
    let delta = (
        candidate.start.0 + along * other.0 - point.0,
        candidate.start.1 + along * other.1 - point.1,
    );
    (
        (delta.0 * -axis.1 + delta.1 * axis.0) / axis.0.hypot(axis.1),
        along,
    )
}

fn cross_section<'a>(
    road: &'a Road,
    candidates: impl IntoIterator<Item = &'a Road>,
) -> Vec<&'a Road> {
    let mut positions = vec![(0.0, road)];
    for candidate in candidates {
        if !compatible_alternative(road, candidate) {
            continue;
        }
        let (lateral, along) = lateral_at(road, candidate, 0.5);
        if (0.0..1.0).contains(&along) {
            positions.push((lateral, candidate));
        }
    }
    positions.sort_by(|a, b| a.0.total_cmp(&b.0));
    let center = positions
        .iter()
        .position(|(_, candidate)| std::ptr::eq(*candidate, road))
        .unwrap();
    let reach = MAX_CARRIAGEWAY_SEPARATION_M / road.mercator_scale();
    let mut left = center;
    let mut right = center;
    while left > 0 && positions[left].0 - positions[left - 1].0 <= reach {
        left -= 1;
    }
    while right + 1 < positions.len() && positions[right + 1].0 - positions[right].0 <= reach {
        right += 1;
    }
    let mut ways = std::collections::BTreeMap::new();
    for (_, candidate) in &positions[left..=right] {
        ways.entry(candidate.way_id).or_insert(*candidate);
    }
    ways.into_values().collect()
}

/// One stored carriageway's traffic and class status, and the whole road's four-class total when this row
/// alone establishes it: a two-way row or a ring, a known share of a two-way total, a street's own flow.
/// `whole` is None where only this direction is known or other carriageways carry the rest.
struct Carriageway {
    counts: [f64; 4],
    estimated: u8,
    whole: Option<f64>,
}

fn carriageway(road: &Road, members: &[&Road], model: &Model) -> Carriageway {
    let total = |counts: [f64; 4]| counts.iter().sum::<f64>();
    if road.basis == 3 {
        return Carriageway {
            counts: road.counts,
            estimated: road.estimated,
            whole: Some(total(road.counts)),
        };
    }
    let measured = road.count_origin_measured && road.counts.iter().any(|v| *v > 0.0);
    if road.tunnel || (matches!(road.access, 2 | 4) && !measured) {
        return Carriageway {
            counts: [0.0; 4],
            estimated: 15,
            whole: Some(0.0),
        };
    }
    let access = model.access_factor(road);
    let one_way = road.direction != 0 && !road.roundabout;
    if road.source_id == 0 {
        let scale = |prior: [f64; 4], factor: f64| prior.map(|value| value * factor);
        return match model.default_traffic(road) {
            TrafficDefault::Carriageway(prior) => {
                let counts = scale(prior, access);
                Carriageway {
                    counts,
                    estimated: 15,
                    whole: (!one_way).then(|| total(counts)),
                }
            }
            TrafficDefault::SectionBothDirections(prior) => {
                let lane_factor = members
                    .iter()
                    .map(|candidate| model.lane_factor(candidate))
                    .fold(1.0, f64::max);
                let share = if members.len() > 1 {
                    1.0 / members.len() as f64
                } else if one_way {
                    0.5
                } else {
                    1.0
                };
                Carriageway {
                    counts: scale(prior, lane_factor * share * access),
                    estimated: 15,
                    whole: (members.len() == 1).then(|| total(scale(prior, lane_factor * access))),
                }
            }
        };
    }
    if road.basis == 1 {
        return Carriageway {
            counts: road.counts,
            estimated: road.estimated,
            whole: None,
        };
    }
    let count = members.len();
    let estimated = if road.basis == 0 || count > 1 || (one_way && road.basis != 4) {
        15
    } else {
        road.estimated
    };
    // Retain the measured legacy rule: a lone local one-way street can carry its entire count.
    let lone_direction_of_a_two_way_total = one_way
        && road.basis != 4
        && (!road.count_origin_measured
            || (road.basis == 2 && road.class <= 2 && !road.corridor.is_empty()));
    let share = if count > 1 {
        1.0 / count as f64
    } else if lone_direction_of_a_two_way_total {
        0.5
    } else {
        1.0
    };
    Carriageway {
        counts: road.counts.map(|value| value * share * access),
        estimated,
        whole: (count == 1).then(|| total(road.counts) * access),
    }
}

/// Four carriageway classes, their status and the whole section's known total; zero when only one direction is known.
pub(crate) fn allocate<'a>(
    road: &'a Road,
    candidates: &[&'a Road],
    model: &Model,
) -> ([f64; 4], u8, f64) {
    let members = cross_section(road, candidates.iter().copied());
    let own = carriageway(road, &members, model);
    let whole = own.whole.unwrap_or_else(|| {
        if members.len() < 2 {
            return 0.0;
        }
        members
            .iter()
            .map(|member| {
                if std::ptr::eq(*member, road) {
                    own.counts
                } else {
                    carriageway(member, &members, model).counts
                }
            })
            .map(|counts| counts.iter().sum::<f64>())
            .sum()
    });
    (own.counts, own.estimated, whole)
}

/// Cut where the physical alternative set changes, before resolving each child.
pub fn intervals<'a>(
    road: &'a Road,
    candidates: &[&'a Road],
    model: &Model,
) -> Vec<(f64, f64, [f64; 4], u8, f64)> {
    let mut cuts = vec![0.0, 1.0];
    let axis = (road.end.0 - road.start.0, road.end.1 - road.start.1);
    if road.basis != 3 {
        let mut laterals = vec![(0.0, 0.0)];
        for candidate in candidates {
            if !compatible_alternative(road, candidate) {
                continue;
            }
            cuts.extend([
                projection(road, candidate.start),
                projection(road, candidate.end),
            ]);
            laterals.push((
                lateral_at(road, candidate, 0.0).0,
                lateral_at(road, candidate, 1.0).0,
            ));
        }
        let limit = MAX_CARRIAGEWAY_SEPARATION_M / road.mercator_scale();
        for (i, a) in laterals.iter().enumerate() {
            for b in laterals.iter().skip(i + 1) {
                let initial = a.0 - b.0;
                let change = a.1 - b.1 - initial;
                if change != 0.0 {
                    cuts.extend([(-limit - initial) / change, (limit - initial) / change]);
                }
            }
        }
    }
    cuts.retain(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0);
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let point = |t: f64| (road.start.0 + axis.0 * t, road.start.1 + axis.1 * t);
    let mut children: Vec<(f64, f64, [f64; 4], u8, f64)> = Vec::new();
    for pair in cuts.windows(2) {
        let child = Road {
            start: point(pair[0]),
            end: point(pair[1]),
            ..road.clone()
        };
        let (counts, estimated, whole) = allocate(&child, candidates, model);
        match children.last_mut() {
            Some(previous)
                if previous.2 == counts && previous.3 == estimated && previous.4 == whole =>
            {
                previous.1 = pair[1]
            }
            _ => children.push((pair[0], pair[1], counts, estimated, whole)),
        }
    }
    merge_children_shorter_than_a_metre(
        &mut children,
        axis.0.hypot(axis.1) * road.mercator_scale(),
    );
    children
}

const MIN_CHILD_LENGTH_M: f64 = 1.0;

/// The shortest sliver first joins its longer neighbour and takes that neighbour's allocation,
/// until no child is shorter than a metre unless the parent itself is.
fn merge_children_shorter_than_a_metre(
    children: &mut Vec<(f64, f64, [f64; 4], u8, f64)>,
    parent_length_m: f64,
) {
    let min_fraction = MIN_CHILD_LENGTH_M / parent_length_m;
    let fraction = |child: &(f64, f64, [f64; 4], u8, f64)| child.1 - child.0;
    while children.len() > 1 {
        let Some(sliver) = (0..children.len())
            .filter(|i| fraction(&children[*i]) < min_fraction)
            .min_by(|a, b| fraction(&children[*a]).total_cmp(&fraction(&children[*b])))
        else {
            break;
        };
        let removed = children.remove(sliver);
        let after_is_longer = sliver < children.len()
            && (sliver == 0 || fraction(&children[sliver]) > fraction(&children[sliver - 1]));
        if after_is_longer {
            children[sliver].0 = removed.0;
        } else {
            children[sliver - 1].1 = removed.1;
        }
        if sliver > 0
            && sliver < children.len()
            && (
                children[sliver - 1].2,
                children[sliver - 1].3,
                children[sliver - 1].4,
            ) == (children[sliver].2, children[sliver].3, children[sliver].4)
        {
            children[sliver - 1].1 = children.remove(sliver).1;
        }
    }
}
