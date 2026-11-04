//! Resolve building demand on local street components before the common carriageway allocation.

use super::{preparation_buildings, preparation_model::DemandModel};
use crate::roads::input::Road;
use anyhow::Result;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Default)]
struct Node {
    edges: Vec<usize>,
    exit: bool,
}

#[derive(Default)]
struct Component {
    segments: Vec<usize>,
    roots: Vec<usize>,
}

#[derive(Default)]
struct Street {
    trips: f64,
    through: bool,
    one_lane: bool,
    multi_lane: bool,
}

// Equal-distance insertions and removals retain the original binary heap's first winner.
#[derive(Default)]
struct Heap(Vec<(f64, usize)>);
impl Heap {
    fn push(&mut self, value: (f64, usize)) {
        self.0.push(value);
        let mut i = self.0.len() - 1;
        while i > 0 {
            let parent = (i - 1) / 2;
            if self.0[parent].0 <= self.0[i].0 {
                break;
            }
            self.0.swap(i, parent);
            i = parent;
        }
    }
    fn pop(&mut self) -> Option<(f64, usize)> {
        if self.0.is_empty() {
            return None;
        }
        let top = self.0.swap_remove(0);
        let mut i = 0;
        loop {
            let mut smallest = i;
            for child in [i * 2 + 1, i * 2 + 2] {
                if child < self.0.len() && self.0[child].0 < self.0[smallest].0 {
                    smallest = child;
                }
            }
            if smallest == i {
                break;
            }
            self.0.swap(i, smallest);
            i = smallest;
        }
        Some(top)
    }
}

fn components(nodes: &[Node], ends: &[[usize; 2]], eligible: &[bool]) -> Vec<Component> {
    let mut seen = vec![false; ends.len()];
    let mut result = Vec::new();
    for seed in 0..ends.len() {
        if !eligible[seed] || seen[seed] {
            continue;
        }
        seen[seed] = true;
        let mut component = Component {
            segments: vec![seed],
            ..Component::default()
        };
        let mut roots = HashSet::new();
        let mut cursor = 0;
        while cursor < component.segments.len() {
            for node in ends[component.segments[cursor]] {
                if nodes[node].exit && roots.insert(node) {
                    component.roots.push(node);
                }
                for &edge in &nodes[node].edges {
                    if !seen[edge] {
                        seen[edge] = true;
                        component.segments.push(edge);
                    }
                }
            }
            cursor += 1;
        }
        result.push(component);
    }
    result
}

fn accumulate(
    component: &Component,
    ends: &[[usize; 2]],
    roads: &[Road],
    loads: &[[f64; 2]],
    model: &DemandModel,
) -> (HashMap<usize, f64>, HashSet<usize>) {
    let mut ids = HashMap::new();
    let mut adjacency: Vec<Vec<usize>> = Vec::new();
    let mut local_ends = Vec::new();
    let mut flow = Vec::new();
    for (local, &index) in component.segments.iter().enumerate() {
        let ends = ends[index].map(|node| {
            *ids.entry(node).or_insert_with(|| {
                adjacency.push(Vec::new());
                adjacency.len() - 1
            })
        });
        for node in ends {
            adjacency[node].push(local);
        }
        local_ends.push(ends);
        flow.push(loads[index][0] * model.fleet(roads[index].country.0)[1] + loads[index][1]);
    }
    let count = adjacency.len();
    let mut distance = vec![f64::INFINITY; count];
    let mut exit = vec![usize::MAX; count];
    let mut down = vec![None; count];
    let mut roots = component
        .roots
        .iter()
        .filter_map(|r| ids.get(r).copied())
        .collect::<Vec<_>>();
    if roots.is_empty() {
        let mut best = 0;
        for i in 1..count {
            if adjacency[i].len() > adjacency[best].len() {
                best = i;
            }
        }
        roots.push(best);
    }
    let mut heap = Heap::default();
    for root in roots {
        distance[root] = 0.0;
        exit[root] = root;
        heap.push((0.0, root));
    }
    while let Some((d, node)) = heap.pop() {
        if d > distance[node] {
            continue;
        }
        for &edge in &adjacency[node] {
            let [a, b] = local_ends[edge];
            let other = if a == node { b } else { a };
            let next = d + roads[component.segments[edge]].length_m.max(1.0);
            if next < distance[other] {
                distance[other] = next;
                down[other] = Some(edge);
                exit[other] = exit[node];
                heap.push((next, other));
            }
        }
    }
    let mut order = (0..count).collect::<Vec<_>>();
    order.sort_by(|&a, &b| distance[b].total_cmp(&distance[a]));
    for node in order {
        let mut incoming = 0.0;
        for &edge in &adjacency[node] {
            let [a, b] = local_ends[edge];
            let other = if a == node { b } else { a };
            if distance[other] > distance[node] {
                incoming += flow[edge];
            }
        }
        if let Some(edge) = down[node] {
            flow[edge] += incoming;
        }
    }
    let boundaries = component
        .segments
        .iter()
        .enumerate()
        .filter_map(|(i, &index)| {
            (exit[local_ends[i][0]] != exit[local_ends[i][1]]).then_some(index)
        })
        .collect();
    (
        component.segments.iter().copied().zip(flow).collect(),
        boundaries,
    )
}

