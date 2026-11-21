//! Match crossing through-train counts to junction-free heavy-rail runs on the global graph.
use super::{
    merge::{STATUS_ESTIMATED, STATUS_UNKNOWN},
    network::{Edge, Network},
    square_intervals::Interval,
    types::{Crossing, CrossingTotal},
};
use crate::osm::geometry::wrapped_longitude_delta;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

const MATCH_GATE_M: f64 = 60.0;
const LONGITUDINAL_MARGIN_M: f64 = 25.0;
/// Cosine of 25 degrees: parallel corridor heading tolerance at crossings.
const MIN_HEADING_COS: f64 = 0.906_307_787;
/// Lateral separation below which two edges belong to the same physical track slot.
const TRACK_SEP_M: f64 = 2.5;
/// Maximum lateral gap between adjacent tracks of the same crossing corridor.
const MAX_ADJACENT_TRACK_GAP_M: f64 = 15.0;
const GRID_CELL_DEG: f64 = 0.005;

// Cross-section sibling gates matching `parallel_geometry.rs` on the global graph.
const TOKENLESS_LATERAL_M: f64 = 15.0;
const TOKENLESS_MIN_COS: f64 = 0.984_807_753; // cos(10 deg)
const SAME_TOKEN_LATERAL_M: f64 = 50.0;
const SAME_TOKEN_MIN_COS: f64 = 0.939_692_621; // cos(20 deg)
const MIN_OVERLAP_M: f64 = 30.0;
const MIN_OVERLAP_FRACTION: f64 = 0.3;

