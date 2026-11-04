//! Grade implicit speeds along junction-free chains using the manifest's legal limits and calibrated distances.

use super::input::Road;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    pub countries: BTreeMap<u16, [f64; 4]>,
    pub classes: Vec<u8>,
    pub defaults: [f64; 13],
    pub unrestricted: f64,
    pub anchored_distance_m: f64,
    pub implicit_distance_m: f64,
    pub minimum_speed_difference: f64,
    pub minimum_level_difference: f64,
}

impl Parameters {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.classes.iter().all(|c| *c < 13)
                && self
                    .defaults
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0 && *v < 255.0)
                && self
                    .countries
                    .values()
                    .flatten()
                    .all(|v| v.is_finite() && *v >= 0.0 && *v < 255.0)
                && [
                    self.unrestricted,
                    self.anchored_distance_m,
                    self.implicit_distance_m,
                    self.minimum_speed_difference,
                    self.minimum_level_difference
                ]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0),
            "invalid speed-transition parameters"
        );
        Ok(())
    }

    fn eligible(&self, road: &Road) -> bool {
        self.classes.contains(&road.class)
            && road.speed_limit == 0
            && [0, 5].contains(&road.access)
            && !road.roundabout
    }

    fn speed(&self, road: &Road, country: [f64; 4]) -> f64 {
        match road.speed_limit {
            255 => return self.unrestricted,
            1..=254 => return f64::from(road.speed_limit),
            _ => (),
        }
        let [urban, rural, motorway, motorroad] = country;
        let legal = match road.class {
            0 => motorway,
            1 => {
                if motorroad > 0.0 {
                    motorroad
                } else {
                    rural
                }
            }
            2..=4 | 9 => match road.built_up {
                1 => rural,
                2 => urban,
                _ => 0.0,
            },
            _ => 0.0,
        };
        if legal > 0.0 {
            legal
        } else {
            self.defaults[usize::from(road.class)]
        }
    }
}

type Point = (i32, i32);

struct TransitionGraph {
    neighbors: Vec<Vec<(usize, Point)>>,
    degree: HashMap<Point, usize>,
    insertion_order: Vec<usize>,
}

impl TransitionGraph {
    fn new(roads: &[Road], selected: &[usize]) -> Self {
        let mut graph = Self {
            neighbors: vec![Vec::new(); roads.len()],
            degree: HashMap::new(),
            insertion_order: Vec::new(),
        };
        let mut ways: HashMap<i64, Vec<usize>> = HashMap::new();
        let mut way_order = Vec::new();
        let mut node_ways: HashMap<Point, Vec<i64>> = HashMap::new();
        let mut node_order = Vec::new();
        for &index in selected {
            let road = &roads[index];
            if !ways.contains_key(&road.way_id) {
                way_order.push(road.way_id);
            }
            ways.entry(road.way_id).or_default().push(index);
            if [7, 8].contains(&road.class) {
                continue;
            }
            for point in road.ends {
                if !node_ways.contains_key(&point) {
                    node_order.push(point);
                }
                let connected = node_ways.entry(point).or_default();
                if !connected.contains(&road.way_id) {
                    connected.push(road.way_id);
                }
                *graph.degree.entry(point).or_default() += 1;
            }
        }
        for way in way_order {
            let rows = ways.get_mut(&way).expect("collected way");
            rows.sort_by_key(|&index| roads[index].segment_idx);
            for pair in rows.windows(2) {
                let point = roads[pair[0]]
                    .ends
                    .into_iter()
                    .find(|point| roads[pair[1]].ends.contains(point));
                if let Some(point) = point {
                    graph.link(pair[0], pair[1], point);
                }
            }
        }
        for point in node_order {
            let ways_at_node = &node_ways[&point];
            if ways_at_node.len() != 2 || graph.degree[&point] != 2 {
                continue;
            }
            let terminal = ways_at_node
                .iter()
                .filter_map(|way| {
                    let rows = &ways[way];
                    [rows[0], rows[rows.len() - 1]]
                        .into_iter()
                        .find(|&index| roads[index].ends.contains(&point))
                })
                .collect::<Vec<_>>();
            if let [first, second] = terminal.as_slice() {
                graph.link(*first, *second, point);
            }
        }
        graph
    }