pub(crate) fn apply(roads: &mut [Road], structures: &Path, model: &DemandModel) -> Result<()> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut ids = HashMap::new();
    let mut ends = Vec::new();
    let mut eligible = Vec::new();
    for (index, road) in roads.iter().enumerate() {
        let pair = road.ends.map(|point| {
            *ids.entry(point).or_insert_with(|| {
                nodes.push(Node::default());
                nodes.len() - 1
            })
        });
        let local = [5, 6, 7, 9].contains(&road.class);
        let allowed =
            local && !road.tunnel && ![2, 4].contains(&road.access) && road.source_id == 0;
        for node in pair {
            if allowed {
                nodes[node].edges.push(index);
            } else if road.class < 5 || road.class >= 10 || local {
                nodes[node].exit = true;
            }
        }
        ends.push(pair);
        eligible.push(allowed);
    }
    let components = components(&nodes, &ends, &eligible);
    let selected = components
        .iter()
        .flat_map(|c| c.segments.iter().copied())
        .collect::<Vec<_>>();
    let (loads, has_buildings) =
        preparation_buildings::assign(structures, roads, &selected, model)?;
    let mut row_trips = vec![0.0; roads.len()];
    let mut streets: HashMap<String, Street> = HashMap::new();
    let mut street_keys = vec![String::new(); roads.len()];
    for (component_id, component) in components.iter().enumerate() {
        let (flow, boundaries) = accumulate(component, &ends, roads, &loads, model);
        for &index in &component.segments {
            let road = &roads[index];
            row_trips[index] = flow[&index];
            let key = if road.name.is_empty() {
                format!("way/{}", road.way_id)
            } else {
                format!("{component_id}/{}", road.name)
            };
            let street = streets.entry(key.clone()).or_default();
            street.trips = street.trips.max(flow[&index]);
            street.through |= boundaries.contains(&index);
            street.one_lane |= road.lanes == 1;
            street.multi_lane |= road.lanes > 1;
            street_keys[index] = key;
        }
    }
    for index in selected {
        let road = &mut roads[index];
        if road.class == 7 && !has_buildings {
            continue;
        }
        let street = &streets[&street_keys[index]];
        let trips = if road.class == 7 {
            let Some([minimum, maximum]) = model.service_trip_bounds else {
                continue;
            };
            row_trips[index].clamp(minimum, maximum)
        } else {
            // Without an admitted street fit, retain the common class prior; no measured background is zero.
            let Some(parameters) = &model.street else {
                continue;
            };
            let background = if road.built_up == 2 {
                if road.class == 9 {
                    parameters.unclassified_urban
                } else {
                    parameters.residential_urban
                }
            } else {
                parameters.rural
            };
            let through = if street.through && road.built_up == 2 {
                parameters.through_factor
            } else {
                1.0
            };
            let narrow = if street.one_lane && !street.multi_lane {
                parameters.single_track_factor
            } else {
                1.0
            };
            background * through * narrow + parameters.demand_scale * street.trips
        };
        let total = trips.round();
        let medium = (total * model.medium_share).round();
        let heavy = (total * model.heavy_share).round();
        let moto = (total * model.fleet(road.country.0)[0]).round();
        road.counts = [total - medium - heavy - moto, medium, heavy, moto];
        road.source_id = 11;
        road.observation_source_id = 11;
        road.basis = 3;
        road.estimated = 15;
    }
    Ok(())
}
