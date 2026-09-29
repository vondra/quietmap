//! Building-exposure receivers of CNOSSOS-EU 2.8 case 1 (Directive (EU) 2021/1226, Annex II):
//! façades are split into segments; a segment over 5 m is cut into the fewest equal intervals no
//! longer than 5 m with one receiver in the middle of each; a remaining segment over 2.5 m gets one
//! receiver in its middle; adjacent remaining segments together over 5 m are one polyline treated
//! the same way. Receivers stand 0.1 m in front of the façade (dev4 `facade_receivers.rs`; owner
//! decision 2026-09-24: a building's exposure is its noisiest receiver).
//!
//! Canonical order: rings as stored; exteriors counter-clockwise and holes clockwise in the
//! north-up metre frame (the building on the left, outward on the right); each ring from its
//! smallest (east, north) vertex, rotated to its first edge carrying receivers of its own so a run
//! of short edges is never split.

use super::{Footprint, Scene};

/// 2.8 case 1 (a): the longest interval between façade receivers (m).
const FACADE_RECEIVER_MAXIMUM_SPACING_M: f64 = 5.0;
/// 2.8 case 1 (b): a remaining segment longer than this carries one receiver (m).
const FACADE_SEGMENT_OWN_RECEIVER_MINIMUM_M: f64 = 2.5;
/// 2.8: "0.1 m in front of the façade" (m).
const FACADE_RECEIVER_OFFSET_M: f64 = 0.1;

/// One façade receiver in click metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacadeReceiver {
    pub position: [f64; 2],
    /// Outward normal of the façade, degrees clockwise from north.
    pub outward_bearing_deg: f64,
}

impl Scene<'_> {
    /// The receivers of a footprint that stand in the open: every 2.8 receiver except those inside
    /// an enclosed footprint (a party wall, or back inside the building itself). A footprint with
    /// no qualifying segment keeps one receiver mid its longest edge before that test; an empty
    /// answer means the building has no exposed façade.
    pub fn facade_receivers(&self, footprint: &Footprint) -> Result<Vec<FacadeReceiver>, String> {
        // Lengths within one int16 step of a limit read as the limit: a wall mapped as 10.00 m
        // arrives 10.00 m +- one step after quantisation and must keep 2 intervals, not 3.
        let resolution_m = self.lattice.metres_per_step[0].max(self.lattice.metres_per_step[1]);
        let mut exposed = Vec::new();
        for receiver in facade_receiver_positions(footprint, resolution_m) {
            if !self.in_enclosed_building(receiver.position)? {
                exposed.push(receiver);
            }
        }
        Ok(exposed)
    }
}

/// Every 2.8 receiver of a footprint in canonical order, before the party-wall test.
pub(super) fn facade_receiver_positions(
    footprint: &Footprint,
    resolution_m: f64,
) -> Vec<FacadeReceiver> {
    let mut receivers = Vec::new();
    let mut longest: Option<Edge> = None;
    for ring in &footprint.rings {
        let edges = canonical_ring_edges(&ring.points, !ring.hole, resolution_m);
        for edge in &edges {
            if longest.is_none_or(|longest| edge.length_m > longest.length_m) {
                longest = Some(*edge);
            }
        }
        place_ring_receivers(&edges, resolution_m, &mut receivers);
    }
    if receivers.is_empty()
        && let Some(edge) = longest
    {
        receivers.push(edge.receiver_at(0.5 * edge.length_m));
    }
    receivers
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    start: [f64; 2],
    end: [f64; 2],
    length_m: f64,
    /// Longer than 2.5 m beyond the length resolution: receivers of its own.
    long: bool,
}

impl Edge {
    /// The receiver `along_m` from the edge start, 0.1 m along the right-hand (outward) normal.
    fn receiver_at(&self, along_m: f64) -> FacadeReceiver {
        let t = along_m / self.length_m;
        let [dx, dy] = [self.end[0] - self.start[0], self.end[1] - self.start[1]];
        let outward = [dy / self.length_m, -dx / self.length_m];
        FacadeReceiver {
            position: [
                self.start[0] + t * dx + FACADE_RECEIVER_OFFSET_M * outward[0],
                self.start[1] + t * dy + FACADE_RECEIVER_OFFSET_M * outward[1],
            ],
            outward_bearing_deg: outward[0].atan2(outward[1]).to_degrees().rem_euclid(360.0),
        }
    }
}

