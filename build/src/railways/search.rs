//! Deterministic shortest paths and rejection of distinct similarly short railway itineraries.
use super::network::{Edge, Network, project};
use std::collections::{HashMap, HashSet};

pub struct Path {
    pub length: f64,
    pub edges: Vec<usize>,
}
pub struct Search {
    distances: Vec<f64>,
    predecessors: Vec<(usize, usize)>,
    visited: Vec<bool>,
    touched: Vec<usize>,
}

// Equal distances preserve the reference heap's insertion and child order, which selects the track.
#[derive(Default)]
struct Queue(Vec<(f64, usize)>);
impl Queue {
    fn push(&mut self, distance: f64, node: usize) {
        self.0.push((distance, node));
        let mut index = self.0.len() - 1;
        while index > 0 {
            let parent = (index - 1) / 2;
            if self.0[parent].0 <= self.0[index].0 {
                break;
            }
            self.0.swap(parent, index);
            index = parent;
        }
    }
    fn pop(&mut self) -> Option<(f64, usize)> {
        let first = *self.0.first()?;
        let last = self.0.pop().unwrap();
        if !self.0.is_empty() {
            self.0[0] = last;
            let mut index = 0;
            loop {
                let mut smallest = index;
                for child in [index * 2 + 1, index * 2 + 2] {
                    if child < self.0.len() && self.0[child].0 < self.0[smallest].0 {
                        smallest = child;
                    }
                }
                if smallest == index {
                    break;
                }
                self.0.swap(index, smallest);
                index = smallest;
            }
        }
        Some(first)
    }
}

impl Search {
    pub fn new(nodes: usize) -> Self {
        Self {
            distances: vec![f64::INFINITY; nodes],
            predecessors: vec![(usize::MAX, usize::MAX); nodes],
            visited: vec![false; nodes],
            touched: Vec::new(),
        }
    }
    pub fn shortest(
        &mut self,
        network: &Network,
        from: usize,
        to: usize,
        allowed: &impl Fn(&Edge) -> bool,
        penalized: &HashSet<usize>,
    ) -> Option<Path> {
        self.penalized(network, from, to, allowed, penalized, 3.0)
    }
    pub fn penalized(
        &mut self,
        network: &Network,
        from: usize,
        to: usize,
        allowed: &impl Fn(&Edge) -> bool,
        penalized: &HashSet<usize>,
        penalty: f64,
    ) -> Option<Path> {
        for node in self.touched.drain(..) {
            self.distances[node] = f64::INFINITY;
            self.visited[node] = false;
        }
        let mut queue = Queue::default();
        self.distances[from] = 0.0;
        self.touched.push(from);
        queue.push(0.0, from);
        while let Some((distance, node)) = queue.pop() {
            if self.visited[node] {
                continue;
            }
            self.visited[node] = true;
            if node == to {
                break;
            }
            for &index in &network.adjacency[node] {
                let edge = &network.edges[index];
                if !allowed(edge) {
                    continue;
                }
                let other = if edge.nodes[0] == node {
                    edge.nodes[1]
                } else {
                    edge.nodes[0]
                };
                if self.visited[other] {
                    continue;
                }
                let next = distance
                    + edge.length
                        * if penalized.contains(&index) {
                            penalty
                        } else {
                            1.0
                        };
                if next < self.distances[other] {
                    if self.distances[other].is_infinite() {
                        self.touched.push(other);
                    }
                    self.distances[other] = next;
                    self.predecessors[other] = (index, node);
                    queue.push(next, other);
                }
            }
        }
        if self.distances[to].is_infinite() {
            return None;
        }
        let mut path = Path {
            length: 0.0,
            edges: Vec::new(),
        };
        let mut node = to;
        while node != from {
            let (edge, previous) = self.predecessors[node];
            path.edges.push(edge);
            path.length += network.edges[edge].length;
            node = previous;
        }
        path.edges.reverse();
        Some(path)
    }

