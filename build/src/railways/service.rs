//! Walk each ordered station leg, orienting shapes, projecting off-track stops, and preserving reversals.
use super::{
    alignment,
    leg_matcher::LegMatcher,
    movements::Stop,
    network::{Network, distance, project},
    route::{self, Visit},
    source_routes::Routes,
};
use crate::osm::geometry::wrapped_longitude_delta;

pub struct Passage {
    pub square: [u16; 2],
    pub way: i64,
    pub segment: i16,
    pub from: f64,
    pub to: f64,
    pub occurrence: i64,
}
pub struct Matched {
    pub passages: Vec<Passage>,
    pub matching: u8,
    pub failed: usize,
}

fn collapse(stops: &[Stop]) -> Vec<Stop> {
    let mut result: Vec<Stop> = Vec::new();
    for stop in stops {
        if result.last().is_none_or(|last| last.id != stop.id) {
            result.push(stop.clone());
        }
    }
    result
}

fn geometry(stops: &[Stop], shape: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let distinct = |mut points: Vec<[f64; 2]>| {
        points.dedup();
        points
    };
    let positions: Vec<[f64; 2]> = stops
        .iter()
        .map(|stop| [stop.latitude, stop.longitude])
        .collect();
    let shape_distinct = distinct(shape.to_vec());
    let mut rev_distinct = shape_distinct.clone();
    rev_distinct.reverse();
    let pos_distinct = distinct(positions);
    if shape.len() < 2 || shape_distinct == pos_distinct || rev_distinct == pos_distinct {
        Vec::new()
    } else {
        shape.to_vec()
    }
}

fn anchors_with_cost(shape: &[[f64; 2]], stops: &[Stop]) -> (Vec<usize>, f64) {
    if shape.is_empty() || stops.is_empty() {
        return (Vec::new(), 0.0);
    }
    let mut costs = vec![0.0; shape.len()];
    let mut predecessors = Vec::new();
    for stop in stops {
        let mut next = Vec::new();
        let mut previous = Vec::new();
        let mut best = 0;
        for (index, point) in shape.iter().enumerate() {
            if costs[index] < costs[best] {
                best = index;
            }
            next.push(costs[best] + distance([stop.latitude, stop.longitude], *point));
            previous.push(best);
        }
        costs = next;
        predecessors.push(previous);
    }
    let mut chosen = 0;
    for index in 1..costs.len() {
        if costs[index] < costs[chosen] {
            chosen = index;
        }
    }
    let best_cost = costs[chosen];
    let mut result = Vec::new();
    for previous in predecessors.iter().rev() {
        result.push(chosen);
        chosen = previous[chosen];
    }
    result.reverse();
    (result, best_cost)
}

fn project_stop_to_anchor(
    shape: &[[f64; 2]],
    anchor: usize,
    stop: [f64; 2],
) -> (usize, f64, [f64; 2]) {
    let foot = |a: [f64; 2], b: [f64; 2], frac: f64| {
        [
            a[0] + frac * (b[0] - a[0]),
            a[1] + frac * wrapped_longitude_delta(a[1], b[1]),
        ]
    };
    let mut best: Option<(f64, usize, f64, [f64; 2])> = None;
    for seg in [
        anchor.checked_sub(1),
        (anchor + 1 < shape.len()).then_some(anchor),
    ]
    .into_iter()
    .flatten()
    {
        let (d, frac) = project(stop, shape[seg], shape[seg + 1]);
        if best.is_none_or(|(bd, _, _, _)| d < bd) {
            best = Some((d, seg, frac, foot(shape[seg], shape[seg + 1], frac)));
        }
    }
    match best {
        Some((_, seg, frac, pt)) => (seg, frac, pt),
        None => (anchor, 0.0, shape[anchor]),
    }
}

fn advancing_legs(shape: &[[f64; 2]], anchors: &[usize], stops: &[Stop]) -> usize {
    (1..stops.len())
        .filter(|&i| {
            let (s0, f0, _) = project_stop_to_anchor(
                shape,
                anchors[i - 1],
                [stops[i - 1].latitude, stops[i - 1].longitude],
            );
            let (s1, f1, _) =
                project_stop_to_anchor(shape, anchors[i], [stops[i].latitude, stops[i].longitude]);
            (s1 as f64 + f1) > (s0 as f64 + f0) + 1e-6
        })
        .count()
}

fn orient_shape(shape: Vec<[f64; 2]>, stops: &[Stop]) -> (Vec<[f64; 2]>, Vec<usize>) {
    if shape.is_empty() {
        return (shape, Vec::new());
    }
    let (fwd_anchors, fwd_cost) = anchors_with_cost(&shape, stops);
    if shape.len() < 2 || stops.len() < 2 {
        return (shape, fwd_anchors);
    }
    let rev_shape: Vec<[f64; 2]> = shape.iter().rev().copied().collect();
    let (rev_anchors, rev_cost) = anchors_with_cost(&rev_shape, stops);
    let rev_wins = rev_cost < fwd_cost - 1e-6
        || ((rev_cost - fwd_cost).abs() <= 1e-6
            && advancing_legs(&rev_shape, &rev_anchors, stops)
                > advancing_legs(&shape, &fwd_anchors, stops));
    if rev_wins {
        (rev_shape, rev_anchors)
    } else {
        (shape, fwd_anchors)
    }
}

