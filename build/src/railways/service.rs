//! Walk each ordered station leg, preserving reversals and quarantining unresolved observations.
use super::{
    alignment,
    leg_matcher::LegMatcher,
    movements::Stop,
    network::distance,
    route::{self, Visit},
    source_routes::Routes,
};

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
    let distinct = |points: Vec<[f64; 2]>| {
        let mut points = points;
        points.dedup();
        points
    };
    let positions = stops
        .iter()
        .map(|stop| [stop.latitude, stop.longitude])
        .collect();
    if shape.len() < 2 || distinct(shape.to_vec()) == distinct(positions) {
        Vec::new()
    } else {
        shape.to_vec()
    }
}

fn anchors(shape: &[[f64; 2]], stops: &[Stop]) -> Vec<usize> {
    if shape.is_empty() || stops.is_empty() {
        return Vec::new();
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
    let mut result = Vec::new();
    for previous in predecessors.iter().rev() {
        result.push(chosen);
        chosen = previous[chosen];
    }
    result.reverse();
    result
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
    let shape = geometry(stops, shape);
    let stations = collapse(stops);
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
    let anchors = anchors(&shape, &stations);
    let mut visits = Vec::<Visit>::new();
    let mut failed = 0;
    for index in 1..stations.len() {
        let mut points = vec![[stations[index - 1].latitude, stations[index - 1].longitude]];
        if !shape.is_empty() {
            points.extend_from_slice(
                &shape[(anchors[index - 1] + 1).min(anchors[index])..anchors[index]],
            );
        }
        points.push([stations[index].latitude, stations[index].longitude]);
        for run in reversal_runs(&points) {
            match matcher.walk(
                run[0],
                *run.last().unwrap(),
                if shape.is_empty() { &[] } else { &run },
            ) {
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
    #[test]
    fn station_stub_keeps_ambiguity_and_reversal_retains_the_return_leg() {
        let stops = [[0.0, 0.0], [0.0, 0.01]].map(|point| Stop {
            source_id: String::new(),
            id: point[1].to_string(),
            latitude: point[0],
            longitude: point[1],
        });
        assert!(geometry(&stops, &[[0.0, 0.0], [0.0, 0.01]]).is_empty());
        assert_eq!(
            reversal_runs(&[[0.0, 0.0], [0.0, 0.01], [0.0, 0.0]]),
            vec![vec![[0.0, 0.0], [0.0, 0.01]], vec![[0.0, 0.01], [0.0, 0.0]]]
        );
    }
}