    fn link(&mut self, first: usize, second: usize, point: Point) {
        for (from, to) in [(first, second), (second, first)] {
            if self.neighbors[from].is_empty() {
                self.insertion_order.push(from);
            }
            self.neighbors[from].push((to, point));
        }
    }
}

struct Planner<'a> {
    roads: &'a [Road],
    graph: TransitionGraph,
    parameters: &'a Parameters,
    country: [f64; 4],
    result: Vec<Option<(f64, u8)>>,
}

impl Planner<'_> {
    fn walk(&mut self, mut current: usize, mut previous: usize, window: f64, anchor: f64) {
        let mut distance = 0.0;
        let mut seen = BTreeSet::new();
        while distance < window && seen.insert(current) {
            let road = &self.roads[current];
            if !self.parameters.eligible(road) {
                return;
            }
            let midpoint = distance + road.length_m / 2.0;
            let own = self.parameters.speed(road, self.country);
            let speed = (anchor + (own - anchor) * (midpoint / window).min(1.0)).round();
            if (speed - own).abs() >= self.parameters.minimum_speed_difference
                && self.result[current].is_none_or(|(old, _)| old > midpoint)
            {
                self.result[current] = Some((midpoint, speed.clamp(1.0, 254.0) as u8));
            }
            distance += road.length_m;
            let mut next = self.graph.neighbors[current]
                .iter()
                .filter(|(index, _)| *index != previous);
            let Some(&(index, point)) = next.next() else {
                return;
            };
            if next.next().is_some() || self.graph.degree.get(&point).copied().unwrap_or(0) > 2 {
                return;
            }
            previous = current;
            current = index;
        }
    }
}

pub fn apply(roads: &mut [Road], parameters: &Parameters) {
    for (&country, &limits) in &parameters.countries {
        let selected = roads
            .iter()
            .enumerate()
            .filter_map(|(i, r)| (r.country.0 == country).then_some(i))
            .collect::<Vec<_>>();
        let graph = TransitionGraph::new(roads, &selected);
        let boundaries = graph
            .insertion_order
            .iter()
            .flat_map(|&first| {
                graph.neighbors[first]
                    .iter()
                    .filter(move |(second, _)| first < *second)
                    .map(move |&(second, point)| (first, second, point))
            })
            .collect::<Vec<_>>();
        let mut planner = Planner {
            roads,
            graph,
            parameters,
            country: limits,
            result: vec![None; roads.len()],
        };
        for (first, second, point) in boundaries {
            let a = &planner.roads[first];
            let b = &planner.roads[second];
            if (!parameters.classes.contains(&a.class) && !parameters.classes.contains(&b.class))
                || a.roundabout
                || b.roundabout
                || planner.graph.degree.get(&point).copied().unwrap_or(0) > 2
            {
                continue;
            }
            let eligible_a = parameters.eligible(a);
            let eligible_b = parameters.eligible(b);
            let speed_a = parameters.speed(a, limits);
            let speed_b = parameters.speed(b, limits);
            // Dev1's trigger is the CNOSSOS category-1 rolling coefficient, 30 log10(v).
            if (speed_a - speed_b).abs() < parameters.minimum_speed_difference
                || (30.0 * (speed_b / speed_a).log10()).abs() < parameters.minimum_level_difference
            {
                continue;
            }
            for (current, previous, eligible, other_eligible, other_speed) in [
                (first, second, eligible_a, eligible_b, speed_b),
                (second, first, eligible_b, eligible_a, speed_a),
            ] {
                if eligible {
                    planner.walk(
                        current,
                        previous,
                        if other_eligible {
                            parameters.implicit_distance_m
                        } else {
                            parameters.anchored_distance_m
                        },
                        if other_eligible {
                            (speed_a + speed_b) / 2.0
                        } else {
                            other_speed
                        },
                    );
                }
            }
        }
        let result = planner.result;
        for index in selected {
            roads[index].speed_taper = result[index].map_or(0, |(_, speed)| speed);
        }
    }
}
