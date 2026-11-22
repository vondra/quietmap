//! Cut-invariant heavy-rail runs, spatial edge index and corridor token compatibility.
use super::network::{Edge, Network, project};
use crate::osm::geometry::wrapped_longitude_delta;
use std::collections::{HashMap, HashSet};

pub(super) const MATCH_GATE_M: f64 = 60.0;
const GRID_CELL_DEG: f64 = 0.005;
// Reject acute switchback hairpins (> 120 deg turn) at degree-2 stub-switch nodes.
const RUN_CONTINUE_MIN_DOT: f64 = -0.5;

pub(super) fn usage_family(usage: u8) -> u8 {
    if usage == 3 { 0 } else { usage }
}

pub(super) fn shares_node(a: [usize; 2], b: [usize; 2]) -> bool {
    a[0] == b[0] || a[0] == b[1] || a[1] == b[0] || a[1] == b[1]
}

fn grid_cell(point: [f64; 2]) -> (i32, i32) {
    (
        (point[0] / GRID_CELL_DEG).floor() as i32,
        (point[1] / GRID_CELL_DEG).floor() as i32,
    )
}

pub(super) fn to_local_m(origin: [f64; 2], cos_lat: f64, p: [f64; 2]) -> [f64; 2] {
    [
        wrapped_longitude_delta(origin[1], p[1]) * 111_320.0 * cos_lat,
        (p[0] - origin[0]) * 110_540.0,
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenRel {
    Same,
    Tokenless,
    Conflict,
}

pub(super) fn tokens_match(a: &Edge, b: &Edge) -> TokenRel {
    if a.corridor.is_empty() || b.corridor.is_empty() {
        return TokenRel::Tokenless;
    }
    if a.corridor == b.corridor {
        return TokenRel::Same;
    }
    if (!a.corridor_name.is_empty() && b.corridor_name.is_empty() && a.corridor_name == b.corridor)
        || (!b.corridor_name.is_empty()
            && a.corridor_name.is_empty()
            && b.corridor_name == a.corridor)
    {
        return TokenRel::Same;
    }
    TokenRel::Conflict
}

/// Spatial index rasterizing every eligible heavy-rail segment across all cells it crosses.
pub(super) struct EdgeGrid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl EdgeGrid {
    pub(super) fn build(network: &Network, eligible: &[bool]) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (edge_idx, edge) in network.edges.iter().enumerate() {
            if !eligible[edge_idx] {
                continue;
            }
            let [a, b] = edge.points;
            let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) / (GRID_CELL_DEG * 0.5))
                .ceil()
                .max(1.0) as usize;
            let mut prev = None;
            for step in 0..=steps {
                let frac = step as f64 / steps as f64;
                let cell = grid_cell([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
                if prev != Some(cell) {
                    cells.entry(cell).or_default().push(edge_idx);
                    prev = Some(cell);
                }
            }
        }
        Self { cells }
    }

    pub(super) fn candidates_for_edge(
        &self,
        edge: &Edge,
        min_idx: usize,
        radius_m: f64,
    ) -> Vec<usize> {
        let [a, b] = edge.points;
        let mid_lat = 0.5 * (a[0] + b[0]);
        let dy = (radius_m / (GRID_CELL_DEG * 110_540.0)).ceil().max(1.0) as i32;
        let dx = (radius_m / (GRID_CELL_DEG * 111_320.0 * mid_lat.to_radians().cos().max(0.05)))
            .ceil()
            .max(1.0) as i32;
        let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) / (GRID_CELL_DEG * 0.5))
            .ceil()
            .max(1.0) as usize;
        let mut out = Vec::new();
        let mut prev = None;
        for step in 0..=steps {
            let frac = step as f64 / steps as f64;
            let cell = grid_cell([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
            if prev == Some(cell) {
                continue;
            }
            prev = Some(cell);
            for oy in -dy..=dy {
                for ox in -dx..=dx {
                    if let Some(bucket) = self.cells.get(&(cell.0 + oy, cell.1 + ox)) {
                        for &edge_idx in bucket {
                            if edge_idx > min_idx {
                                out.push(edge_idx);
                            }
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub(super) fn nearest_edge(&self, origin: [f64; 2], network: &Network) -> Option<(usize, f64)> {
        let (cy, cx) = grid_cell(origin);
        let cos_lat = origin[0].to_radians().cos().max(0.05);
        let dy = (MATCH_GATE_M / (GRID_CELL_DEG * 110_540.0)).ceil().max(1.0) as i32;
        let dx = (MATCH_GATE_M / (GRID_CELL_DEG * 111_320.0 * cos_lat))
            .ceil()
            .max(1.0) as i32;
        let mut seen = HashSet::new();
        type EdgeTieKey = (i64, i16, [u16; 2], usize);
        let mut best: Option<(f64, EdgeTieKey, usize, f64)> = None;
        for oy in -dy..=dy {
            for ox in -dx..=dx {
                for &edge_idx in self
                    .cells
                    .get(&(cy + oy, cx + ox))
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    if !seen.insert(edge_idx) {
                        continue;
                    }
                    let edge = &network.edges[edge_idx];
                    let a = to_local_m(origin, cos_lat, edge.points[0]);
                    let b = to_local_m(origin, cos_lat, edge.points[1]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let len = dx.hypot(dy);
                    if len < 1e-3 {
                        continue;
                    }
                    let frac = (-(a[0] * dx + a[1] * dy) / (len * len)).clamp(0.0, 1.0);
                    let dist = (a[0] + frac * dx).hypot(a[1] + frac * dy);
                    if dist > MATCH_GATE_M {
                        continue;
                    }
                    let key = (edge.way, edge.segment, edge.square, edge_idx);
                    if best.is_none_or(|b| dist < b.0 || (dist == b.0 && key < b.1)) {
                        best = Some((dist, key, edge_idx, frac));
                    }
                }
            }
        }
        best.map(|(_, _, edge_idx, frac)| (edge_idx, frac))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RunEdge {
    pub edge_idx: usize,
    pub reversed: bool,
    pub s0: f64,
    pub s1: f64,
}

#[derive(Debug)]
pub(super) struct Run {
    pub edges: Vec<RunEdge>,
    pub total_len: f64,
    pub closed: bool,
    pub endpoints: Option<[usize; 2]>,
    sort_key: (i64, u64),
}

impl Run {
    pub(super) fn edge_pos_at(&self, s: f64) -> usize {
        let s = s.clamp(0.0, self.total_len);
        let idx = self.edges.partition_point(|e| e.s1 < s);
        idx.min(self.edges.len().saturating_sub(1))
    }

    pub(super) fn point_at(&self, network: &Network, s: f64) -> [f64; 2] {
        let re = self.edges[self.edge_pos_at(s)];
        let edge = &network.edges[re.edge_idx];
        let frac = if re.s1 - re.s0 > 1e-9 {
            ((s - re.s0) / (re.s1 - re.s0)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let t = if re.reversed { 1.0 - frac } else { frac };
        [
            edge.points[0][0] + t * (edge.points[1][0] - edge.points[0][0]),
            edge.points[0][1] + t * (edge.points[1][1] - edge.points[0][1]),
        ]
    }

    pub(super) fn s_of_edge_t(&self, pos: usize, t_edge: f64) -> f64 {
        let re = self.edges[pos];
        let frac = if re.reversed { 1.0 - t_edge } else { t_edge };
        re.s0 + frac.clamp(0.0, 1.0) * (re.s1 - re.s0)
    }

    pub(super) fn s_to_edge_extent(&self, pos: usize, edge: &Edge, s: f64) -> f64 {
        let re = self.edges[pos];
        let frac = if re.s1 - re.s0 > 1e-9 {
            ((s - re.s0) / (re.s1 - re.s0)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let t = if re.reversed { 1.0 - frac } else { frac };
        edge.extent[0] + t * (edge.extent[1] - edge.extent[0])
    }

    pub(super) fn edge_extent_to_s(&self, pos: usize, edge: &Edge, m: f64) -> f64 {
        let span = edge.extent[1] - edge.extent[0];
        let t = if span.abs() > 1e-9 {
            ((m - edge.extent[0]) / span).clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.s_of_edge_t(pos, t)
    }

    pub(super) fn project_onto_range(
        &self,
        network: &Network,
        pt: [f64; 2],
        s_lo: f64,
        s_hi: f64,
        hint: Option<f64>,
    ) -> (f64, f64) {
        let scan = |lo: f64, hi: f64| {
            let start = self.edges.partition_point(|e| e.s1 < lo);
            let mut best = (f64::INFINITY, lo);
            for re in &self.edges[start..] {
                if re.s0 > hi {
                    break;
                }
                let edge = &network.edges[re.edge_idx];
                let (dist, t) = project(pt, edge.points[0], edge.points[1]);
                let frac = if re.reversed { 1.0 - t } else { t };
                let s = (re.s0 + frac * (re.s1 - re.s0)).clamp(s_lo, s_hi);
                if dist < best.0 {
                    best = (dist, s);
                }
            }
            best
        };
        if let Some(h) = hint {
            let local = scan((h - 200.0).max(s_lo), (h + 200.0).min(s_hi));
            if local.0 <= 80.0 {
                return local;
            }
        }
        scan(s_lo, s_hi)
    }
}

fn edges_continue_at(network: &Network, node: usize, a_idx: usize, b_idx: usize) -> bool {
    let origin = network.nodes[node];
    let cos_lat = origin[0].to_radians().cos().max(0.05);
    let other_pt = |idx: usize| {
        let e = &network.edges[idx];
        if e.nodes[0] == node {
            e.points[1]
        } else {
            e.points[0]
        }
    };
    let va = to_local_m(origin, cos_lat, other_pt(a_idx));
    let vb = to_local_m(origin, cos_lat, other_pt(b_idx));
    let la = va[0].hypot(va[1]);
    let lb = vb[0].hypot(vb[1]);
    if la < 1e-6 || lb < 1e-6 {
        return true;
    }
    let dot = (-va[0] * vb[0] - va[1] * vb[1]) / (la * lb);
    dot >= RUN_CONTINUE_MIN_DOT
}

pub(super) fn build_runs(
    network: &Network,
    eligible: &[bool],
) -> (Vec<Run>, Vec<Option<(usize, usize)>>) {
    let mut deg: Vec<Vec<usize>> = vec![Vec::new(); network.nodes.len()];
    for (edge_idx, edge) in network.edges.iter().enumerate() {
        if eligible[edge_idx] && edge.nodes[0] != edge.nodes[1] {
            deg[edge.nodes[0]].push(edge_idx);
            deg[edge.nodes[1]].push(edge_idx);
        }
    }
    let smooth_node = |node: usize| -> bool {
        deg[node].len() == 2 && edges_continue_at(network, node, deg[node][0], deg[node][1])
    };
    let mut visited = vec![false; network.edges.len()];
    let mut runs = Vec::new();
    let edge_key = |idx: usize| {
        let e = &network.edges[idx];
        (e.way, e.extent[0].min(e.extent[1]).to_bits())
    };
    let finish_run = |chain: Vec<(usize, bool)>, closed: bool| -> Run {
        let mut s = 0.0;
        let mut edges = Vec::with_capacity(chain.len());
        let mut sort_key = (i64::MAX, u64::MAX);
        for (edge_idx, reversed) in chain {
            let len = network.edges[edge_idx].length.max(1e-3);
            sort_key = sort_key.min(edge_key(edge_idx));
            edges.push(RunEdge {
                edge_idx,
                reversed,
                s0: s,
                s1: s + len,
            });
            s += len;
        }
        let endpoints = (!closed).then(|| {
            let first = edges[0];
            let last = *edges.last().unwrap();
            [
                network.edges[first.edge_idx].nodes[usize::from(first.reversed)],
                network.edges[last.edge_idx].nodes[usize::from(!last.reversed)],
            ]
        });
        Run {
            edges,
            total_len: s,
            closed,
            endpoints,
            sort_key,
        }
    };
    for (edge_idx, edge) in network.edges.iter().enumerate() {
        if !eligible[edge_idx] || visited[edge_idx] {
            continue;
        }
        if edge.nodes[0] == edge.nodes[1] {
            visited[edge_idx] = true;
            runs.push(finish_run(vec![(edge_idx, false)], true));
            continue;
        }
        let start_node = if !smooth_node(edge.nodes[0]) {
            edge.nodes[0]
        } else if !smooth_node(edge.nodes[1]) {
            edge.nodes[1]
        } else {
            continue;
        };
        let mut chain = Vec::new();
        let (mut cur_node, mut cur_edge) = (start_node, edge_idx);
        loop {
            visited[cur_edge] = true;
            let e = &network.edges[cur_edge];
            let reversed = e.nodes[0] != cur_node;
            chain.push((cur_edge, reversed));
            let next_node = if reversed { e.nodes[0] } else { e.nodes[1] };
            if !smooth_node(next_node) {
                break;
            }
            match deg[next_node].iter().copied().find(|&n| n != cur_edge) {
                Some(ne) if !visited[ne] => {
                    cur_node = next_node;
                    cur_edge = ne;
                }
                _ => break,
            }
        }
        if edge_key(chain.last().unwrap().0) < edge_key(chain[0].0) {
            chain.reverse();
            for (_, rev) in &mut chain {
                *rev = !*rev;
            }
        }
        runs.push(finish_run(chain, false));
    }
    for edge_idx in 0..network.edges.len() {
        if !eligible[edge_idx] || visited[edge_idx] {
            continue;
        }
        let mut chain = Vec::new();
        let (mut cur_edge, mut cur_node) = (edge_idx, network.edges[edge_idx].nodes[0]);
        loop {
            visited[cur_edge] = true;
            let e = &network.edges[cur_edge];
            let reversed = e.nodes[0] != cur_node;
            chain.push((cur_edge, reversed));
            let next_node = if reversed { e.nodes[0] } else { e.nodes[1] };
            match deg[next_node].iter().copied().find(|&n| n != cur_edge) {
                Some(ne) if !visited[ne] => {
                    cur_node = next_node;
                    cur_edge = ne;
                }
                _ => break,
            }
        }
        let min_pos = (0..chain.len())
            .min_by_key(|&i| edge_key(chain[i].0))
            .unwrap_or(0);
        chain.rotate_left(min_pos);
        if chain[0].1 {
            chain[1..].reverse();
            for (_, rev) in &mut chain {
                *rev = !*rev;
            }
        }
        runs.push(finish_run(chain, true));
    }
    runs.sort_by(|a, b| {
        a.sort_key
            .cmp(&b.sort_key)
            .then_with(|| a.total_len.total_cmp(&b.total_len))
            .then_with(|| a.edges[0].edge_idx.cmp(&b.edges[0].edge_idx))
    });
    let mut edge_run = vec![None; network.edges.len()];
    for (run_id, run) in runs.iter().enumerate() {
        for (pos, re) in run.edges.iter().enumerate() {
            edge_run[re.edge_idx] = Some((run_id, pos));
        }
    }
    (runs, edge_run)
}
