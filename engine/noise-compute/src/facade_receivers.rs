//! Building-exposure receivers of CNOSSOS §2.8 case 1: every ≤ 5 m along each façade, 0.1 m in front.
//!
//! Directive (EU) 2021/1226, Annex II §2.8 ("Assessment of the noise exposure of
//! buildings"), case 1: façades are split into segments; a segment over 5 m is cut
//! into the fewest equal intervals no longer than 5 m with one receiver in the middle
//! of each; a remaining segment over 2.5 m gets one receiver in its middle; adjacent
//! remaining segments together longer than 5 m are treated as one polyline the same
//! way. Receivers stand 0.1 m in front of the façade, 4 m above the ground
//! (`DEFAULT_RECEIVER_HEIGHT`). Owner decision 2026-09-24: the building's exposure
//! is its noisiest such receiver by all-source Lden.
//!
//! One function places the receivers for the façade-exposure stage (GPU) and the
//! popup (CPU), so both evaluate the same points in the same canonical order:
//! polygon parts as stored; the exterior ring counter-clockwise, holes clockwise
//! (the building always on the left, so the outward normal is on the right); each
//! ring from its lexicographically smallest (gx, gy) vertex, then rotated to its
//! first edge longer than 2.5 m so that a run of short edges is never split.

use crate::propagation::obstacle_index::ObstacleSet;
use grid::poly::GridPolygons;
use grid::{grid_to_meters, meters_to_grid};

/// §2.8 case 1 (a): the longest interval between façade receivers (m).
pub const FACADE_RECEIVER_MAXIMUM_SPACING_M: f64 = 5.0;
/// §2.8 case 1 (b): a remaining segment longer than this carries one receiver (m).
pub const FACADE_SEGMENT_OWN_RECEIVER_MINIMUM_M: f64 = 2.5;
/// §2.8: "0.1 m in front of the façade" (m).
pub const FACADE_RECEIVER_OFFSET_M: f64 = 0.1;
/// Lengths within one z30 quantum of a limit read as the limit: a wall mapped as
/// 10.00 m arrives 10.00 ± 0.02 m after snapping, and must keep 2 intervals, not 3.
const LENGTH_RESOLUTION_M: f64 = grid::GRID_QUANTUM_M;

fn longer_than(length_m: f64, limit_m: f64) -> bool {
    length_m > limit_m + LENGTH_RESOLUTION_M
}

/// One façade receiver: its z30 cell and the façade's outward bearing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacadeReceiverPosition {
    pub gx: i32,
    pub gy: i32,
    /// Outward normal of the façade, degrees clockwise from grid north.
    pub outward_bearing_deg: f32,
}

impl FacadeReceiverPosition {
    /// The receiver's (lat, lon): the decode every reader of the z30 cell uses.
    pub fn latitude_longitude(self) -> (f64, f64) {
        let (x, y) = grid_to_meters(self.gx, self.gy);
        let (lon, lat) = grid::poly::meters_to_lonlat(x, y);
        (lat, lon)
    }
}

/// The receivers of one enclosed footprint that stand in the open: every §2.8
/// receiver except those inside an enclosed footprint (a party wall, or back
/// inside the building itself). Empty means the building has no exposed façade.
pub fn exposed_facade_receivers(
    polygons: &GridPolygons,
    obstacles: &ObstacleSet,
) -> Vec<FacadeReceiverPosition> {
    facade_receiver_positions(polygons)
        .into_iter()
        .filter(|receiver| {
            let (lat, lon) = receiver.latitude_longitude();
            obstacles.enclosed_footprint_at(lat, lon).is_none()
        })
        .collect()
}

/// Every §2.8 receiver of a footprint in canonical order, before the party-wall
/// test. A footprint with no qualifying segment keeps one receiver at the
/// middle of its longest edge.
pub fn facade_receiver_positions(polygons: &GridPolygons) -> Vec<FacadeReceiverPosition> {
    let Some(&origin) = polygons.first().and_then(|rings| rings.first()?.first()) else {
        return Vec::new();
    };
    let frame = LocalFrame::at(origin);
    let mut receivers = Vec::new();
    let mut longest_edge: Option<(f64, Edge)> = None;
    for rings in polygons {
        for (ring_index, ring) in rings.iter().enumerate() {
            let edges = canonical_ring_edges(ring, ring_index == 0, &frame);
            for edge in &edges {
                if longest_edge.is_none_or(|(length, _)| edge.length_m > length) {
                    longest_edge = Some((edge.length_m, *edge));
                }
            }
            place_ring_receivers(&edges, &frame, &mut receivers);
        }
    }
    if receivers.is_empty() {
        if let Some((_, edge)) = longest_edge {
            receivers.push(frame.receiver_on(&edge, 0.5 * edge.length_m));
        }
    }
    receivers
}

/// Ground metres around one vertex: Web Mercator metres scaled by cos(latitude).
struct LocalFrame {
    origin_x_m: f64,
    origin_y_m: f64,
    ground_m_per_mercator_m: f64,
}

impl LocalFrame {
    fn at((gx, gy): (i32, i32)) -> Self {
        let (origin_x_m, origin_y_m) = grid_to_meters(gx, gy);
        let latitude = grid::poly::meters_to_lonlat(origin_x_m, origin_y_m).1;
        Self {
            origin_x_m,
            origin_y_m,
            ground_m_per_mercator_m: latitude.to_radians().cos(),
        }
    }

