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

fn collect_snaps(
    network: &Network,
    point: [f64; 2],
    allowed: &impl Fn(&Edge) -> bool,
) -> Vec<Snap> {
    let mut out = Vec::new();
    for index in network.nearby(point, 300.0) {
        let edge = &network.edges[index];
        if !allowed(edge) {
            continue;
        }
        let (distance, fraction) = project(point, edge.points[0], edge.points[1]);
        if distance <= 300.0 {
            out.push(Snap {
                edge: index,
                fraction,
                distance,
            });
        }
    }
    out.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.edge.cmp(&b.edge)));
    out
}

pub(super) fn has_corridor_snap(network: &Network, point: [f64; 2], shape: &[[f64; 2]]) -> bool {
    let corridor =
        |edge: &Edge| shape.is_empty() || distance_to_line(edge.midpoint(), shape) <= 250.0;
    network.nearby(point, 300.0).into_iter().any(|index| {
        let edge = &network.edges[index];
        corridor(edge) && project(point, edge.points[0], edge.points[1]).0 <= 300.0
    })
}

fn corridor_fallback_pair(
    network: &Network,
    from: [f64; 2],
    to: [f64; 2],
    corridor: &impl Fn(&Edge) -> bool,
) -> Option<(Snap, Snap)> {
    let starts = collect_snaps(network, from, corridor);
    let ends = collect_snaps(network, to, corridor);
    if starts.is_empty() || ends.is_empty() {
        return None;
    }
    let mut best: Option<(Snap, Snap, f64)> = None;
    let consider = |best: &mut Option<(Snap, Snap, f64)>, s: Snap, e: Snap| {
        let sum = s.distance + e.distance;
        let better = best.is_none_or(|(bs, be, bd)| {
            sum.total_cmp(&bd)
                .then(s.edge.cmp(&bs.edge))
                .then(e.edge.cmp(&be.edge))
                .is_lt()
        });
        if better {
            *best = Some((s, e, sum));
        }
    };
    let end_by_edge: HashMap<usize, Snap> = ends.iter().map(|&e| (e.edge, e)).collect();
    for &s in &starts {
        if let Some(&e) = end_by_edge.get(&s.edge)
            && (s.fraction - e.fraction).abs() > 1e-12
        {
            consider(&mut best, s, e);
        }
    }
    let mut end_by_node: HashMap<usize, Vec<Snap>> = HashMap::new();
    for &e in &ends {
        let nodes = network.edges[e.edge].nodes;
        end_by_node.entry(nodes[0]).or_default().push(e);
        if nodes[1] != nodes[0] {
            end_by_node.entry(nodes[1]).or_default().push(e);
        }
    }
    let mut families = Vec::new();
    for &s in &starts {
        let fam = network.edges[s.edge].family();
        if !families.contains(&fam) {
            families.push(fam);
        }
    }
    for fam in families {
        let allowed = |edge: &Edge| {
            (edge.traversal_only || fam == 0 || edge.family() == fam) && corridor(edge)
        };
        let mut seen: HashSet<usize> = HashSet::new();
        for &s in &starts {
            if network.edges[s.edge].family() != fam {
                continue;
            }
            if best.is_some_and(|(_, _, bd)| s.distance > bd) {
                break;
            }
            let se = &network.edges[s.edge];
            if seen.contains(&se.nodes[0]) && seen.contains(&se.nodes[1]) {
                continue;
            }
            let mut stack = Vec::new();
            for node in se.nodes {
                if seen.insert(node) {
                    stack.push(node);
                }
            }
            let mut best_end: Option<Snap> = None;
            while let Some(u) = stack.pop() {
                if let Some(candidates) = end_by_node.get(&u) {
                    for &e in candidates {
                        if e.edge != s.edge
                            && best_end.is_none_or(|be| {
                                e.distance
                                    .total_cmp(&be.distance)
                                    .then(e.edge.cmp(&be.edge))
                                    .is_lt()
                            })
                        {
                            best_end = Some(e);
                        }
                    }
                }
                for &edge_idx in &network.adjacency[u] {
                    let edge = &network.edges[edge_idx];
                    if !allowed(edge) {
                        continue;
                    }
                    let v = if edge.nodes[0] == u {
                        edge.nodes[1]
                    } else {
                        edge.nodes[0]
                    };
                    if seen.insert(v) {
                        stack.push(v);
                    }
                }
            }
            if let Some(e) = best_end {
                consider(&mut best, s, e);
            }
        }
    }
    best.map(|(s, e, _)| (s, e))
}