#[allow(clippy::too_many_arguments)]
fn project_failed_snap_run(
    network: &Network,
    shape: &[[f64; 2]],
    a0: usize,
    stop0: [f64; 2],
    a1: usize,
    stop1: [f64; 2],
    run: &[[f64; 2]],
    run_idx: usize,
    run_count: usize,
) -> Option<Vec<[f64; 2]>> {
    let snap_start = run_idx == 0 && !route::has_corridor_snap(network, run[0], run);
    let snap_end =
        run_idx + 1 == run_count && !route::has_corridor_snap(network, *run.last()?, run);
    if !snap_start && !snap_end {
        return None;
    }
    let mut adj = Vec::with_capacity(run.len() + 2);
    if snap_start {
        let (seg0, _, pt0) = project_stop_to_anchor(shape, a0, stop0);
        adj.push(pt0);
        if seg0 < a0 {
            adj.push(shape[a0]);
        }
        adj.extend_from_slice(&run[1..]);
    } else {
        adj.extend_from_slice(run);
    }
    if snap_end {
        adj.pop();
        let (seg1, _, pt1) = project_stop_to_anchor(shape, a1, stop1);
        if seg1 >= a1 {
            adj.push(shape[a1]);
        }
        adj.push(pt1);
    }
    adj.dedup();
    (adj.len() >= 2).then_some(adj)
}

fn reversal_runs(points: &[[f64; 2]]) -> Vec<Vec<[f64; 2]>> {
    let mut runs = Vec::new();
    let mut run = vec![points[0]];
    for index in 1..points.len() - 1 {
        let previous = *run.last().unwrap();
        let current = points[index];
        let next = points[index + 1];
        run.push(current);
        let cosine = ((current[0] + next[0]) / 2.0 * std::f64::consts::PI / 180.0)
            .cos()
            .max(0.05);
        let ax = (current[1] - previous[1]) * 111_320.0 * cosine;
        let ay = (current[0] - previous[0]) * 110_540.0;
        let bx = (next[1] - current[1]) * 111_320.0 * cosine;
        let by = (next[0] - current[0]) * 110_540.0;
        let spans = ax.hypot(ay) * bx.hypot(by);
        if spans > 0.0 && (ax * bx + ay * by) / spans <= -0.5 {
            runs.push(run);
            run = vec![current];
        }
    }
    run.push(*points.last().unwrap());
    runs.push(run);
    runs
}

