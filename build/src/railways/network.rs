//! A connected railway graph keyed by original OSM node identity and source-way cuts.
use super::write::{col_f32, col_i16, col_i32, col_i64, col_u8};
use crate::osm::geometry;
use arrow_array::{Array, Float64Array, RecordBatch, UInt16Array};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Edge {
    pub nodes: [usize; 2],
    pub points: [[f64; 2]; 2],
    pub length: f64,
    pub way: i64,
    pub segment: i16,
    pub square: [u16; 2],
    pub extent: [f64; 2],
    pub rail_type: u8,
    pub traversal_only: bool,
}

impl Edge {
    pub fn family(&self) -> u8 {
        if self.rail_type == 0 {
            1
        } else if self.rail_type == 3 {
            2
        } else {
            0
        }
    }
    pub fn midpoint(&self) -> [f64; 2] {
        [
            (self.points[0][0] + self.points[1][0]) / 2.0,
            (self.points[0][1] + self.points[1][1]) / 2.0,
        ]
    }
}

#[derive(Clone, Copy)]
pub struct AcousticPiece {
    pub square: [u16; 2],
    pub way: i64,
    pub segment: i16,
    pub extent: [f64; 2],
}

#[derive(Default)]
pub struct Network {
    pub nodes: Vec<[f64; 2]>,
    pub edges: Vec<Edge>,
    pub adjacency: Vec<Vec<usize>>,
    pub components: Vec<usize>,
    cells: HashMap<(i32, i32), Vec<usize>>,
    pub way_pieces: HashMap<i64, Vec<AcousticPiece>>,
    node_cells: HashMap<(i32, i32), Vec<usize>>,
}

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
enum NodeKey {
    Original(i64),
    Cut(i64, u16, u64),
}

pub fn squares(directory: &Path) -> Result<Vec<[u16; 2]>, String> {
    let mut squares = Vec::new();
    for x in std::fs::read_dir(directory.join("z9")).map_err(|error| error.to_string())? {
        let x = x.map_err(|error| error.to_string())?;
        let Ok(x_number) = x.file_name().to_string_lossy().parse::<u16>() else {
            continue;
        };
        for y in std::fs::read_dir(x.path()).map_err(|error| error.to_string())? {
            let y = y.map_err(|error| error.to_string())?;
            let Ok(y_number) = y.file_name().to_string_lossy().parse::<u16>() else {
                continue;
            };
            if x_number >= 512 || y_number >= 512 {
                return Err("invalid OSM geography square".into());
            }
            if y.path().join("railways.arrow").exists() {
                squares.push([x_number, y_number]);
            }
        }
    }
    squares.sort();
    Ok(squares)
}

fn number<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref())
        .ok_or_else(|| format!("source railway topology lacks {name}"))
}

impl Network {
    pub fn read(directory: &Path, bounds: [f64; 4]) -> Result<Self, String> {
        let mut network = Self::default();
        let mut nodes = HashMap::new();
        let northwest = geometry::square_of(bounds[2], bounds[1]);
        let southeast = geometry::square_of(bounds[0], bounds[3]);
        for square in squares(directory)? {
            if square[0] < northwest.x
                || square[0] > southeast.x
                || square[1] < northwest.y
                || square[1] > southeast.y
            {
                continue;
            }
            let path = directory.join(format!("z9/{}/{}", square[0], square[1]));
            let rails = crate::dev4::read_table(&path.join("railways.arrow"))?
                .ok_or("railways disappeared")?;
            if rails.metadata.contains_key("rail_traffic_contract") {
                return Err("network requires source OSM geography".into());
            }
            let pieces = crate::dev4::read_table(&path.join("railways.pieces.arrow"))?
                .ok_or("missing original railway topology")?;
            crate::dev4::require_stamp(&pieces, "transport_pieces_contract", "1")?;
            let mut topology = HashMap::new();
            for batch in &pieces.batches {
                let way = col_i64(batch, "way_id")?;
                let segment = col_i16(batch, "segment_idx")?;
                let from = number::<Float64Array>(batch, "from_m")?;
                let to = number::<Float64Array>(batch, "to_m")?;
                for row in 0..batch.num_rows() {
                    let mut keys = [NodeKey::Original(0); 2];
                    let mut points = [[0.0; 2]; 2];
                    for (index, side) in ["start", "end"].into_iter().enumerate() {
                        let fraction =
                            number::<Float64Array>(batch, &format!("{side}_fraction"))?.value(row);
                        let key = if fraction > 0.0 {
                            NodeKey::Cut(
                                way.value(row),
                                number::<UInt16Array>(batch, &format!("{side}_vertex"))?.value(row),
                                fraction.to_bits(),
                            )
                        } else {
                            NodeKey::Original(col_i64(batch, &format!("{side}_node"))?.value(row))
                        };
                        let (longitude, latitude) = geometry::grid_cell_lonlat(
                            col_i32(batch, &format!("{side}_gx"))?.value(row),
                            col_i32(batch, &format!("{side}_gy"))?.value(row),
                        );
                        points[index] = [latitude, longitude];
                        keys[index] = key;
                    }
                    if from.is_null(row) || to.is_null(row) {
                        continue;
                    }
                    topology.insert(
                        (way.value(row), segment.value(row)),
                        (keys, points, [from.value(row), to.value(row)]),
                    );
                }
            }
            for batch in &rails.batches {
                let way = col_i64(batch, "osm_id")?;
                let segment = col_i16(batch, "segment_idx")?;
                let kind = col_u8(batch, "rail_type")?;
                let service = col_u8(batch, "service")?;
                let length = col_f32(batch, "length_m")?;
                for row in 0..batch.num_rows() {
                    let key = (way.value(row), segment.value(row));
                    let Some(&(keys, points, extent)) = topology.get(&key) else {
                        return Err(format!("missing complete railway piece {key:?}"));
                    };
                    network
                        .way_pieces
                        .entry(key.0)
                        .or_default()
                        .push(AcousticPiece {
                            square,
                            way: key.0,
                            segment: key.1,
                            extent,
                        });
                    let traversal_only = service.value(row) == 4;
                    if !traversal_only
                        && (service.value(row) != 0 || ![0, 3].contains(&kind.value(row)))
                    {
                        continue;
                    }
                    let nodes = [0, 1].map(|index| {
                        *nodes.entry(keys[index]).or_insert_with(|| {
                            let id = network.nodes.len();
                            network.nodes.push(points[index]);
                            id
                        })
                    });
                    network.edges.push(Edge {
                        nodes,
                        points,
                        extent,
                        length: f64::from(length.value(row)),
                        way: key.0,
                        segment: key.1,
                        square,
                        rail_type: kind.value(row),
                        traversal_only,
                    });
                }
            }
        }
        network.index();
        eprintln!(
            "railways: {} connected source edges, {} nodes",
            network.edges.len(),
            network.nodes.len()
        );
        Ok(network)
    }

