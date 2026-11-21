//! Match crossing through-train counts and allocate crossing-backed corridors once on the global graph.
use super::{
    matcher::Claims,
    merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_UNKNOWN, row_evidence},
    model::RailModel,
    network::{Edge, Network, project},
    parallel_tracks::{scope_value, scope_winner, subtract_passenger_periods},
    square_intervals::{Interval, grouped},
    types::Crossing,
};
use crate::osm::geometry::wrapped_longitude_delta;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

const MATCH_GATE_M: f64 = 60.0;
/// Lateral separation below which two edges occupy the same physical track slot.
const TRACK_SEP_M: f64 = 2.5;
const GRID_CELL_DEG: f64 = 0.005;

// Cross-section sibling gates matching `parallel_geometry.rs` on the global graph.
const TOKENLESS_LATERAL_M: f64 = 15.0;
const TOKENLESS_MIN_COS: f64 = 0.984_807_753; // cos(10 deg)
const SAME_TOKEN_LATERAL_M: f64 = 50.0;
const SAME_TOKEN_MIN_COS: f64 = 0.939_692_621; // cos(20 deg)
const MIN_OVERLAP_M: f64 = 30.0;
const MIN_OVERLAP_FRACTION: f64 = 0.3;

fn is_heavy_main(edge: &Edge, country: [u8; 2]) -> bool {
    !edge.traversal_only && edge.rail_type == 0 && edge.country == country
}

fn usage_family(usage: u8) -> u8 {
    if usage == 3 { 0 } else { usage }
}

fn shares_node(a: [usize; 2], b: [usize; 2]) -> bool {
    a[0] == b[0] || a[0] == b[1] || a[1] == b[0] || a[1] == b[1]
}

fn grid_cell(point: [f64; 2]) -> (i32, i32) {
    (
        (point[0] / GRID_CELL_DEG).floor() as i32,
        (point[1] / GRID_CELL_DEG).floor() as i32,
    )
}

fn to_local_m(origin: [f64; 2], cos_lat: f64, p: [f64; 2]) -> [f64; 2] {
    [
        wrapped_longitude_delta(origin[1], p[1]) * 111_320.0 * cos_lat,
        (p[0] - origin[0]) * 110_540.0,
    ]
}

