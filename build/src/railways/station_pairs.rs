//! Resolve station-pair observations by gauge, and keep failed corridors out of timetable-silent priors.
use super::{
    movements::{Location, Movement},
    network::{Edge, Network, distance, project},
    search::{Path, Search},
};
use std::collections::{HashMap, HashSet};

pub fn add_counts(to: &mut Movement, from: &Movement) {
    for (old, new) in [
        (&mut to.passenger, from.passenger),
        (&mut to.freight, from.freight),
    ] {
        if let Some(value) = new {
            *old = Some(old.unwrap_or(0.0) + value);
        }
    }
    for (estimate, addition) in to
        .missing_calendar_estimate
        .iter_mut()
        .zip(from.missing_calendar_estimate)
    {
        *estimate += addition;
    }
    to.passenger_status = to.passenger_status.max(from.passenger_status);
    to.freight_status = to.freight_status.max(from.freight_status);
}

fn great_circle(a: [f64; 2], b: [f64; 2]) -> f64 {
    let radians = std::f64::consts::PI / 180.0;
    let dlat = (b[0] - a[0]) * radians;
    let dlon = crate::osm::geometry::wrapped_longitude_delta(a[1], b[1]) * radians;
    let h = (dlat / 2.0).sin().powi(2)
        + (a[0] * radians).cos() * (b[0] * radians).cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().min(1.0).asin()
}

fn quarantine_gap(
    network: &Network,
    search: &mut Search,
    a: [f64; 2],
    b: [f64; 2],
    nodes: [Option<usize>; 2],
    bound: f64,
    quarantine: &mut HashSet<usize>,
) {
    let radius = bound.min(5000.0);
    let dy = radius / 110_540.0;
    let dx = radius
        / (111_320.0
            * ((a[0] + b[0]) / 2.0 * std::f64::consts::PI / 180.0)
                .cos()
                .max(0.05));
    for (index, edge) in network.edges.iter().enumerate() {
        if edge.traversal_only {
            continue;
        }
        let point = edge.midpoint();
        if point[0] < a[0].min(b[0]) - dy
            || point[0] > a[0].max(b[0]) + dy
            || point[1] < a[1].min(b[1]) - dx
            || point[1] > a[1].max(b[1]) + dx
        {
            continue;
        }
        if project(point, a, b).0 <= radius {
            quarantine.insert(index);
        }
    }
    for (node, other) in [(nodes[0], b), (nodes[1], a)] {
        if let Some(node) = node {
            for (node, length) in search.flood(network, node, bound.min(50_000.0)) {
                if length + distance(network.nodes[node], other) > bound {
                    continue;
                }
                for &edge in &network.adjacency[node] {
                    if !network.edges[edge].traversal_only {
                        quarantine.insert(edge);
                    }
                }
            }
        }
    }
}

fn quarantine_alternatives(
    network: &Network,
    search: &mut Search,
    nodes: [usize; 2],
    best: &Path,
    alternative: &Path,
    allowed: &impl Fn(&Edge) -> bool,
    quarantine: &mut HashSet<usize>,
) {
    let mut used: HashSet<_> = best
        .edges
        .iter()
        .chain(&alternative.edges)
        .copied()
        .collect();
    let mut penalty = 3.0;
    for _ in 0..6 {
        let Some(next) = search.penalized(network, nodes[0], nodes[1], allowed, &used, penalty)
        else {
            break;
        };
        if next.length <= 1.2 * best.length {
            let fresh = next.edges.iter().any(|edge| !used.contains(edge));
            used.extend(next.edges);
            if fresh {
                continue;
            }
        }
        if penalty > 27.0 {
            break;
        }
        penalty *= 3.0;
    }
    quarantine.extend(
        used.into_iter()
            .filter(|edge| !network.edges[*edge].traversal_only),
    );
}

pub struct MatchedPairs {
    pub edges: HashMap<usize, Movement>,
    pub quarantine: HashSet<usize>,
}

pub fn match_pairs(network: &Network, movements: &[Movement]) -> MatchedPairs {
    let mut pairs = Vec::<Movement>::new();
    let mut indices = HashMap::new();
    for movement in movements {
        let Location::StationPair { from, to } = &movement.location else {
            continue;
        };
        let key = [
            from.latitude.to_bits(),
            from.longitude.to_bits(),
            to.latitude.to_bits(),
            to.longitude.to_bits(),
        ];
        if let Some(&index) = indices.get(&key) {
            add_counts(&mut pairs[index], movement);
        } else {
            indices.insert(key, pairs.len());
            pairs.push(movement.clone());
        }
    }
    let mut result = MatchedPairs {
        edges: HashMap::new(),
        quarantine: HashSet::new(),
    };
    let mut search = Search::new(network.nodes.len());
    let mut failed = 0;
    for movement in &pairs {
        let Location::StationPair { from, to } = &movement.location else {
            unreachable!()
        };
        let a = [from.latitude, from.longitude];
        let b = [to.latitude, to.longitude];
        let bound = 2.5 * great_circle(a, b) + 2000.0;
        let mut attempts = Vec::new();
        for family in [1, 2] {
            if let (Some(from), Some(to)) = (
                network.nearest_node(a, family),
                network.nearest_node(b, family),
            ) {
                attempts.push((family, from, to));
            }
        }
        attempts.sort_by(|a, b| (a.1.1 + a.2.1).total_cmp(&(b.1.1 + b.2.1)));
        let mut primary = None;
        let mut walked = None;
        for (family, from, to) in attempts {
            let allowed = |edge: &Edge| edge.traversal_only || edge.family() == family;
            primary.get_or_insert([Some(from.0), Some(to.0)]);
            if network.components[from.0] != network.components[to.0] {
                continue;
            }
            let Some(path) = search.shortest(network, from.0, to.0, &allowed, &HashSet::new())
            else {
                continue;
            };
            if path.length > bound {
                continue;
            }
            walked = Some((family, from.0, to.0, path));
            break;
        }
        let Some((family, from, to, path)) = walked else {
            failed += 1;
            quarantine_gap(
                network,
                &mut search,
                a,
                b,
                primary.unwrap_or_else(|| {
                    [
                        network.nearest_node(a, 0).map(|x| x.0),
                        network.nearest_node(b, 0).map(|x| x.0),
                    ]
                }),
                bound,
                &mut result.quarantine,
            );
            continue;
        };
        let allowed = |edge: &Edge| edge.traversal_only || edge.family() == family;
        if let Some(alternative) = search.alternative(network, from, to, &path, &allowed) {
            failed += 1;
            quarantine_alternatives(
                network,
                &mut search,
                [from, to],
                &path,
                &alternative,
                &allowed,
                &mut result.quarantine,
            );
            continue;
        }
        for edge in path.edges {
            if network.edges[edge].traversal_only {
                continue;
            }
            if let Some(old) = result.edges.get_mut(&edge) {
                add_counts(old, movement);
            } else {
                result.edges.insert(edge, movement.clone());
            }
        }
    }
    eprintln!(
        "railways: {} directed station pairs, {failed} failed, {} claimed edges, {} quarantined",
        pairs.len(),
        result.edges.len(),
        result.quarantine.len()
    );
    result
}