pub fn match_service(
    routes: &Routes,
    matcher: &mut LegMatcher<'_>,
    stops: &[Stop],
    shape: &[[f64; 2]],
) -> Matched {
    let network = matcher.network;
    let stations = collapse(stops);
    let (shape, anchors) = orient_shape(geometry(stops, shape), &stations);
    let observations = if shape.is_empty() {
        stations
            .iter()
            .map(|stop| [stop.latitude, stop.longitude])
            .collect()
    } else {
        shape.clone()
    };
    if let Some(route) = routes.unique(stops)
        && let Some(passages) = alignment::align(&observations, route)
    {
        let mut clipped = Vec::new();
        for passage in passages {
            let lower = passage.from.min(passage.to);
            let upper = passage.from.max(passage.to);
            if let Some(pieces) = network.way_pieces.get(&passage.way) {
                let mut visits = Vec::new();
                for edge in pieces {
                    let from = lower.max(edge.extent[0]);
                    let to = upper.min(edge.extent[1]);
                    if to > from {
                        visits.push(Passage {
                            square: edge.square,
                            way: edge.way,
                            segment: edge.segment,
                            from: if passage.from < passage.to { from } else { to },
                            to: if passage.from < passage.to { to } else { from },
                            occurrence: passage.occurrence,
                        });
                    }
                }
                visits.sort_by(|a, b| a.from.min(a.to).total_cmp(&b.from.min(b.to)));
                if passage.from > passage.to {
                    visits.reverse();
                }
                clipped.extend(visits);
            }
        }
        return Matched {
            passages: clipped,
            matching: 1,
            failed: 0,
        };
    }
    let mut visits = Vec::<Visit>::new();
    let mut failed = 0;
    for index in 1..stations.len() {
        let stop0 = [stations[index - 1].latitude, stations[index - 1].longitude];
        let stop1 = [stations[index].latitude, stations[index].longitude];
        let mut points = vec![stop0];
        if !shape.is_empty() {
            points.extend_from_slice(
                &shape[(anchors[index - 1] + 1).min(anchors[index])..anchors[index]],
            );
        }
        points.push(stop1);
        let runs = reversal_runs(&points);
        let run_count = runs.len();
        for (run_idx, run) in runs.into_iter().enumerate() {
            let walked = match matcher.walk(
                run[0],
                *run.last().unwrap(),
                if shape.is_empty() { &[] } else { &run },
            ) {
                Ok(next) => Ok(next),
                Err("snap") if !shape.is_empty() => {
                    if let Some(adj) = project_failed_snap_run(
                        network,
                        &shape,
                        anchors[index - 1],
                        stop0,
                        anchors[index],
                        stop1,
                        &run,
                        run_idx,
                        run_count,
                    ) {
                        matcher.walk(adj[0], *adj.last().unwrap(), &adj)
                    } else {
                        Err("snap")
                    }
                }
                Err(err) => Err(err),
            };
            match walked {
                Ok(next) => route::stitch(&mut visits, next),
                Err(_) => failed += 1,
            }
        }
    }
    let passages = visits
        .iter()
        .enumerate()
        .filter_map(|(occurrence, visit)| {
            let edge = &network.edges[visit.edge];
            if edge.traversal_only {
                return None;
            }
            let from = edge.extent[0] + visit.from * (edge.extent[1] - edge.extent[0]);
            let to = edge.extent[0] + visit.to * (edge.extent[1] - edge.extent[0]);
            (from != to).then_some(Passage {
                square: edge.square,
                way: edge.way,
                segment: edge.segment,
                from,
                to,
                occurrence: occurrence as i64,
            })
        })
        .collect();
    Matched {
        passages,
        matching: 2,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::railways::network::Edge;

    fn mk_stop(id: &str, pt: [f64; 2]) -> Stop {
        Stop {
            source_id: String::new(),
            id: id.into(),
            latitude: pt[0],
            longitude: pt[1],
        }
    }

    fn straight_net(nodes: &[[f64; 2]]) -> Network {
        let mut net = Network::default();
        net.nodes = nodes.to_vec();
        let mut extent = 0.0;
        for i in 1..nodes.len() {
            let len = distance(nodes[i - 1], nodes[i]);
            net.edges.push(Edge {
                nodes: [i - 1, i],
                points: [nodes[i - 1], nodes[i]],
                length: len,
                way: 1,
                segment: (i - 1) as i16,
                square: [100, 100],
                extent: [extent, extent + len],
                rail_type: 0,
                usage: 0,
                traffic_mode: 0,
                country: *b"US",
                corridor: "main".into(),
                corridor_name: String::new(),
                traversal_only: false,
            });
            extent += len;
        }
        net.index();
        net
    }

    #[test]
    fn station_stub_checks_both_orders_and_reversal_retains_return_leg() {
        let stops = [mk_stop("a", [0.0, 0.0]), mk_stop("b", [0.0, 0.01])];
        assert!(geometry(&stops, &[[0.0, 0.0], [0.0, 0.01]]).is_empty());
        assert!(geometry(&stops, &[[0.0, 0.01], [0.0, 0.0]]).is_empty());
        assert_eq!(
            reversal_runs(&[[0.0, 0.0], [0.0, 0.01], [0.0, 0.0]]),
            vec![vec![[0.0, 0.0], [0.0, 0.01]], vec![[0.0, 0.01], [0.0, 0.0]]]
        );
    }

    #[test]
    fn reversed_shape_orients_to_stops_and_walks_curving_track() {
        // Track bends 556 m north of the A-C chord so both edge midpoints lie 278 m off the chord
        // (> 250 m corridor gate); with only terminal stops C -> A, geometry() retains the shape,
        // unoriented slicing collapses to the chord and fails, and orient_shape walks both edges.
        let pts = [[0.0, 0.0], [0.005, 0.005], [0.0, 0.010]];
        let net = straight_net(&pts);
        let mut matcher = LegMatcher::new(&net);
        let stops = [mk_stop("C", pts[2]), mk_stop("A", pts[0])];
        assert!(!geometry(&stops, &pts).is_empty());
        let matched = match_service(&Routes::default(), &mut matcher, &stops, &pts);
        assert_eq!(matched.failed, 0);
        assert_eq!(matched.passages.len(), 2);
        // Equal-cost tie on a 2-vertex segment picks reversed order via advancing_legs.
        let seg = vec![[0.0, 0.0], [0.009, 0.0]];
        let tie_stops = [mk_stop("1", [0.0036, 0.0]), mk_stop("2", [0.0018, 0.0])];
        let (oriented, _) = orient_shape(seg, &tie_stops);
        assert_eq!(oriented[0], [0.009, 0.0]);
    }

    #[test]
    fn off_track_stop_projects_onto_oriented_shape_only_when_snap_fails() {
        let pts = [[0.0, 0.0], [0.0, 0.004], [0.0, 0.008]];
        let net = straight_net(&pts);
        let mut matcher = LegMatcher::new(&net);
        // First stop lies 450 m north of track (> 300 m snap gate), second stop is on track.
        let stops = [mk_stop("off", [0.0041, 0.0005]), mk_stop("on", pts[2])];
        let matched = match_service(&Routes::default(), &mut matcher, &stops, &pts);
        assert_eq!(matched.failed, 0);
        assert_eq!(matched.passages.len(), 2);
    }
}