/// A ring's edges with the building on their left, from the smallest vertex, rotated to the first
/// edge that carries receivers of its own.
fn canonical_ring_edges(points: &[[f64; 2]], exterior: bool, resolution_m: f64) -> Vec<Edge> {
    let mut vertices: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for &point in points {
        if vertices.last() != Some(&point) {
            vertices.push(point);
        }
    }
    while vertices.len() > 1 && vertices.first() == vertices.last() {
        vertices.pop();
    }
    let count = vertices.len();
    if count < 3 {
        return Vec::new();
    }
    let origin = vertices[0];
    let twice_area: f64 = (0..count)
        .map(|i| {
            let [a, b] = [vertices[i], vertices[(i + 1) % count]];
            (a[0] - origin[0]) * (b[1] - origin[1]) - (b[0] - origin[0]) * (a[1] - origin[1])
        })
        .sum();
    if (twice_area > 0.0) != exterior {
        vertices.reverse();
    }
    let smallest = (0..count)
        .min_by(|&i, &j| {
            (vertices[i][0].total_cmp(&vertices[j][0]))
                .then(vertices[i][1].total_cmp(&vertices[j][1]))
        })
        .expect("a ring has vertices");
    vertices.rotate_left(smallest);
    let mut edges: Vec<Edge> = (0..count)
        .map(|i| {
            let [start, end] = [vertices[i], vertices[(i + 1) % count]];
            let length_m = (end[0] - start[0]).hypot(end[1] - start[1]);
            let long = length_m > FACADE_SEGMENT_OWN_RECEIVER_MINIMUM_M + resolution_m;
            Edge {
                start,
                end,
                length_m,
                long,
            }
        })
        .filter(|edge| edge.length_m > 0.0)
        .collect();
    if let Some(first_long) = edges.iter().position(|edge| edge.long) {
        edges.rotate_left(first_long);
    }
    edges
}

/// 2.8 case 1 over one ring: long edges alone, runs of short edges as polylines when together
/// longer than 5 m.
fn place_ring_receivers(edges: &[Edge], resolution_m: f64, receivers: &mut Vec<FacadeReceiver>) {
    let mut index = 0;
    while index < edges.len() {
        if edges[index].long {
            place_along(&edges[index..=index], resolution_m, receivers);
            index += 1;
            continue;
        }
        let run_end = edges[index..]
            .iter()
            .position(|edge| edge.long)
            .map_or(edges.len(), |offset| index + offset);
        let run = &edges[index..run_end];
        let run_length_m: f64 = run.iter().map(|edge| edge.length_m).sum();
        if run_length_m > FACADE_RECEIVER_MAXIMUM_SPACING_M + resolution_m {
            place_along(run, resolution_m, receivers);
        }
        index = run_end;
    }
}

/// The fewest equal intervals no longer than 5 m along a polyline, one receiver mid each.
fn place_along(polyline: &[Edge], resolution_m: f64, receivers: &mut Vec<FacadeReceiver>) {
    let total_m: f64 = polyline.iter().map(|edge| edge.length_m).sum();
    let intervals = ((total_m - resolution_m) / FACADE_RECEIVER_MAXIMUM_SPACING_M)
        .ceil()
        .max(1.0) as usize;
    let interval_m = total_m / intervals as f64;
    let (mut edge_index, mut edge_start_m) = (0, 0.0);
    for interval in 0..intervals {
        let at_m = (interval as f64 + 0.5) * interval_m;
        while edge_index + 1 < polyline.len() && at_m > edge_start_m + polyline[edge_index].length_m
        {
            edge_start_m += polyline[edge_index].length_m;
            edge_index += 1;
        }
        receivers.push(polyline[edge_index].receiver_at(at_m - edge_start_m));
    }
}