    pub fn ambiguous(
        &mut self,
        network: &Network,
        from: usize,
        to: usize,
        best: &Path,
        allowed: &impl Fn(&Edge) -> bool,
    ) -> bool {
        self.alternative(network, from, to, best, allowed).is_some()
    }
    pub fn alternative(
        &mut self,
        network: &Network,
        from: usize,
        to: usize,
        best: &Path,
        allowed: &impl Fn(&Edge) -> bool,
    ) -> Option<Path> {
        let edges: HashSet<_> = best.edges.iter().copied().collect();
        let alternative = self.shortest(network, from, to, allowed, &edges)?;
        if edges.is_empty() || alternative.length > 1.2 * best.length {
            return None;
        }
        let other: HashSet<_> = alternative.edges.iter().copied().collect();
        if other.intersection(&edges).count() as f64 / edges.len() as f64 >= 0.5 {
            return None;
        }
        if parallel_twin(network, best, &alternative) {
            None
        } else {
            Some(alternative)
        }
    }

    pub fn flood(&mut self, network: &Network, from: usize, bound: f64) -> Vec<(usize, f64)> {
        for node in self.touched.drain(..) {
            self.distances[node] = f64::INFINITY;
            self.visited[node] = false;
        }
        let mut queue = Queue::default();
        self.distances[from] = 0.0;
        self.touched.push(from);
        queue.push(0.0, from);
        let mut result = Vec::new();
        while let Some((distance, node)) = queue.pop() {
            if self.visited[node] {
                continue;
            }
            self.visited[node] = true;
            if distance > bound {
                continue;
            }
            result.push((node, distance));
            for &index in &network.adjacency[node] {
                let edge = &network.edges[index];
                let other = if edge.nodes[0] == node {
                    edge.nodes[1]
                } else {
                    edge.nodes[0]
                };
                if self.visited[other] {
                    continue;
                }
                let next = distance + edge.length;
                if next < self.distances[other] {
                    if self.distances[other].is_infinite() {
                        self.touched.push(other);
                    }
                    self.distances[other] = next;
                    queue.push(next, other);
                }
            }
        }
        result
    }
}

fn parallel_twin(network: &Network, best: &Path, alternative: &Path) -> bool {
    let best_set: HashSet<_> = best.edges.iter().copied().collect();
    let mut cells = HashMap::<(i32, i32), Vec<usize>>::new();
    for &index in &best.edges {
        let edge = &network.edges[index];
        if edge.traversal_only {
            continue;
        }
        let [a, b] = edge.points;
        for y in (a[0].min(b[0]) / 0.001).floor() as i32..=(a[0].max(b[0]) / 0.001).floor() as i32 {
            for x in
                (a[1].min(b[1]) / 0.001).floor() as i32..=(a[1].max(b[1]) / 0.001).floor() as i32
            {
                cells.entry((y, x)).or_default().push(index);
            }
        }
    }
    let mut samples = Vec::new();
    let mut total = 0.0;
    let mut far = 0.0;
    for &index in &alternative.edges {
        let edge = &network.edges[index];
        if best_set.contains(&index) || edge.traversal_only {
            continue;
        }
        let point = edge.midpoint();
        let y = (point[0] / 0.001).floor() as i32;
        let x = (point[1] / 0.001).floor() as i32;
        let dy = (500.0_f64 / 110.54).ceil() as i32;
        let dx = (500.0 / (111.32 * (point[0] * std::f64::consts::PI / 180.0).cos().max(0.05)))
            .ceil()
            .max(1.0) as i32;
        let mut distance = 500.0_f64;
        let mut seen = HashSet::new();
        for oy in -dy..=dy {
            for ox in -dx..=dx {
                if let Some(bucket) = cells.get(&(y + oy, x + ox)) {
                    for &index in bucket {
                        if !seen.insert(index) {
                            continue;
                        }
                        let edge = &network.edges[index];
                        distance = distance.min(project(point, edge.points[0], edge.points[1]).0);
                    }
                }
            }
        }
        total += edge.length;
        if distance >= 500.0 {
            far += edge.length;
        }
        samples.push((distance, edge.length));
    }
    if total == 0.0 {
        return true;
    }
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cumulative = 0.0;
    let mut median = None;
    let mut percentile = samples.last().unwrap().0;
    for (distance, length) in samples {
        cumulative += length;
        if cumulative > total / 2.0 && median.is_none() {
            median = Some(distance);
        }
        if cumulative > total * 0.75 {
            percentile = distance;
            break;
        }
    }
    median.unwrap() <= 50.0 && percentile <= 120.0 && far / total <= 0.10
}
