//! Split source lines into bounded acoustic chords while retaining their original vertex intervals.

use crate::osm::geometry::{
    M_PER_DEG_LAT, flat_dist, m_per_deg_lon, normalize_longitude, wrapped_longitude_delta,
};

/// Flat-earth bearing in degrees (0..360), 0 = North, 90 = East.
/// Scales longitude by cos(mid_lat) so LKPR's 60° runway reads 60°,
/// not 70° (raw `atan2(Δlon, Δlat)` overshoot at non-equator latitudes).
/// Antimeridian wrap consistent with `flat_dist`.
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    let mid_lat = (lat1 + lat2) / 2.0;
    let cos_lat = mid_lat.to_radians().cos();
    let dlon = wrapped_longitude_delta(lon1, lon2);
    let dx = dlon * cos_lat;
    let dy = lat2 - lat1;
    let bearing = dx.atan2(dy).to_degrees();
    let normalised = if bearing < 0.0 {
        bearing + 360.0
    } else {
        bearing
    };
    normalised as f32
}

/// Maximum perpendicular distance an intermediate vertex may stray from
/// the chord before the walker emits and restarts. 1 m matches OSM
/// surveyor digitisation noise; shared source nodes are protected separately.
const CHORD_EPS_M: f64 = 1.0;

/// Perpendicular distance from `p` to the chord `(a, b)` in metres.
/// Antimeridian-safe via the same `cos(mid_lat)` scaling used by
/// [`flat_dist`].
fn perp_distance_to_chord(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let mid_lat = (a[0] + b[0]) / 2.0;
    let m_lon = m_per_deg_lon(mid_lat.to_radians());
    let bdlon = wrapped_longitude_delta(a[1], b[1]);
    let pdlon = wrapped_longitude_delta(a[1], p[1]);
    let bx = bdlon * m_lon;
    let by = (b[0] - a[0]) * M_PER_DEG_LAT;
    let px = pdlon * m_lon;
    let py = (p[0] - a[0]) * M_PER_DEG_LAT;
    let ab_len_sq = bx * bx + by * by;
    if ab_len_sq < 1e-9 {
        return (px * px + py * py).sqrt();
    }
    let t = (px * bx + py * by) / ab_len_sq;
    let foot_x = t * bx;
    let foot_y = t * by;
    ((px - foot_x).powi(2) + (py - foot_y).powi(2)).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourcePosition {
    pub vertex_index: usize,
    pub fraction_to_next: f64,
}

impl SourcePosition {
    fn vertex(vertex_index: usize) -> Self {
        Self {
            vertex_index,
            fraction_to_next: 0.0,
        }
    }

    fn coordinates(self, node_coordinates: &impl Fn(usize) -> [f64; 2]) -> [f64; 2] {
        let start = node_coordinates(self.vertex_index);
        if self.fraction_to_next == 0.0 {
            return start;
        }
        let end = node_coordinates(self.vertex_index + 1);
        [
            start[0] + (end[0] - start[0]) * self.fraction_to_next,
            normalize_longitude(
                start[1] + wrapped_longitude_delta(start[1], end[1]) * self.fraction_to_next,
            ),
        ]
    }
}

#[derive(Debug)]
pub struct SourceInterval {
    pub start: SourcePosition,
    pub end: SourcePosition,
    pub length_m: f32,
}

impl SourceInterval {
    pub fn geometry(
        &self,
        node_coordinates: impl Fn(usize) -> [f64; 2],
    ) -> ([f64; 2], [f64; 2], f32) {
        (
            self.start.coordinates(&node_coordinates),
            self.end.coordinates(&node_coordinates),
            self.length_m,
        )
    }
}

/// End a simplification range at each shared source node or missing coordinate.
pub fn split_at_junctions(
    nodes: impl IntoIterator<Item = (Option<[f64; 2]>, bool)>,
    max_length_m: f64,
) -> Vec<SourceInterval> {
    let mut segments = Vec::new();
    let mut range = Vec::new();
    for (index, (coords, junction)) in nodes.into_iter().enumerate() {
        if let Some(coords) = coords {
            if range.last().map(|&(_, point)| point) != Some(coords) {
                range.push((index, coords));
            }
            if junction {
                split_range(&range, max_length_m, &mut segments);
                range.clear();
                range.push((index, coords));
            }
        } else {
            split_range(&range, max_length_m, &mut segments);
            range.clear();
        }
    }
    split_range(&range, max_length_m, &mut segments);
    segments
}

fn split_range(
    vertices: &[(usize, [f64; 2])],
    max_length_m: f64,
    segments: &mut Vec<SourceInterval>,
) {
    let mut i = 0;
    while i + 1 < vertices.len() {
        let (start_vertex, start) = vertices[i];
        let (end_vertex, end) = vertices[i + 1];
        let first_hop = flat_dist(start[0], start[1], end[0], end[1]);
        if first_hop > max_length_m {
            let count = (first_hop / max_length_m).ceil() as usize;
            let position = |part| match part {
                0 => SourcePosition::vertex(start_vertex),
                part if part == count => SourcePosition::vertex(end_vertex),
                // Dedup may skip equal coordinates; interpolation belongs to the final original hop.
                part => SourcePosition {
                    vertex_index: end_vertex - 1,
                    fraction_to_next: part as f64 / count as f64,
                },
            };
            for part in 0..count {
                segments.push(SourceInterval {
                    start: position(part),
                    end: position(part + 1),
                    length_m: (first_hop / count as f64) as f32,
                });
            }
            i += 1;
            continue;
        }

        let mut j = i + 1;
        let mut cumulative_length = first_hop;
        while j + 1 < vertices.len() {
            let next = j + 1;
            let current_coords = vertices[j].1;
            let next_coords = vertices[next].1;
            let candidate_length = cumulative_length
                + flat_dist(
                    current_coords[0],
                    current_coords[1],
                    next_coords[0],
                    next_coords[1],
                );
            if candidate_length > max_length_m
                || vertices[i + 1..=j].iter().any(|&(_, point)| {
                    perp_distance_to_chord(point, start, next_coords) > CHORD_EPS_M
                })
            {
                break;
            }
            j = next;
            cumulative_length = candidate_length;
        }
        segments.push(SourceInterval {
            start: SourcePosition::vertex(start_vertex),
            end: SourcePosition::vertex(vertices[j].0),
            length_m: cumulative_length as f32,
        });
        i = j;
    }
}

#[cfg(test)]
#[path = "microsegment_tests.rs"]
mod tests;
