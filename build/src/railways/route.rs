//! Match ordered service observations to connected tracks, retaining every directed repeated passage.
use super::{
    network::{Edge, Network, distance, distance_to_line, project},
    search::{Path, Search},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub struct Visit {
    pub edge: usize,
    pub to_node: usize,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Copy)]
struct Snap {
    edge: usize,
    fraction: f64,
    distance: f64,
}

fn snaps(
    network: &Network,
    point: [f64; 2],
    allowed: &impl Fn(&Edge) -> bool,
) -> Vec<(usize, Snap)> {
    let mut components = Vec::<(usize, Snap)>::new();
    let mut indices = HashMap::new();
    for index in network.nearby(point, 300.0) {
        let edge = &network.edges[index];
        if !allowed(edge) {
            continue;
        }
        let (distance, fraction) = project(point, edge.points[0], edge.points[1]);
        if distance > 300.0 {
            continue;
        }
        let component = network.components[edge.nodes[0]];
        let position = *indices.entry(component).or_insert_with(|| {
            let at = components.len();
            components.push((
                component,
                Snap {
                    edge: index,
                    fraction,
                    distance: f64::INFINITY,
                },
            ));
            at
        });
        if distance < components[position].1.distance {
            components[position].1 = Snap {
                edge: index,
                fraction,
                distance,
            };
        }
    }
    components
}

fn visit(network: &Network, edge: usize, from: f64, to: f64) -> Visit {
    let nodes = network.edges[edge].nodes;
    Visit {
        edge,
        to_node: nodes[usize::from(from < to)],
        from,
        to,
    }
}

pub fn walk(
    network: &Network,
    search: &mut Search,
    from: [f64; 2],
    to: [f64; 2],
    shape: &[[f64; 2]],
) -> Result<Vec<Visit>, &'static str> {
    let corridor =
        |edge: &Edge| shape.is_empty() || distance_to_line(edge.midpoint(), shape) <= 250.0;
    let starts = snaps(network, from, &corridor);
    let ends = snaps(network, to, &corridor);
    if starts.is_empty() || ends.is_empty() {
        return Err("snap");
    }
    let mut pair: Option<(Snap, Snap, f64)> = None;
    for (component, start) in starts {
        if let Some((_, end)) = ends.iter().find(|(candidate, _)| *candidate == component) {
            let distance = start.distance + end.distance;
            if pair.is_none_or(|(_, _, old)| distance < old) {
                pair = Some((start, *end, distance));
            }
        }
    }
    let Some((start, end, _)) = pair else {
        return Err("disconnected");
    };
    if start.edge == end.edge {
        return Ok(if start.fraction == end.fraction {
            Vec::new()
        } else {
            vec![visit(network, start.edge, start.fraction, end.fraction)]
        });
    }
    let first = &network.edges[start.edge];
    let last = &network.edges[end.edge];
    let family = first.family();
    let allowed = |edge: &Edge| {
        (edge.traversal_only || family == 0 || edge.family() == family) && corridor(edge)
    };
    let approach = |edge: &Edge, fraction: f64, node: usize| {
        (fraction - if node == edge.nodes[0] { 0.0 } else { 1.0 }).abs() * edge.length
    };
    let mut combinations = Vec::new();
    for u in first.nodes {
        for v in last.nodes {
            combinations.push((
                u,
                v,
                approach(first, start.fraction, u)
                    + approach(last, end.fraction, v)
                    + distance(network.nodes[u], network.nodes[v]),
            ));
        }
    }
    combinations.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut winner: Option<(usize, usize, Path)> = None;
    let mut best = f64::INFINITY;
    for (u, v, lower_bound) in combinations {
        if lower_bound >= best {
            break;
        }
        if let Some(path) = search.shortest(network, u, v, &allowed, &HashSet::new()) {
            let length =
                approach(first, start.fraction, u) + path.length + approach(last, end.fraction, v);
            if length < best {
                best = length;
                winner = Some((u, v, path));
            }
        }
    }
    let Some((u, v, path)) = winner else {
        return Err("disconnected");
    };
    if shape.is_empty() && search.ambiguous(network, u, v, &path, &allowed) {
        return Err("ambiguous");
    }
    let mut visits = Vec::new();
    let mut node = u;
    for &index in &path.edges {
        let edge = &network.edges[index];
        let forward = edge.nodes[0] == node;
        visits.push(visit(
            network,
            index,
            if forward { 0.0 } else { 1.0 },
            if forward { 1.0 } else { 0.0 },
        ));
        node = if forward {
            edge.nodes[1]
        } else {
            edge.nodes[0]
        };
    }
    let from_parameter = if u == first.nodes[0] { 0.0 } else { 1.0 };
    if visits.first().is_some_and(|row| row.edge == start.edge) {
        visits[0].from = start.fraction;
    } else {
        visits.insert(
            0,
            Visit {
                edge: start.edge,
                to_node: u,
                from: start.fraction,
                to: from_parameter,
            },
        );
    }
    let to_parameter = if v == last.nodes[0] { 0.0 } else { 1.0 };
    if visits.last().is_some_and(|row| row.edge == end.edge) {
        visits.last_mut().unwrap().to = end.fraction;
    } else {
        visits.push(Visit {
            edge: end.edge,
            to_node: if v == last.nodes[0] {
                last.nodes[1]
            } else {
                last.nodes[0]
            },
            from: to_parameter,
            to: end.fraction,
        });
    }
    Ok(visits)
}

pub fn stitch(visits: &mut Vec<Visit>, mut next: Vec<Visit>) {
    if let (Some(previous), Some(first)) = (visits.last_mut(), next.first())
        && previous.edge == first.edge
        && previous.to_node == first.to_node
    {
        previous.to = first.to;
        next.remove(0);
    }
    visits.extend(next);
}
