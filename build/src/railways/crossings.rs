//! Match crossing through-train counts to junction-free heavy-rail runs on the global graph.
use super::{
    merge::{STATUS_ESTIMATED, STATUS_UNKNOWN},
    network::{Edge, Network},
    square_intervals::Interval,
    types::{Crossing, CrossingTotal},
};
use crate::osm::geometry::wrapped_longitude_delta;
use std::collections::{HashMap, HashSet};

const MATCH_GATE_M: f64 = 60.0;
const LONGITUDINAL_MARGIN_M: f64 = 25.0;
/// Cosine of 25 degrees: parallel corridor heading tolerance.
const MIN_HEADING_COS: f64 = 0.906_307_787;
/// Lateral separation below which two edges belong to the same physical track slot.
const TRACK_SEP_M: f64 = 2.5;
/// Maximum lateral gap between adjacent tracks of the same crossing corridor.
const MAX_ADJACENT_TRACK_GAP_M: f64 = 15.0;
const GRID_CELL_DEG: f64 = 0.005;

fn is_heavy_main(edge: &Edge) -> bool {
    !edge.traversal_only && edge.rail_type == 0
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

    fn candidates(&self, point: [f64; 2]) -> Vec<usize> {
        let (cy, cx) = grid_cell(point);
        let dy = (MATCH_GATE_M / (GRID_CELL_DEG * 110_540.0)).ceil().max(1.0) as i32;
        let dx = (MATCH_GATE_M
            / (GRID_CELL_DEG * 111_320.0 * point[0].to_radians().cos().max(0.05)))
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
    dist: f64,
    a: [f64; 2],
    b: [f64; 2],
    closest: [f64; 2],
    dir: [f64; 2],
}

fn project_origin(
    origin: [f64; 2],
    edge: &Edge,
    edge_idx: usize,
    run_id: usize,
) -> Option<LocalSegment> {
    let cos_lat = origin[0].to_radians().cos().max(0.05);
    let to_local = |p: [f64; 2]| -> [f64; 2] {
        [
            wrapped_longitude_delta(origin[1], p[1]) * 111_320.0 * cos_lat,
            (p[0] - origin[0]) * 110_540.0,
        ]
    };
    let a = to_local(edge.points[0]);
    let b = to_local(edge.points[1]);
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return None;
    }
    let frac = (-(a[0] * dx + a[1] * dy) / (len * len)).clamp(0.0, 1.0);
    let closest = [a[0] + frac * dx, a[1] + frac * dy];
    let dist = closest[0].hypot(closest[1]);
    if dist > MATCH_GATE_M {
        return None;
    }
    Some(LocalSegment {
        edge_idx,
        run_id,
        way: edge.way,
        segment: edge.segment,
        dist,
        a,
        b,
        closest,
        dir: [dx / len, dy / len],
    })
}

#[derive(Clone, Copy)]
struct RunObservation<'a> {
    crossing_id: &'a str,
    periods_per_track: [f64; 3],
    total_per_track: f64,
    passenger_per_track: Option<f64>,
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
        .candidates(origin)
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
    let mut aligned: Vec<(&LocalSegment, f64)> = local
        .iter()
        .filter_map(|seg| {
            let cos_angle = (seg.dir[0] * u[0] + seg.dir[1] * u[1]).abs();
            if cos_angle < MIN_HEADING_COS {
                return None;
            }
            let pa = seg.a[0] * u[0] + seg.a[1] * u[1];
            let pb = seg.b[0] * u[0] + seg.b[1] * u[1];
            let (a0, a1) = (pa.min(pb), pa.max(pb));
            if a0 > LONGITUDINAL_MARGIN_M || a1 < -LONGITUDINAL_MARGIN_M {
                return None;
            }
            let lateral = seg.closest[0] * n[0] + seg.closest[1] * n[1];
            Some((seg, lateral))
        })
        .collect();
    aligned.sort_by(|(a, la), (b, lb)| {
        a.dist
            .total_cmp(&b.dist)
            .then_with(|| la.abs().total_cmp(&lb.abs()))
            .then_with(|| (a.way, a.segment, a.edge_idx).cmp(&(b.way, b.segment, b.edge_idx)))
    });
    let max_slots = usize::from(crossing.main_tracks.unwrap_or(1).max(1));
    let mut slots: Vec<(usize, f64)> = Vec::with_capacity(max_slots);
    for (seg, lateral) in aligned {
        if slots.len() >= max_slots {
            break;
        }
        if slots.iter().any(|&(run_id, _)| run_id == seg.run_id) {
            continue;
        }
        if slots
            .iter()
            .any(|&(_, slot_lat)| (lateral - slot_lat).abs() < TRACK_SEP_M)
        {
            continue;
        }
        if !slots.is_empty()
            && !slots
                .iter()
                .any(|&(_, slot_lat)| (lateral - slot_lat).abs() <= MAX_ADJACENT_TRACK_GAP_M)
        {
            continue;
        }
        slots.push((seg.run_id, lateral));
    }
    slots.into_iter().map(|(run_id, _)| run_id).collect()
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
        matched_tracks: chosen.matched_tracks,
    }
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
            matched_tracks: k,
        };
        for run_id in slot_runs {
            by_run.entry(run_id).or_default().push(obs);
        }
    }
    let resolved: HashMap<usize, CrossingTotal> = by_run
        .into_iter()
        .map(|(run_id, obs)| (run_id, aggregate_run(obs)))
        .collect();
    let mut claims = Vec::new();
    for (edge_idx, edge) in network.edges.iter().enumerate() {
        if !is_heavy_main(edge) || !squares.contains(&edge.square) {
            continue;
        }
        let Some(&total) = resolved.get(&runs[edge_idx]) else {
            continue;
        };
        claims.push((
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
        ));
    }
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