fn shortest_between(
    network: &Network,
    search: &mut Search,
    start: Snap,
    end: Snap,
    shape: &[[f64; 2]],
    corridor: &impl Fn(&Edge) -> bool,
) -> Result<Vec<Visit>, &'static str> {
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
    match shortest_between(network, search, start, end, shape, &corridor) {
        Ok(visits) => Ok(visits),
        Err("disconnected") if !shape.is_empty() => {
            let Some((s, e)) = corridor_fallback_pair(network, from, to, &corridor) else {
                return Err("disconnected");
            };
            shortest_between(network, search, s, e, shape, &corridor)
        }
        Err(err) => Err(err),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn push_edge(net: &mut Network, a: usize, b: usize, way: i64, rail_type: u8) {
        let len = distance(net.nodes[a], net.nodes[b]);
        net.edges.push(Edge {
            nodes: [a, b],
            points: [net.nodes[a], net.nodes[b]],
            length: len,
            way,
            segment: 0,
            square: [100, 100],
            extent: [0.0, len],
            rail_type,
            usage: 0,
            traffic_mode: 0,
            country: *b"US",
            corridor: "main".into(),
            corridor_name: String::new(),
            traversal_only: false,
        });
    }

    #[test]
    fn corridor_fallback_connects_parallel_tracks_with_distant_crossover_and_isolated_families() {
        // Two parallel heavy-rail tracks 10 m apart running east from x=0 to x=0.02 (~2.2 km),
        // linked only by a crossover at x=-0.005 (550 m west, outside the 250 m leg corridor).
        // A family-0 track (rail_type=5) sits even closer to `from` and must not poison family-1.
        let mut net = Network::default();
        net.nodes = vec![
            [0.0, -0.005],      // 0: west crossover north
            [0.0001, -0.005],   // 1: west crossover south
            [0.0, 0.0],         // 2: track 1 start
            [0.0, 0.02],        // 3: track 1 end
            [0.0001, 0.0],      // 4: track 2 start
            [0.0001, 0.02],     // 5: track 2 end
            [-0.00001, 0.0],    // 6: family-0 stub start
            [-0.00001, 0.0005], // 7: family-0 stub end
        ];
        push_edge(&mut net, 0, 1, 1, 0);
        push_edge(&mut net, 0, 2, 2, 0);
        push_edge(&mut net, 1, 4, 3, 0);
        push_edge(&mut net, 2, 3, 10, 0); // Track 1
        push_edge(&mut net, 4, 5, 20, 0); // Track 2
        push_edge(&mut net, 6, 7, 30, 5); // family 0 stub
        net.index();

        // Start pin sits 2 m south of Track 1 (and 1 m from family-0 stub); end pin sits 2 m
        // north of Track 2. Primary single-snap lands on Track 1 -> Track 2 (disconnected inside
        // the 250 m corridor), while corridor_fallback_pair connects along Track 1 or Track 2.
        let from = [0.00002, 0.0];
        let to = [0.00008, 0.02];
        let shape = [[0.00005, 0.0], [0.00005, 0.02]];
        let mut search = Search::new(net.nodes.len());
        let visits = walk(&net, &mut search, from, to, &shape).expect("parallel track fallback");
        assert_eq!(visits.len(), 1);
        assert!(matches!(net.edges[visits[0].edge].way, 10 | 20));
    }
}