    pub fn index(&mut self) {
        self.adjacency = vec![Vec::new(); self.nodes.len()];
        self.cells.clear();
        self.node_cells.clear();
        for (index, edge) in self.edges.iter().enumerate() {
            self.adjacency[edge.nodes[0]].push(index);
            if edge.nodes[0] != edge.nodes[1] {
                self.adjacency[edge.nodes[1]].push(index);
            }
            for point in edge.points {
                self.cells.entry(cell(point)).or_default().push(index);
            }
        }
        for (index, point) in self.nodes.iter().enumerate() {
            self.node_cells.entry(cell(*point)).or_default().push(index);
        }
        self.components = vec![usize::MAX; self.nodes.len()];
        let mut queue = Vec::new();
        for start in 0..self.nodes.len() {
            if self.components[start] != usize::MAX {
                continue;
            }
            queue.clear();
            queue.push(start);
            self.components[start] = start;
            let mut cursor = 0;
            while cursor < queue.len() {
                let node = queue[cursor];
                cursor += 1;
                for &edge in &self.adjacency[node] {
                    for target in self.edges[edge].nodes {
                        if self.components[target] == usize::MAX {
                            self.components[target] = start;
                            queue.push(target);
                        }
                    }
                }
            }
        }
    }

    pub fn nearest_node(&self, point: [f64; 2], family: u8) -> Option<(usize, f64)> {
        let (y, x) = cell(point);
        let dy = (300.0_f64 / 1105.4).ceil().max(1.0) as i32;
        let dx = (300.0 / (1113.2 * (point[0] * std::f64::consts::PI / 180.0).cos().max(0.05)))
            .ceil()
            .max(1.0) as i32;
        let mut nearest = None;
        for oy in -dy..=dy {
            for ox in -dx..=dx {
                for &node in self.node_cells.get(&(y + oy, x + ox)).into_iter().flatten() {
                    if family != 0
                        && !self.adjacency[node].iter().any(|index| {
                            let edge = &self.edges[*index];
                            edge.traversal_only || edge.family() == family
                        })
                    {
                        continue;
                    }
                    let error = distance(point, self.nodes[node]);
                    if error <= 300.0 && nearest.is_none_or(|(_, best)| error < best) {
                        nearest = Some((node, error));
                    }
                }
            }
        }
        nearest
    }

    pub fn nearby(&self, point: [f64; 2], radius: f64) -> Vec<usize> {
        let (y, x) = cell(point);
        let dy = (radius / (0.01 * 110_540.0)).ceil().max(1.0) as i32;
        let dx = (radius / (0.01 * 111_320.0 * point[0].to_radians().cos().max(0.05)))
            .ceil()
            .max(1.0) as i32;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for offset_y in -dy..=dy {
            for offset_x in -dx..=dx {
                if let Some(edges) = self.cells.get(&(y + offset_y, x + offset_x)) {
                    for &edge in edges {
                        if seen.insert(edge) {
                            result.push(edge);
                        }
                    }
                }
            }
        }
        result
    }
}

fn cell(point: [f64; 2]) -> (i32, i32) {
    (
        (point[0] / 0.01).floor() as i32,
        (point[1] / 0.01).floor() as i32,
    )
}

// The source matcher projects at the observation latitude; OSM prefix metres use segment latitude.
pub fn project(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let cosine = (point[0] * std::f64::consts::PI / 180.0).cos();
    let px = geometry::wrapped_longitude_delta(a[1], point[1]) * 111_320.0 * cosine;
    let py = (point[0] - a[0]) * 110_540.0;
    let bx = geometry::wrapped_longitude_delta(a[1], b[1]) * 111_320.0 * cosine;
    let by = (b[0] - a[0]) * 110_540.0;
    let squared_length = bx * bx + by * by;
    if squared_length < 1e-6 {
        return (distance(point, a), 0.0);
    }
    let fraction = ((px * bx + py * by) / squared_length).clamp(0.0, 1.0);
    ((px - fraction * bx).hypot(py - fraction * by), fraction)
}

pub fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let cosine = ((a[0] + b[0]) / 2.0 * std::f64::consts::PI / 180.0).cos();
    (geometry::wrapped_longitude_delta(a[1], b[1]) * 111_320.0 * cosine)
        .hypot((b[0] - a[0]) * 110_540.0)
}

pub fn distance_to_line(point: [f64; 2], line: &[[f64; 2]]) -> f64 {
    line.windows(2)
        .map(|pair| project(point, pair[0], pair[1]).0)
        .fold(f64::INFINITY, f64::min)
}
