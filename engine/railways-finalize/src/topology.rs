//! Original-way metres and SourcePosition interpolation from transport.sqlite.

use grid::geo::{flat_dist, interpolate_longitude_short_arc};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct WayPosition {
    pub vertex: usize,
    pub fraction: f64,
}

#[derive(Clone, Debug)]
pub struct Piece {
    pub from_m: f64,
    pub to_m: f64,
    pub way: Arc<WayGeometry>,
}

#[derive(Debug)]
pub struct WayGeometry {
    nodes: Vec<[f64; 2]>,
    distances: Vec<f64>,
}

impl WayGeometry {
    pub fn new(nodes: Vec<[f64; 2]>) -> Result<Self, String> {
        let distances = way_distances(&nodes)?;
        Ok(Self { nodes, distances })
    }

    pub fn coordinates_at(&self, metres: f64) -> [f64; 2] {
        let target = metres.clamp(0.0, *self.distances.last().unwrap());
        let index = self
            .distances
            .partition_point(|distance| *distance < target)
            .saturating_sub(1)
            .min(self.nodes.len() - 2);
        let span = self.distances[index + 1] - self.distances[index];
        let fraction = if span <= 1e-12 {
            0.0
        } else {
            ((target - self.distances[index]) / span).clamp(0.0, 1.0)
        };
        let start = self.nodes[index];
        let end = self.nodes[index + 1];
        [
            start[0] + (end[0] - start[0]) * fraction,
            interpolate_longitude_short_arc(start[1], end[1], fraction),
        ]
    }
}

pub fn load_square_pieces(path: &Path, square: &str) -> Result<HashMap<(i64, i16), Piece>, String> {
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut statement = connection
        .prepare(
            "SELECT p.way_id, p.segment_idx, p.start_vertex, p.start_fraction,
                    p.end_vertex, p.end_fraction
             FROM source_pieces p JOIN source_ways w ON w.osm_id = p.way_id
             WHERE p.square = ? AND w.family = 'railways'",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([square], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i16>(1)?,
                row.get::<_, usize>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, usize>(4)?,
                row.get::<_, f64>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut pieces = HashMap::new();
    let mut ways = HashMap::new();
    let mut geometry = connection
        .prepare("SELECT nodes_json FROM source_ways WHERE osm_id = ?")
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (osm_id, segment_idx, start_v, start_f, end_v, end_f) =
            row.map_err(|e| e.to_string())?;
        let way = match ways.entry(osm_id) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let json: String = geometry
                    .query_row([osm_id], |row| row.get(0))
                    .map_err(|e| e.to_string())?;
                Arc::clone(entry.insert(Arc::new(WayGeometry::new(parse_nodes(&json)?)?)))
            }
        };
        let start = WayPosition {
            vertex: start_v,
            fraction: start_f,
        };
        let end = WayPosition {
            vertex: end_v,
            fraction: end_f,
        };
        let from_m = distance_at(&way.distances, start)?;
        let to_m = distance_at(&way.distances, end)?;
        if to_m <= from_m {
            return Err(format!(
                "invalid source piece interval {osm_id}:{segment_idx}"
            ));
        }
        pieces.insert((osm_id, segment_idx), Piece { from_m, to_m, way });
    }
    Ok(pieces)
}

fn parse_nodes(json: &str) -> Result<Vec<[f64; 2]>, String> {
    let raw: Vec<(String, Option<[f64; 2]>)> =
        serde_json::from_str(json).map_err(|e| format!("nodes_json: {e}"))?;
    raw.into_iter()
        .map(|(_, coordinates)| {
            coordinates.ok_or_else(|| "source railway way has a missing coordinate".to_owned())
        })
        .collect()
}

pub fn way_distances(nodes: &[[f64; 2]]) -> Result<Vec<f64>, String> {
    if nodes.len() < 2 || nodes.iter().flatten().any(|value| !value.is_finite()) {
        return Err("source distances require a complete way with at least two nodes".to_owned());
    }
    let mut distances = vec![0.0];
    for window in nodes.windows(2) {
        distances.push(
            distances.last().copied().unwrap()
                + flat_dist(window[0][0], window[0][1], window[1][0], window[1][1]),
        );
    }
    Ok(distances)
}

pub fn distance_at(distances: &[f64], position: WayPosition) -> Result<f64, String> {
    if !position.fraction.is_finite() || !(0.0..=1.0).contains(&position.fraction) {
        return Err("invalid source position".to_owned());
    }
    let start = *distances
        .get(position.vertex)
        .ok_or("invalid source position")?;
    if position.fraction == 0.0 {
        return Ok(start);
    }
    let end = *distances
        .get(position.vertex + 1)
        .ok_or("invalid source position")?;
    Ok(start + position.fraction * (end - start))
}
