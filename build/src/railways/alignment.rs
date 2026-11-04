//! Align service shapes to ordered source ways without inventing a shortcut or reversal.
use super::{
    network::{distance, project},
    source_routes::Itinerary,
};
use std::collections::HashMap;

pub struct Passage {
    pub way: i64,
    pub occurrence: i64,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Copy)]
struct Position {
    occurrence: usize,
    local: f64,
    point: [f64; 2],
    error: f64,
}
struct State {
    position: Position,
    cost: f64,
    previous: usize,
}
struct Segment {
    occurrence: usize,
    points: [[f64; 2]; 2],
    range: [f64; 2],
    fractions: [f64; 2],
}
struct Traversal<'a> {
    route: &'a Itinerary,
    entries: Vec<f64>,
    exits: Vec<f64>,
    reversed: Vec<bool>,
}

impl Traversal<'_> {
    fn direction(&self, index: usize) -> f64 {
        if self.route.ways[index].reverse {
            -1.0
        } else {
            1.0
        }
    }
    fn between(
        &self,
        from: Position,
        to: Position,
        mut visit: Option<&mut dyn FnMut(usize, f64, f64)>,
    ) -> f64 {
        if to.occurrence < from.occurrence {
            return f64::INFINITY;
        }
        let mut length = 0.0;
        let mut travel = |index: usize, a: f64, b: f64| {
            let span = (b - a) * self.direction(index);
            length = if span < 0.0 {
                f64::INFINITY
            } else {
                length + span
            };
            if length.is_finite()
                && let Some(callback) = visit.as_mut()
            {
                callback(index, a, b);
            }
        };
        if from.occurrence == to.occurrence {
            travel(from.occurrence, from.local, to.local);
        } else if to.occurrence == from.occurrence + 1 && self.reversed[to.occurrence] {
            let turn = if self.direction(from.occurrence) == 1.0 {
                from.local.max(to.local)
            } else {
                from.local.min(to.local)
            };
            travel(from.occurrence, from.local, turn);
            travel(to.occurrence, turn, to.local);
        } else {
            if self.reversed[from.occurrence + 1..=to.occurrence]
                .iter()
                .any(|value| *value)
            {
                return f64::INFINITY;
            }
            travel(from.occurrence, from.local, self.exits[from.occurrence]);
            for index in from.occurrence + 1..to.occurrence {
                travel(index, self.entries[index], self.exits[index]);
            }
            travel(to.occurrence, self.entries[to.occurrence], to.local);
        }
        length
    }
}

fn candidates(point: [f64; 2], previous: &[State], segments: &[Segment]) -> Vec<Position> {
    let mut positions = Vec::new();
    let mut indices = HashMap::new();
    let mut add = |occurrence: usize, local: f64, at: [f64; 2]| {
        let error = distance(point, at);
        if error <= 250.0 {
            let value = Position {
                occurrence,
                local,
                point: at,
                error,
            };
            if let Some(&index) = indices.get(&(occurrence, local.to_bits())) {
                positions[index] = value;
            } else {
                indices.insert((occurrence, local.to_bits()), positions.len());
                positions.push(value);
            }
        }
    };
    for segment in segments {
        let [a, b] = segment.points;
        let fraction = project(point, a, b)
            .1
            .clamp(segment.fractions[0], segment.fractions[1]);
        let at = [
            a[0] + fraction * (b[0] - a[0]),
            a[1] + fraction * crate::osm::geometry::wrapped_longitude_delta(a[1], b[1]),
        ];
        let local = if fraction == 1.0 {
            segment.range[1]
        } else {
            segment.range[0] + fraction * (segment.range[1] - segment.range[0])
        };
        add(segment.occurrence, local, at);
    }
    for state in previous {
        add(
            state.position.occurrence,
            state.position.local,
            state.position.point,
        );
    }
    positions
}