    fn local(&self, (gx, gy): (i32, i32)) -> [f64; 2] {
        let (x, y) = grid_to_meters(gx, gy);
        [
            (x - self.origin_x_m) * self.ground_m_per_mercator_m,
            (y - self.origin_y_m) * self.ground_m_per_mercator_m,
        ]
    }

    /// The receiver `along_m` from the edge start, offset outward, on the z30 grid.
    fn receiver_on(&self, edge: &Edge, along_m: f64) -> FacadeReceiverPosition {
        let t = along_m / edge.length_m;
        let [dx, dy] = [edge.end[0] - edge.start[0], edge.end[1] - edge.start[1]];
        let outward = [dy / edge.length_m, -dx / edge.length_m];
        let x = edge.start[0] + t * dx + FACADE_RECEIVER_OFFSET_M * outward[0];
        let y = edge.start[1] + t * dy + FACADE_RECEIVER_OFFSET_M * outward[1];
        let (gx, gy) = meters_to_grid(
            self.origin_x_m + x / self.ground_m_per_mercator_m,
            self.origin_y_m + y / self.ground_m_per_mercator_m,
        );
        FacadeReceiverPosition {
            gx,
            gy,
            outward_bearing_deg: outward[0].atan2(outward[1]).to_degrees().rem_euclid(360.0) as f32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    start: [f64; 2],
    end: [f64; 2],
    length_m: f64,
}

impl Edge {
    fn has_own_receivers(&self) -> bool {
        longer_than(self.length_m, FACADE_SEGMENT_OWN_RECEIVER_MINIMUM_M)
    }
}

/// The ring's edges with the building on their left, from the smallest vertex,
/// rotated to the first edge that carries receivers of its own.
fn canonical_ring_edges(ring: &[(i32, i32)], exterior: bool, frame: &LocalFrame) -> Vec<Edge> {
    let mut vertices: Vec<(i32, i32)> = Vec::with_capacity(ring.len());
    for &vertex in ring {
        if vertices.last() != Some(&vertex) {
            vertices.push(vertex);
        }
    }
    while vertices.len() > 1 && vertices.first() == vertices.last() {
        vertices.pop();
    }
    if vertices.len() < 3 {
        return Vec::new();
    }
    let twice_area: i128 = (0..vertices.len())
        .map(|i| {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % vertices.len()];
            i128::from(x0) * i128::from(y1) - i128::from(x1) * i128::from(y0)
        })
        .sum();
    if (twice_area > 0) != exterior {
        vertices.reverse();
    }
    let smallest = (0..vertices.len())
        .min_by_key(|&i| vertices[i])
        .expect("ring has vertices");
    vertices.rotate_left(smallest);
    let mut edges: Vec<Edge> = (0..vertices.len())
        .map(|i| {
            let start = frame.local(vertices[i]);
            let end = frame.local(vertices[(i + 1) % vertices.len()]);
            Edge {
                start,
                end,
                length_m: (end[0] - start[0]).hypot(end[1] - start[1]),
            }
        })
        .filter(|edge| edge.length_m > 0.0)
        .collect();
    if let Some(first_long) = edges.iter().position(Edge::has_own_receivers) {
        edges.rotate_left(first_long);
    }
    edges
}

/// §2.8 case 1 over one ring: long and medium edges alone, runs of short edges
/// as polylines when together longer than 5 m.
fn place_ring_receivers(
    edges: &[Edge],
    frame: &LocalFrame,
    receivers: &mut Vec<FacadeReceiverPosition>,
) {
    let mut index = 0;
    while index < edges.len() {
        if edges[index].has_own_receivers() {
            place_along(&edges[index..=index], frame, receivers);
            index += 1;
            continue;
        }
        let run_end = edges[index..]
            .iter()
            .position(Edge::has_own_receivers)
            .map_or(edges.len(), |offset| index + offset);
        let run = &edges[index..run_end];
        let run_length_m = run.iter().map(|edge| edge.length_m).sum::<f64>();
        if longer_than(run_length_m, FACADE_RECEIVER_MAXIMUM_SPACING_M) {
            place_along(run, frame, receivers);
        }
        index = run_end;
    }
}

/// The fewest equal intervals ≤ 5 m along a polyline, one receiver in the middle of each.
fn place_along(polyline: &[Edge], frame: &LocalFrame, receivers: &mut Vec<FacadeReceiverPosition>) {
    let total_m: f64 = polyline.iter().map(|edge| edge.length_m).sum();
    let intervals = ((total_m - LENGTH_RESOLUTION_M) / FACADE_RECEIVER_MAXIMUM_SPACING_M)
        .ceil()
        .max(1.0) as usize;
    let interval_m = total_m / intervals as f64;
    let mut edge_index = 0;
    let mut edge_start_m = 0.0;
    for interval in 0..intervals {
        let at_m = (interval as f64 + 0.5) * interval_m;
        while edge_index + 1 < polyline.len() && at_m > edge_start_m + polyline[edge_index].length_m {
            edge_start_m += polyline[edge_index].length_m;
            edge_index += 1;
        }
        receivers.push(frame.receiver_on(&polyline[edge_index], at_m - edge_start_m));
    }
}

#[cfg(test)]
#[path = "facade_receivers_tests.rs"]
mod tests;