fn is_heavy_main(edge: &Edge) -> bool {
    !edge.traversal_only && edge.rail_type == 0
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

/// Spatial index rasterizing every heavy-rail running segment across all cells it crosses
/// so long straight rural segments are found even when their endpoints lie kilometres away.
struct EdgeGrid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl EdgeGrid {
    fn build(network: &Network) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (edge_idx, edge) in network.edges.iter().enumerate() {
            if !is_heavy_main(edge) {
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

/// Union heavy-rail running edges at every node where exactly two running edges meet.
/// Crossovers (`traversal_only`), sidings, and square cuts do not break a corridor run;
/// switches with three or more heavy-rail running branches do.
fn build_runs(network: &Network) -> Vec<usize> {
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
            if is_heavy_main(edge) && edge.nodes[0] != edge.nodes[1] {
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

struct LocalSegment {
    edge_idx: usize,
    run_id: usize,
    way: i64,
    segment: i16,
    nodes: [usize; 2],
    dist: f64,
    a: [f64; 2],
    b: [f64; 2],
    dir: [f64; 2],
}

fn to_local_m(origin: [f64; 2], cos_lat: f64, p: [f64; 2]) -> [f64; 2] {
    [
        wrapped_longitude_delta(origin[1], p[1]) * 111_320.0 * cos_lat,
        (p[0] - origin[0]) * 110_540.0,
    ]
}

fn project_origin(
    origin: [f64; 2],
    edge: &Edge,
    edge_idx: usize,
    run_id: usize,
) -> Option<LocalSegment> {
    let cos_lat = origin[0].to_radians().cos().max(0.05);
    let a = to_local_m(origin, cos_lat, edge.points[0]);
    let b = to_local_m(origin, cos_lat, edge.points[1]);
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return None;
    }
    let frac = (-(a[0] * dx + a[1] * dy) / (len * len)).clamp(0.0, 1.0);
    let dist = (a[0] + frac * dx).hypot(a[1] + frac * dy);
    if dist > MATCH_GATE_M {
        return None;
    }
    Some(LocalSegment {
        edge_idx,
        run_id,
        way: edge.way,
        segment: edge.segment,
        nodes: edge.nodes,
        dist,
        a,
        b,
        dir: [dx / len, dy / len],
    })
}

#[derive(Clone, Copy)]
struct RunObservation<'a> {
    crossing_id: &'a str,
    periods_per_track: [f64; 3],
    total_per_track: f64,
    passenger_per_track: Option<f64>,
    measured_periods: bool,
    matched_tracks: u8,
}

fn match_crossing_slots(
    crossing: &Crossing,
    network: &Network,
    grid: &EdgeGrid,
    runs: &[usize],
) -> Vec<usize> {
    let origin = [crossing.lat, crossing.lon];
    let local: Vec<LocalSegment> = grid
        .candidates(origin, MATCH_GATE_M)
        .into_iter()
        .filter_map(|edge_idx| {
            project_origin(origin, &network.edges[edge_idx], edge_idx, runs[edge_idx])
        })
        .collect();
    let Some(anchor) = local.iter().min_by(|a, b| {
        a.dist
            .total_cmp(&b.dist)
            .then_with(|| (a.way, a.segment, a.edge_idx).cmp(&(b.way, b.segment, b.edge_idx)))
    }) else {
        return Vec::new();
    };
    let u = anchor.dir;
    let n = [-u[1], u[0]];
    let mut aligned: Vec<(&LocalSegment, f64, bool)> = local
        .iter()
        .filter_map(|seg| {
            let cos_angle = (seg.dir[0] * u[0] + seg.dir[1] * u[1]).abs();
            if cos_angle < MIN_HEADING_COS {
                return None;
            }
            let sa = seg.a[0] * u[0] + seg.a[1] * u[1];
            let sb = seg.b[0] * u[0] + seg.b[1] * u[1];
            let (a0, a1) = (sa.min(sb), sa.max(sb));
            if a0 > LONGITUDINAL_MARGIN_M || a1 < -LONGITUDINAL_MARGIN_M {
                return None;
            }
            // Evaluate lateral offset at the point on the segment closest to the crossing
            // cross-section (s = 0 along u) so diverging turnouts collapse onto their switch.
            let t_zero = if (sb - sa).abs() > 1e-6 {
                (-sa / (sb - sa)).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let px = seg.a[0] + t_zero * (seg.b[0] - seg.a[0]);
            let py = seg.a[1] + t_zero * (seg.b[1] - seg.a[1]);
            let lateral = px * n[0] + py * n[1];
            let spans_zero = a0 <= 0.0 && a1 >= 0.0;
            Some((seg, lateral, spans_zero))
        })
        .collect();
    aligned.sort_by(|(a, la, sa), (b, lb, sb)| {
        (!sa)
            .cmp(&(!sb))
            .then_with(|| a.dist.total_cmp(&b.dist))
            .then_with(|| la.abs().total_cmp(&lb.abs()))
            .then_with(|| (a.way, a.segment, a.edge_idx).cmp(&(b.way, b.segment, b.edge_idx)))
    });
    let max_slots = usize::from(crossing.main_tracks.unwrap_or(1).max(1));
    let mut slots: Vec<(usize, f64, [usize; 2])> = Vec::with_capacity(max_slots);
    for (seg, lateral, _) in aligned {
        if slots.len() >= max_slots {
            break;
        }
        // Reject diverging branches sharing a switch node with an accepted track, and
        // collapse edges within TRACK_SEP_M onto the same physical track slot. Distinct
        // parallel legs of a balloon loop (>= TRACK_SEP_M apart, no shared node) keep
        // separate slots so the crossing count divides across both legs.
        if slots.iter().any(|&(_, slot_lat, slot_nodes)| {
            (lateral - slot_lat).abs() < TRACK_SEP_M || shares_node(slot_nodes, seg.nodes)
        }) {
            continue;
        }
        if !slots.is_empty()
            && !slots
                .iter()
                .any(|&(_, slot_lat, _)| (lateral - slot_lat).abs() <= MAX_ADJACENT_TRACK_GAP_M)
        {
            continue;
        }
        slots.push((seg.run_id, lateral, seg.nodes));
    }
    slots.into_iter().map(|(run_id, _, _)| run_id).collect()
}

fn aggregate_run(mut obs: Vec<RunObservation<'_>>) -> CrossingTotal {
    if obs.iter().any(|item| item.total_per_track > 0.0) {
        obs.retain(|item| item.total_per_track > 0.0);
    }
    obs.sort_by(|a, b| {
        a.total_per_track
            .total_cmp(&b.total_per_track)
            .then_with(|| {
                a.passenger_per_track
                    .is_none()
                    .cmp(&b.passenger_per_track.is_none())
            })
            .then_with(|| a.crossing_id.cmp(b.crossing_id))
    });
    let chosen = obs[obs.len() / 2];
    CrossingTotal {
        periods_per_track: chosen.periods_per_track,
        passenger_per_track: chosen.passenger_per_track,
        measured_periods: chosen.measured_periods,
        matched_tracks: chosen.matched_tracks,
    }
}

fn edge_sibling_limit(a: &Edge, dir_a: [f64; 2], b: &Edge, dir_b: [f64; 2]) -> Option<f64> {
    if a.way == b.way
        || a.rail_type != b.rail_type
        || usage_family(a.usage) != usage_family(b.usage)
        || shares_node(a.nodes, b.nodes)
    {
        return None;
    }
    let same_token = !a.corridor.is_empty() && a.corridor == b.corridor;
    if !same_token && !a.corridor.is_empty() && !b.corridor.is_empty() {
        return None;
    }
    let (max_lateral, min_cos) = if same_token {
        (SAME_TOKEN_LATERAL_M, SAME_TOKEN_MIN_COS)
    } else {
        (TOKENLESS_LATERAL_M, TOKENLESS_MIN_COS)
    };
    ((dir_a[0] * dir_b[0] + dir_a[1] * dir_b[1]).abs() >= min_cos).then_some(max_lateral)
}

fn global_edge_crossing(
    edge_idx: usize,
    network: &Network,
    runs: &[usize],
    resolved: &HashMap<usize, CrossingTotal>,
    grid: &EdgeGrid,
    way_edges: &HashMap<i64, Vec<usize>>,
) -> Option<CrossingTotal> {
    let edge = &network.edges[edge_idx];
    let mid = edge.midpoint();
    let cos_lat = mid[0].to_radians().cos().max(0.05);
    let a0 = to_local_m(mid, cos_lat, edge.points[0]);
    let a1 = to_local_m(mid, cos_lat, edge.points[1]);
    let span = (a1[0] - a0[0]).hypot(a1[1] - a0[1]);
    if span < 1e-3 {
        return resolved.get(&runs[edge_idx]).copied();
    }
    let dir_a = [(a1[0] - a0[0]) / span, (a1[1] - a0[1]) / span];
    let search_radius = if edge.corridor.is_empty() {
        TOKENLESS_LATERAL_M
    } else {
        SAME_TOKEN_LATERAL_M
    };
    let mut nearest: HashMap<i64, (f64, usize)> = HashMap::new();
    for other_idx in grid.candidates(mid, search_radius) {
        let other = &network.edges[other_idx];
        let b0 = to_local_m(mid, cos_lat, other.points[0]);
        let b1 = to_local_m(mid, cos_lat, other.points[1]);
        let blen = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
        if blen < 1e-3 {
            continue;
        }
        let dir_b = [(b1[0] - b0[0]) / blen, (b1[1] - b0[1]) / blen];
        let Some(limit) = edge_sibling_limit(edge, dir_a, other, dir_b) else {
            continue;
        };
        let t = -(b0[0] * dir_b[0] + b0[1] * dir_b[1]) / blen;
        if !(-0.02..=1.02).contains(&t) {
            continue;
        }
        let tc = t.clamp(0.0, 1.0);
        let lateral = (b0[0] + tc * (b1[0] - b0[0])).hypot(b0[1] + tc * (b1[1] - b0[1]));
        if lateral >= limit {
            continue;
        }
        let entry = nearest.entry(other.way).or_insert((lateral, other_idx));
        if lateral < entry.0 {
            *entry = (lateral, other_idx);
        }
    }
    let s_min = (a0[0] * dir_a[0] + a0[1] * dir_a[1]).min(a1[0] * dir_a[0] + a1[1] * dir_a[1]);
    let s_max = (a0[0] * dir_a[0] + a0[1] * dir_a[1]).max(a1[0] * dir_a[0] + a1[1] * dir_a[1]);
    let min_overlap = MIN_OVERLAP_M
        .min(0.8 * span)
        .max(MIN_OVERLAP_FRACTION * span);
    let mut siblings = Vec::new();
    for (other_way, (_, rep_idx)) in nearest {
        let mut spans: Vec<(f64, f64)> = way_edges
            .get(&other_way)
            .into_iter()
            .flatten()
            .filter_map(|&idx| {
                let other = &network.edges[idx];
                let b0 = to_local_m(mid, cos_lat, other.points[0]);
                let b1 = to_local_m(mid, cos_lat, other.points[1]);
                let blen = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
                if blen < 1e-3 {
                    return None;
                }
                let dir_b = [(b1[0] - b0[0]) / blen, (b1[1] - b0[1]) / blen];
                let limit = edge_sibling_limit(edge, dir_a, other, dir_b)?;
                let sep = (b0[0] * dir_a[1] - b0[1] * dir_a[0]).abs();
                if sep >= limit {
                    return None;
                }
                let sb0 = b0[0] * dir_a[0] + b0[1] * dir_a[1];
                let sb1 = b1[0] * dir_a[0] + b1[1] * dir_a[1];
                let lo = sb0.min(sb1).max(s_min);
                let hi = sb0.max(sb1).min(s_max);
                (hi > lo).then_some((lo, hi))
            })
            .collect();
        if spans.is_empty() {
            continue;
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
        overlap += cur.1 - cur.0;
        if overlap >= min_overlap {
            siblings.push(rep_idx);
        }
    }
    let mut carriers = Vec::with_capacity(1 + siblings.len());
    if let Some(&own) = resolved.get(&runs[edge_idx]) {
        carriers.push(own);
    }
    for rep_idx in &siblings {
        if let Some(&sib) = resolved.get(&runs[*rep_idx]) {
            carriers.push(sib);
        }
    }
    if carriers.is_empty() {
        return None;
    }
    let tracks = (1 + siblings.len()) as f64;
    let mut periods_per_track = [0.0; 3];
    let mut pax_sum = 0.0;
    let mut pax_all_known = true;
    let mut measured_periods = true;
    for carrier in &carriers {
        for (sum, val) in periods_per_track.iter_mut().zip(carrier.periods_per_track) {
            *sum += val / tracks;
        }
        match carrier.passenger_per_track {
            Some(pax) => pax_sum += pax / tracks,
            None => pax_all_known = false,
        }
        measured_periods = measured_periods && carrier.measured_periods;
    }
    Some(CrossingTotal {
        periods_per_track,
        passenger_per_track: pax_all_known.then_some(pax_sum),
        measured_periods,
        matched_tracks: 1,
    })
}

pub fn match_crossings(
    network: &Network,
    crossings: &[Crossing],
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
) -> Vec<([u16; 2], Interval)> {
    let runs = build_runs(network);
    let grid = EdgeGrid::build(network);
    let mut by_run: HashMap<usize, Vec<RunObservation<'_>>> = HashMap::new();
    let mut matched_crossings = 0usize;
    let mut unmatched_crossings = 0usize;
    for crossing in crossings {
        let Some(total_periods) = crossing.total_periods else {
            continue;
        };
        let slot_runs = match_crossing_slots(crossing, network, &grid, &runs);
        if slot_runs.is_empty() {
            unmatched_crossings += 1;
            continue;
        }
        matched_crossings += 1;
        let k = slot_runs.len() as u8;
        let kf = f64::from(k);
        let periods_per_track = total_periods.map(|v| v / kf);
        let total_per_track: f64 = periods_per_track.iter().sum();
        let passenger_per_track = crossing.passenger_daily.map(|v| v / kf);
        let obs = RunObservation {
            crossing_id: &crossing.id,
            periods_per_track,
            total_per_track,
            passenger_per_track,
            measured_periods: crossing.measured_periods,
            matched_tracks: k,
        };
        let mut emitted = Vec::with_capacity(slot_runs.len());
        for run_id in slot_runs {
            if !emitted.contains(&run_id) {
                emitted.push(run_id);
                by_run.entry(run_id).or_default().push(obs);
            }
        }
    }
    let resolved: HashMap<usize, CrossingTotal> = by_run
        .into_iter()
        .map(|(run_id, obs)| (run_id, aggregate_run(obs)))
        .collect();
    let mut way_edges: HashMap<i64, Vec<usize>> = HashMap::new();
    for (edge_idx, edge) in network.edges.iter().enumerate() {
        if is_heavy_main(edge) {
            way_edges.entry(edge.way).or_default().push(edge_idx);
        }
    }
    let claims: Vec<([u16; 2], Interval)> = (0..network.edges.len())
        .into_par_iter()
        .filter_map(|edge_idx| {
            let edge = &network.edges[edge_idx];
            if !is_heavy_main(edge) || !squares.contains(&edge.square) {
                return None;
            }
            let total =
                global_edge_crossing(edge_idx, network, &runs, &resolved, &grid, &way_edges)?;
            Some((
                edge.square,
                Interval {
                    missing_calendar_estimate: [0.0; 2],
                    osm_id: edge.way,
                    segment_idx: edge.segment,
                    from_m: edge.extent[0].min(edge.extent[1]),
                    to_m: edge.extent[0].max(edge.extent[1]),
                    country,
                    source_id,
                    passenger: total.passenger_per_track.unwrap_or(0.0),
                    freight: total.periods_per_track.iter().sum(),
                    passenger_status: if total.passenger_per_track.is_some() {
                        STATUS_ESTIMATED
                    } else {
                        STATUS_UNKNOWN
                    },
                    freight_status: STATUS_ESTIMATED,
                    matching: 2,
                    stop: 0,
                    crossing: Some(total),
                },
            ))
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