/// Spatial index rasterizing every heavy-rail running segment of `country` across all cells it
/// crosses so long straight rural segments are found even when endpoints lie kilometres away.
struct EdgeGrid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl EdgeGrid {
    fn build(network: &Network, country: [u8; 2]) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (edge_idx, edge) in network.edges.iter().enumerate() {
            if !is_heavy_main(edge, country) {
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

    fn candidates(&self, point: [f64; 2], radius_m: f64) -> Vec<usize> {
        let (cy, cx) = grid_cell(point);
        let dy = (radius_m / (GRID_CELL_DEG * 110_540.0)).ceil().max(1.0) as i32;
        let dx = (radius_m / (GRID_CELL_DEG * 111_320.0 * point[0].to_radians().cos().max(0.05)))
            .ceil()
            .max(1.0) as i32;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for oy in -dy..=dy {
            for ox in -dx..=dx {
                if let Some(bucket) = self.cells.get(&(cy + oy, cx + ox)) {
                    for &edge_idx in bucket {
                        if seen.insert(edge_idx) {
                            out.push(edge_idx);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Union heavy-rail running edges of `country` at every node where exactly two meet.
/// Crossovers (`traversal_only`), sidings, and square cuts do not break a run; switches with
/// three or more heavy-rail running branches and country borders do.
fn build_runs(network: &Network, country: [u8; 2]) -> Vec<usize> {
    let n = network.edges.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut incident = Vec::with_capacity(4);
    for edges in &network.adjacency {
        incident.clear();
        for &edge_idx in edges {
            let edge = &network.edges[edge_idx];
            if is_heavy_main(edge, country) && edge.nodes[0] != edge.nodes[1] {
                incident.push(edge_idx);
            }
        }
        if incident.len() == 2 {
            let a = find(&mut parent, incident[0]);
            let b = find(&mut parent, incident[1]);
            if a != b {
                parent[b] = a;
            }
        }
    }
    (0..n).map(|idx| find(&mut parent, idx)).collect()
}

/// Shared corridor compatibility rule used for both crossing slotting and cross-section grouping.
/// Two ways that share a switch node must stay within the tighter 10-degree heading gate so
/// parallel tracks converging at a switch ahead stay paired while diverging turnouts are rejected.
fn corridor_gate(a: &Edge, dir_a: [f64; 2], b: &Edge, dir_b: [f64; 2]) -> Option<f64> {
    if a.way == b.way
        || a.rail_type != b.rail_type
        || usage_family(a.usage) != usage_family(b.usage)
        || a.country != b.country
    {
        return None;
    }
    let same_token = !a.corridor.is_empty() && a.corridor == b.corridor;
    if !same_token && !a.corridor.is_empty() && !b.corridor.is_empty() {
        return None;
    }
    let max_lateral = if same_token {
        SAME_TOKEN_LATERAL_M
    } else {
        TOKENLESS_LATERAL_M
    };
    let min_cos = if !same_token || shares_node(a.nodes, b.nodes) {
        TOKENLESS_MIN_COS
    } else {
        SAME_TOKEN_MIN_COS
    };
    ((dir_a[0] * dir_b[0] + dir_a[1] * dir_b[1]).abs() >= min_cos).then_some(max_lateral)
}

fn not_coincident(b0: [f64; 2], b1: [f64; 2], dir_a: [f64; 2]) -> bool {
    let sep0 = (b0[0] * dir_a[1] - b0[1] * dir_a[0]).abs();
    let sep1 = (b1[0] * dir_a[1] - b1[1] * dir_a[0]).abs();
    sep0.max(sep1) >= TRACK_SEP_M
}

struct LocalFrame {
    mid: [f64; 2],
    cos_lat: f64,
    dir: [f64; 2],
    normal: [f64; 2],
    s_min: f64,
    s_max: f64,
    min_overlap: f64,
}

impl LocalFrame {
    fn of(edge: &Edge) -> Option<Self> {
        let mid = edge.midpoint();
        let cos_lat = mid[0].to_radians().cos().max(0.05);
        let a0 = to_local_m(mid, cos_lat, edge.points[0]);
        let a1 = to_local_m(mid, cos_lat, edge.points[1]);
        let span = (a1[0] - a0[0]).hypot(a1[1] - a0[1]);
        if span < 1e-3 {
            return None;
        }
        let dir = [(a1[0] - a0[0]) / span, (a1[1] - a0[1]) / span];
        let s0 = a0[0] * dir[0] + a0[1] * dir[1];
        let s1 = a1[0] * dir[0] + a1[1] * dir[1];
        Some(Self {
            mid,
            cos_lat,
            dir,
            normal: [-dir[1], dir[0]],
            s_min: s0.min(s1),
            s_max: s0.max(s1),
            min_overlap: MIN_OVERLAP_M
                .min(0.8 * span)
                .max(MIN_OVERLAP_FRACTION * span),
        })
    }

    fn overlap_along(
        &self,
        edge: &Edge,
        candidates: &[usize],
        network: &Network,
        lateral_gate: bool,
    ) -> f64 {
        let mut spans: Vec<(f64, f64)> = candidates
            .iter()
            .filter_map(|&idx| {
                let other = &network.edges[idx];
                if other.way == edge.way {
                    return None;
                }
                let b0 = to_local_m(self.mid, self.cos_lat, other.points[0]);
                let b1 = to_local_m(self.mid, self.cos_lat, other.points[1]);
                let blen = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
                if blen < 1e-3 {
                    return None;
                }
                let dir_b = [(b1[0] - b0[0]) / blen, (b1[1] - b0[1]) / blen];
                let limit = corridor_gate(edge, self.dir, other, dir_b)?;
                if lateral_gate {
                    let mid_b = [(b0[0] + b1[0]) * 0.5, (b0[1] + b1[1]) * 0.5];
                    let sep = (mid_b[0] * self.dir[1] - mid_b[1] * self.dir[0]).abs();
                    if sep >= limit {
                        return None;
                    }
                }
                let sb0 = b0[0] * self.dir[0] + b0[1] * self.dir[1];
                let sb1 = b1[0] * self.dir[0] + b1[1] * self.dir[1];
                let lo = sb0.min(sb1).max(self.s_min);
                let hi = sb0.max(sb1).min(self.s_max);
                (hi > lo).then_some((lo, hi))
            })
            .collect();
        if spans.is_empty() {
            return 0.0;
        }
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut overlap = 0.0;
        let mut cur = spans[0];
        for (lo, hi) in spans.into_iter().skip(1) {
            if lo <= cur.1 {
                cur.1 = cur.1.max(hi);
            } else {
                overlap += cur.1 - cur.0;
                cur = (lo, hi);
            }
        }
        overlap + (cur.1 - cur.0)
    }
}

#[derive(Clone, Copy)]
struct NeighborCandidate {
    lateral: f64,
    key: (i64, i16, usize),
    signed_lat: f64,
    edge_idx: usize,
}

fn direct_neighbors_of(
    edge_idx: usize,
    network: &Network,
    country: [u8; 2],
    runs: &[usize],
    run_edges: &HashMap<usize, Vec<usize>>,
    grid: &EdgeGrid,
) -> Vec<usize> {
    let edge = &network.edges[edge_idx];
    if !is_heavy_main(edge, country) {
        return Vec::new();
    }
    let Some(frame) = LocalFrame::of(edge) else {
        return Vec::new();
    };
    let search_radius = if edge.corridor.is_empty() {
        TOKENLESS_LATERAL_M
    } else {
        SAME_TOKEN_LATERAL_M
    };
    let mut nearest: HashMap<i64, NeighborCandidate> = HashMap::new();
    for other_idx in grid.candidates(frame.mid, search_radius) {
        let other = &network.edges[other_idx];
        let b0 = to_local_m(frame.mid, frame.cos_lat, other.points[0]);
        let b1 = to_local_m(frame.mid, frame.cos_lat, other.points[1]);
        let blen = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
        if blen < 1e-3 {
            continue;
        }
        let dir_b = [(b1[0] - b0[0]) / blen, (b1[1] - b0[1]) / blen];
        let Some(limit) = corridor_gate(edge, frame.dir, other, dir_b) else {
            continue;
        };
        if !not_coincident(b0, b1, frame.dir) {
            continue;
        }
        let t = -(b0[0] * dir_b[0] + b0[1] * dir_b[1]) / blen;
        if !(-0.02..=1.02).contains(&t) {
            continue;
        }
        let tc = t.clamp(0.0, 1.0);
        let cp = [b0[0] + tc * (b1[0] - b0[0]), b0[1] + tc * (b1[1] - b0[1])];
        let lateral = cp[0].hypot(cp[1]);
        if lateral >= limit {
            continue;
        }
        let signed_lat = cp[0] * frame.normal[0] + cp[1] * frame.normal[1];
        let cand = NeighborCandidate {
            lateral,
            key: (other.way, other.segment, other_idx),
            signed_lat,
            edge_idx: other_idx,
        };
        let entry = nearest.entry(other.way).or_insert(cand);
        if (cand.lateral, cand.key) < (entry.lateral, entry.key) {
            *entry = cand;
        }
    }
    let mut survivors: Vec<NeighborCandidate> = nearest
        .into_values()
        .filter(|cand| {
            let cands = run_edges
                .get(&runs[cand.edge_idx])
                .map(Vec::as_slice)
                .unwrap_or_default();
            frame.overlap_along(edge, cands, network, true) >= frame.min_overlap
        })
        .collect();
    survivors.sort_by(|a, b| {
        a.lateral
            .total_cmp(&b.lateral)
            .then_with(|| a.key.cmp(&b.key))
    });
    let mut kept_lats: Vec<f64> = Vec::with_capacity(survivors.len());
    let mut out = Vec::with_capacity(survivors.len());
    for cand in survivors {
        if kept_lats
            .iter()
            .any(|&kept| (cand.signed_lat - kept).abs() < TRACK_SEP_M)
        {
            continue;
        }
        kept_lats.push(cand.signed_lat);
        out.push(cand.edge_idx);
    }
    out.sort_by_key(|&idx| network.edges[idx].way);
    out
}

/// Transitive closure over ways on the global graph, keeping one representative per physical track
/// abreast of `edge_idx`'s midpoint.
fn expand_section(
    edge_idx: usize,
    network: &Network,
    runs: &[usize],
    run_edges: &HashMap<usize, Vec<usize>>,
    way_edges: &HashMap<i64, Vec<usize>>,
    direct_neighbors: &[Vec<usize>],
) -> Vec<usize> {
    let edge = &network.edges[edge_idx];
    let Some(frame) = LocalFrame::of(edge) else {
        return vec![edge_idx];
    };
    let mut seen_ways = HashSet::from([edge.way]);
    let mut visited_members = HashSet::from([edge_idx]);
    let mut queue = VecDeque::from([edge_idx]);
    let mut grown = vec![edge_idx];
    let mut kept_lats: Vec<f64> = Vec::new();
    while let Some(member) = queue.pop_front() {
        for &other_idx in &direct_neighbors[member] {
            let other_way = network.edges[other_idx].way;
            if !seen_ways.insert(other_way) {
                continue;
            }
            let Some((cand_idx, signed_lat)) = best_way_representative(
                edge,
                &frame,
                way_edges
                    .get(&other_way)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                network,
            ) else {
                continue;
            };
            let cands = run_edges
                .get(&runs[cand_idx])
                .map(Vec::as_slice)
                .unwrap_or_default();
            if frame.overlap_along(edge, cands, network, false) < frame.min_overlap {
                continue;
            }
            if kept_lats
                .iter()
                .any(|&kept| (signed_lat - kept).abs() < TRACK_SEP_M)
            {
                continue;
            }
            kept_lats.push(signed_lat);
            grown.push(cand_idx);
            if visited_members.insert(cand_idx) {
                queue.push_back(cand_idx);
            }
            if visited_members.insert(other_idx) {
                queue.push_back(other_idx);
            }
        }
    }
    grown[1..].sort_by_key(|&idx| network.edges[idx].way);
    grown
}

#[derive(Clone, Copy)]
struct RepCandidate {
    abreast: bool,
    dist: f64,
    key: (i64, i16, usize),
    signed_lat: f64,
    edge_idx: usize,
}

fn best_way_representative(
    edge: &Edge,
    frame: &LocalFrame,
    way_candidates: &[usize],
    network: &Network,
) -> Option<(usize, f64)> {
    let mut best: Option<RepCandidate> = None;
    for &idx in way_candidates {
        let cand = &network.edges[idx];
        let b0 = to_local_m(frame.mid, frame.cos_lat, cand.points[0]);
        let b1 = to_local_m(frame.mid, frame.cos_lat, cand.points[1]);
        let blen = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
        if blen < 1e-3 {
            continue;
        }
        let dir_b = [(b1[0] - b0[0]) / blen, (b1[1] - b0[1]) / blen];
        if corridor_gate(edge, frame.dir, cand, dir_b).is_none()
            || !not_coincident(b0, b1, frame.dir)
        {
            continue;
        }
        let t = -(b0[0] * dir_b[0] + b0[1] * dir_b[1]) / blen;
        let abreast = (-0.02..=1.02).contains(&t);
        let tc = t.clamp(0.0, 1.0);
        let cp = [b0[0] + tc * (b1[0] - b0[0]), b0[1] + tc * (b1[1] - b0[1])];
        let dist = cp[0].hypot(cp[1]);
        let signed_lat = cp[0] * frame.normal[0] + cp[1] * frame.normal[1];
        let key = (cand.way, cand.segment, idx);
        let better = best.is_none_or(|b| {
            (!abreast)
                .cmp(&(!b.abreast))
                .then_with(|| dist.total_cmp(&b.dist))
                .then_with(|| key.cmp(&b.key))
                .is_lt()
        });
        if better {
            best = Some(RepCandidate {
                abreast,
                dist,
                key,
                signed_lat,
                edge_idx: idx,
            });
        }
    }
    best.map(|b| (b.edge_idx, b.signed_lat))
}

fn nearest_edge(origin: [f64; 2], network: &Network, grid: &EdgeGrid) -> Option<usize> {
    let cos_lat = origin[0].to_radians().cos().max(0.05);
    let mut best: Option<(f64, (i64, i16, usize), usize)> = None;
    for edge_idx in grid.candidates(origin, MATCH_GATE_M) {
        let edge = &network.edges[edge_idx];
        let a = to_local_m(origin, cos_lat, edge.points[0]);
        let b = to_local_m(origin, cos_lat, edge.points[1]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let len = dx.hypot(dy);
        if len < 1e-3 {
            continue;
        }
        let frac = (-(a[0] * dx + a[1] * dy) / (len * len)).clamp(0.0, 1.0);
        let dist = (a[0] + frac * dx).hypot(a[1] + frac * dy);
        if dist > MATCH_GATE_M {
            continue;
        }
        let key = (edge.way, edge.segment, edge_idx);
        if best.is_none_or(|(bd, bk, _)| dist < bd || (dist == bd && key < bk)) {
            best = Some((dist, key, edge_idx));
        }
    }
    best.map(|(_, _, idx)| idx)
}

#[derive(Clone, Copy)]
struct RunObservation<'a> {
    crossing_id: &'a str,
    measured_periods: [f64; 3],
    unsplit_daily: f64,
    total: f64,
    passenger: Option<f64>,
    has_measured: bool,
}

#[derive(Clone, Copy)]
struct RunTotal {
    measured_periods: [f64; 3],
    unsplit_daily: f64,
    passenger: Option<f64>,
}

fn aggregate_run(mut obs: Vec<RunObservation<'_>>) -> RunTotal {
    if obs.iter().any(|item| item.total > 0.0) {
        obs.retain(|item| item.total > 0.0);
    }
    obs.sort_by(|a, b| {
        a.total
            .total_cmp(&b.total)
            // Upper median picks len/2; sort missing passenger splits first so known splits win ties.
            .then_with(|| b.passenger.is_none().cmp(&a.passenger.is_none()))
            // Sort unsplit weekly observations first so measured period splits win ties.
            .then_with(|| a.has_measured.cmp(&b.has_measured))
            .then_with(|| a.crossing_id.cmp(b.crossing_id))
    });
    let chosen = obs[obs.len() / 2];
    RunTotal {
        measured_periods: chosen.measured_periods,
        unsplit_daily: chosen.unsplit_daily,
        passenger: chosen.passenger,
    }
}

fn higher_rank_freight_wins(
    model: &RailModel,
    country: [u8; 2],
    source_id: u16,
    idx_vec: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
) -> bool {
    let dom = scope_winner(model, country, idx_vec, domestic, false, |t| t.freight);
    if dom != 0
        && dom != source_id
        && !model.is_residual(dom)
        && model.should_overwrite(source_id, dom)
    {
        return true;
    }
    let for_ = scope_winner(model, country, idx_vec, foreign, false, |t| t.freight);
    for_ != 0
        && for_ != source_id
        && !model.is_residual(for_)
        && !model.blocks_foreign_national(source_id, for_)
        && model.should_overwrite(source_id, for_)
}

fn winning_timetable(
    model: &RailModel,
    country: [u8; 2],
    source_id: u16,
    idx_vec: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
) -> Option<CategoryFlow> {
    let dom = scope_winner(model, country, idx_vec, domestic, true, |t| t.passenger);
    if dom != 0 && !model.is_residual(dom) && model.should_overwrite(source_id, dom) {
        return Some(scope_value(model, idx_vec, domestic, dom, |t| t.passenger));
    }
    let for_ = scope_winner(model, country, idx_vec, foreign, true, |t| t.passenger);
    if for_ != 0
        && !model.is_residual(for_)
        && !model.blocks_foreign_national(source_id, for_)
        && model.should_overwrite(source_id, for_)
    {
        return Some(scope_value(model, idx_vec, foreign, for_, |t| t.passenger));
    }
    None
}

fn point_on_edge(edge: &Edge, distance_m: f64) -> [f64; 2] {
    let span = edge.extent[1] - edge.extent[0];
    let frac = if span.abs() > 1e-6 {
        ((distance_m - edge.extent[0]) / span).clamp(0.0, 1.0)
    } else {
        0.5
    };
    [
        edge.points[0][0] + frac * (edge.points[1][0] - edge.points[0][0]),
        edge.points[0][1] + frac * (edge.points[1][1] - edge.points[0][1]),
    ]
}

fn query_window_for_sibling(
    edge: &Edge,
    mid_s: f64,
    sib: &Edge,
    sib_ivs: &[Interval],
) -> (f64, f64) {
    let (s_lo, s_hi) = (
        sib.extent[0].min(sib.extent[1]),
        sib.extent[0].max(sib.extent[1]),
    );
    if s_hi - s_lo <= 1e-4 {
        return (s_lo, s_hi);
    }
    let pt = point_on_edge(edge, mid_s);
    let (_, frac) = project(pt, sib.points[0], sib.points[1]);
    let mut p = (sib.extent[0] + frac * (sib.extent[1] - sib.extent[0])).clamp(s_lo, s_hi);
    let near_cut = sib_ivs
        .iter()
        .any(|iv| (iv.from_m - p).abs() < 1e-4 || (iv.to_m - p).abs() < 1e-4);
    if near_cut {
        let step = (0.25 * (s_hi - s_lo)).min(1e-3);
        p = if p + step < s_hi {
            p + step
        } else {
            (p - step).max(s_lo)
        };
    }
    ((p - 1e-6).max(s_lo), (p + 1e-6).min(s_hi))
}

#[allow(clippy::too_many_arguments)]
fn resolve_edge_intervals(
    edge_idx: usize,
    model: &RailModel,
    network: &Network,
    runs: &[usize],
    run_edges: &HashMap<usize, Vec<usize>>,
    way_edges: &HashMap<i64, Vec<usize>>,
    direct_neighbors: &[Vec<usize>],
    resolved: &HashMap<usize, RunTotal>,
    piece_claims: &HashMap<([u16; 2], i64, i16), Vec<Interval>>,
    source_id: u16,
    country: [u8; 2],
) -> Vec<([u16; 2], Interval)> {
    let edge = &network.edges[edge_idx];
    let members = expand_section(
        edge_idx,
        network,
        runs,
        run_edges,
        way_edges,
        direct_neighbors,
    );
    let mut seen_runs = Vec::with_capacity(members.len());
    let mut carriers = Vec::with_capacity(members.len());
    for &member in &members {
        let run_id = runs[member];
        if !seen_runs.contains(&run_id) {
            seen_runs.push(run_id);
            if let Some(&total) = resolved.get(&run_id) {
                carriers.push(total);
            }
        }
    }
    if carriers.is_empty() {
        return Vec::new();
    }
    let tracks = members.len() as f64;
    let mut measured = [0.0; 3];
    let mut unsplit = 0.0;
    let mut pax_sum = 0.0;
    let mut pax_all_known = true;
    for carrier in &carriers {
        for (sum, val) in measured.iter_mut().zip(carrier.measured_periods) {
            *sum += val / tracks;
        }
        unsplit += carrier.unsplit_daily / tracks;
        match carrier.passenger {
            Some(pax) => pax_sum += pax / tracks,
            None => pax_all_known = false,
        }
    }
    let shares = model.period_shares(country, edge.rail_type);
    let total_periods = [
        measured[0] + unsplit * shares.frt[0],
        measured[1] + unsplit * shares.frt[1],
        measured[2] + unsplit * shares.frt[2],
    ];
    let total_daily: f64 = total_periods.iter().sum();
    let has_measured = measured.iter().sum::<f64>() > 0.0;
    let inv_pax = pax_all_known.then_some(pax_sum);

    let lo = edge.extent[0].min(edge.extent[1]);
    let hi = edge.extent[0].max(edge.extent[1]);
    let mut cuts = vec![lo, hi];
    if let Some(own_ivs) = piece_claims.get(&(edge.square, edge.way, edge.segment)) {
        for iv in own_ivs {
            cuts.push(iv.from_m.clamp(lo, hi));
            cuts.push(iv.to_m.clamp(lo, hi));
        }
    }
    for &m in members.iter().skip(1) {
        let sib = &network.edges[m];
        let Some(sib_ivs) = piece_claims.get(&(sib.square, sib.way, sib.segment)) else {
            continue;
        };
        let (s_lo, s_hi) = (
            sib.extent[0].min(sib.extent[1]),
            sib.extent[0].max(sib.extent[1]),
        );
        for iv in sib_ivs {
            for cut_m in [iv.from_m, iv.to_m] {
                if cut_m > s_lo + 1e-4 && cut_m < s_hi - 1e-4 {
                    let pt = point_on_edge(sib, cut_m);
                    let (_, e_frac) = project(pt, edge.points[0], edge.points[1]);
                    let e_cut = edge.extent[0] + e_frac * (edge.extent[1] - edge.extent[0]);
                    cuts.push(e_cut.clamp(lo, hi));
                }
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    let mut unique = Vec::with_capacity(cuts.len());
    for cut in cuts {
        if unique.last().is_none_or(|prev: &f64| cut - *prev > 1e-4) {
            unique.push(cut);
        }
    }
    if unique.len() < 2 {
        unique = vec![lo, hi];
    }

    let idx_vec: Vec<usize> = (0..members.len()).collect();
    let mut domestic = Vec::with_capacity(members.len());
    let mut foreign = Vec::with_capacity(members.len());
    let mut out = Vec::with_capacity(unique.len().saturating_sub(1));
    for window in unique.windows(2) {
        let (a, b) = (window[0], window[1]);
        let mid_s = (a + b) * 0.5;
        domestic.clear();
        foreign.clear();
        for &m in &members {
            let m_edge = &network.edges[m];
            let m_ivs = piece_claims
                .get(&(m_edge.square, m_edge.way, m_edge.segment))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let (q_from, q_to) = if m == edge_idx {
                (a, b)
            } else {
                query_window_for_sibling(edge, mid_s, m_edge, m_ivs)
            };
            let (dom, for_) =
                row_evidence(model, m_ivs, q_from, q_to, m_edge.rail_type, m_edge.country);
            domestic.push(dom);
            foreign.push(for_);
        }
        if higher_rank_freight_wins(model, country, source_id, &idx_vec, &domestic, &foreign) {
            continue;
        }
        let resolved_traffic = if let Some(raw_pax) =
            winning_timetable(model, country, source_id, &idx_vec, &domestic, &foreign)
        {
            let passenger = CategoryFlow {
                periods: raw_pax.periods.map(|v| v / tracks),
                missing_calendar_estimate: raw_pax.missing_calendar_estimate.map(|v| v / tracks),
                ..raw_pax
            };
            let pax_daily: f64 = passenger.periods.iter().sum();
            let effective_pax = inv_pax.map_or(pax_daily, |inv| pax_daily.max(inv));
            let target_frt = (total_daily - effective_pax).max(0.0);
            let freight_periods = if has_measured {
                subtract_passenger_periods(total_periods, passenger.periods, target_frt)
            } else {
                shares.frt.map(|s| target_frt * s)
            };
            RowTraffic {
                passenger,
                freight: CategoryFlow {
                    periods: freight_periods,
                    missing_calendar_estimate: [0.0; 3],
                    status: STATUS_ESTIMATED,
                    source_id,
                    matching: 2,
                    stop: 0,
                },
            }
        } else if let Some(pax_daily) = inv_pax {
            let pax_periods =
                if has_measured && total_daily > 0.0 && (pax_daily - total_daily).abs() <= 1e-6 {
                    total_periods
                } else {
                    shares.pax.map(|s| pax_daily * s)
                };
            let target_frt = (total_daily - pax_daily).max(0.0);
            let freight_periods = if has_measured {
                subtract_passenger_periods(total_periods, pax_periods, target_frt)
            } else {
                shares.frt.map(|s| target_frt * s)
            };
            RowTraffic {
                passenger: CategoryFlow {
                    periods: pax_periods,
                    missing_calendar_estimate: [0.0; 3],
                    status: STATUS_ESTIMATED,
                    source_id,
                    matching: 2,
                    stop: 0,
                },
                freight: CategoryFlow {
                    periods: freight_periods,
                    missing_calendar_estimate: [0.0; 3],
                    status: STATUS_ESTIMATED,
                    source_id,
                    matching: 2,
                    stop: 0,
                },
            }
        } else {
            // Unknown passenger split without a timetable: leave the section on its class prior.
            continue;
        };
        out.push((
            edge.square,
            Interval {
                missing_calendar_estimate: [0.0; 2],
                osm_id: edge.way,
                segment_idx: edge.segment,
                from_m: a,
                to_m: b,
                country,
                source_id,
                passenger: 0.0,
                freight: 0.0,
                passenger_status: STATUS_UNKNOWN,
                freight_status: STATUS_UNKNOWN,
                matching: 2,
                stop: 0,
                resolved_section: Some(resolved_traffic),
            },
        ));
    }
    out
}

pub fn match_crossings(
    model: &RailModel,
    network: &Network,
    crossings: &[Crossing],
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
    existing_claims: &Claims,
) -> Vec<([u16; 2], Interval)> {
    let runs = build_runs(network, country);
    let grid = EdgeGrid::build(network, country);
    let mut run_edges: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut way_edges: HashMap<i64, Vec<usize>> = HashMap::new();
    for (edge_idx, edge) in network.edges.iter().enumerate() {
        if is_heavy_main(edge, country) {
            run_edges.entry(runs[edge_idx]).or_default().push(edge_idx);
            way_edges.entry(edge.way).or_default().push(edge_idx);
        }
    }
    let direct_neighbors: Vec<Vec<usize>> = (0..network.edges.len())
        .into_par_iter()
        .map(|edge_idx| direct_neighbors_of(edge_idx, network, country, &runs, &run_edges, &grid))
        .collect();

    let mut by_run: HashMap<usize, Vec<RunObservation<'_>>> = HashMap::new();
    let mut matched_crossings = 0usize;
    let mut unmatched_crossings = 0usize;
    for crossing in crossings {
        let Some(total_periods) = crossing.total_periods else {
            continue;
        };
        let Some(anchor) = nearest_edge([crossing.lat, crossing.lon], network, &grid) else {
            unmatched_crossings += 1;
            continue;
        };
        matched_crossings += 1;
        let section = expand_section(
            anchor,
            network,
            &runs,
            &run_edges,
            &way_edges,
            &direct_neighbors,
        );
        let tracks = section.len() as f64;
        let mut run_counts: Vec<(usize, usize)> = Vec::with_capacity(section.len());
        for edge_idx in section {
            let run_id = runs[edge_idx];
            if let Some((_, count)) = run_counts.iter_mut().find(|(r, _)| *r == run_id) {
                *count += 1;
            } else {
                run_counts.push((run_id, 1));
            }
        }
        let crossing_total: f64 = total_periods.iter().sum();
        for (run_id, count) in run_counts {
            let share = count as f64 / tracks;
            let measured_periods = if crossing.measured_periods {
                total_periods.map(|v| v * share)
            } else {
                [0.0; 3]
            };
            let unsplit_daily = if crossing.measured_periods {
                0.0
            } else {
                crossing_total * share
            };
            by_run.entry(run_id).or_default().push(RunObservation {
                crossing_id: &crossing.id,
                measured_periods,
                unsplit_daily,
                total: crossing_total * share,
                passenger: crossing.passenger_daily.map(|p| p * share),
                has_measured: crossing.measured_periods,
            });
        }
    }
    let resolved: HashMap<usize, RunTotal> = by_run
        .into_iter()
        .map(|(run_id, obs)| (run_id, aggregate_run(obs)))
        .collect();

    let mut piece_claims: HashMap<([u16; 2], i64, i16), Vec<Interval>> = HashMap::new();
    for (&square, rows) in existing_claims {
        for ((way, seg), intervals) in grouped(rows.clone()) {
            piece_claims.insert((square, way, seg), intervals);
        }
    }

    let claims: Vec<([u16; 2], Interval)> = (0..network.edges.len())
        .into_par_iter()
        .flat_map(|edge_idx| {
            let edge = &network.edges[edge_idx];
            if !is_heavy_main(edge, country) || !squares.contains(&edge.square) {
                return Vec::new();
            }
            resolve_edge_intervals(
                edge_idx,
                model,
                network,
                &runs,
                &run_edges,
                &way_edges,
                &direct_neighbors,
                &resolved,
                &piece_claims,
                source_id,
                country,
            )
        })
        .collect();
    eprintln!(
        "railways: crossings source {source_id}: {matched_crossings} matched, {unmatched_crossings} unmatched, {} runs, {} edge claims",
        resolved.len(),
        claims.len()
    );
    claims
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