pub fn align(shape: &[[f64; 2]], route: &Itinerary) -> Option<Vec<Passage>> {
    if shape.len() < 2 || route.ways.is_empty() {
        return None;
    }
    let mut traversal = Traversal {
        route,
        entries: Vec::new(),
        exits: Vec::new(),
        reversed: Vec::new(),
    };
    for (index, directed) in route.ways.iter().enumerate() {
        let length = *directed.way.metres.last()?;
        traversal
            .entries
            .push(if directed.reverse { length } else { 0.0 });
        traversal
            .exits
            .push(if directed.reverse { 0.0 } else { length });
        traversal.reversed.push(
            index > 0
                && directed.way.id == route.ways[index - 1].way.id
                && directed.reverse != route.ways[index - 1].reverse,
        );
    }
    for index in 1..route.ways.len() {
        if traversal.reversed[index] {
            continue;
        }
        let before = &route.ways[index - 1].way;
        let after = &route.ways[index].way;
        let mut joins = Vec::new();
        for (a, first) in before.nodes.iter().enumerate() {
            for (b, second) in after.nodes.iter().enumerate() {
                if first == second {
                    let pair = (before.metres[a], after.metres[b]);
                    if !joins.contains(&pair) {
                        joins.push(pair);
                    }
                }
            }
        }
        if joins.len() != 1 {
            return None;
        }
        traversal.exits[index - 1] = joins[0].0;
        traversal.entries[index] = joins[0].1;
    }
    let mut segments = Vec::new();
    for (index, directed) in route.ways.iter().enumerate() {
        if (traversal.exits[index] - traversal.entries[index]) * traversal.direction(index) < 0.0 {
            return None;
        }
        let lower = traversal.entries[index].min(traversal.exits[index]);
        let upper = traversal.entries[index].max(traversal.exits[index]);
        for vertex in 1..directed.way.points.len() {
            let start = directed.way.metres[vertex - 1];
            let end = directed.way.metres[vertex];
            let length = end - start;
            if end < lower || start > upper || length == 0.0 {
                continue;
            }
            segments.push(Segment {
                occurrence: index,
                points: [directed.way.points[vertex - 1], directed.way.points[vertex]],
                range: [start, end],
                fractions: [
                    0.0_f64.max((lower - start) / length),
                    1.0_f64.min((upper - start) / length),
                ],
            });
        }
    }
    let mut layers = Vec::<Vec<State>>::new();
    for (vertex, point) in shape.iter().enumerate() {
        let previous = layers.last().map(Vec::as_slice).unwrap_or_default();
        let mut column = Vec::new();
        let source_distance = if vertex > 0 {
            distance(shape[vertex - 1], *point)
        } else {
            0.0
        };
        for position in candidates(*point, previous, &segments) {
            let mut cost = if vertex == 0 {
                position.error.powi(2)
            } else {
                f64::INFINITY
            };
            let mut best = usize::MAX;
            for (index, state) in previous.iter().enumerate() {
                let length = traversal.between(state.position, position, None);
                let candidate =
                    state.cost + position.error.powi(2) + (length - source_distance).powi(2);
                if candidate < cost {
                    cost = candidate;
                    best = index;
                }
            }
            if cost.is_finite() {
                column.push(State {
                    position,
                    cost,
                    previous: best,
                });
            }
        }
        if column.is_empty() {
            return None;
        }
        layers.push(column);
    }
    let last = layers.last()?;
    let mut chosen = 0;
    for index in 1..last.len() {
        if last[index].cost < last[chosen].cost {
            chosen = index;
        }
    }
    let mut positions = Vec::new();
    for layer in layers.iter().rev() {
        let state = &layer[chosen];
        positions.push(state.position);
        chosen = state.previous;
    }
    positions.reverse();
    let mut passages = Vec::<Passage>::new();
    for pair in positions.windows(2) {
        traversal.between(
            pair[0],
            pair[1],
            Some(&mut |index, from, to| {
                if from == to {
                    return;
                }
                if let Some(last) = passages.last_mut()
                    && last.occurrence == index as i64
                    && last.to == from
                {
                    last.to = to;
                    return;
                }
                passages.push(Passage {
                    way: route.ways[index].way.id,
                    occurrence: index as i64,
                    from,
                    to,
                });
            }),
        );
    }
    Some(passages)
}
