//! Complete OSM train itineraries, retaining repeated ways and unique node-connected orientation.
use super::{
    movements::Stop,
    network::{distance, project},
};
use crate::dev4::column;
use arrow_array::{
    Array, Float64Array, Int32Array, Int64Array, ListArray, RecordBatch, StringArray, UInt8Array,
};
use arrow_ipc::reader::{FileReader, StreamReader};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::BufReader,
    path::Path,
    sync::Arc,
};

pub struct Way {
    pub id: i64,
    pub nodes: Vec<i64>,
    pub points: Vec<[f64; 2]>,
    pub metres: Vec<f64>,
}
pub struct DirectedWay {
    pub way: Arc<Way>,
    pub reverse: bool,
}
pub struct Itinerary {
    pub ways: Vec<DirectedWay>,
    pub points: Vec<[f64; 2]>,
    pub metres: Vec<f64>,
}
pub struct Routes {
    routes: Vec<Itinerary>,
    cells: HashMap<(i32, i32), Vec<usize>>,
}

fn list<T: Array + 'static>(
    batch: &RecordBatch,
    name: &str,
    row: usize,
) -> Result<Arc<dyn Array>, String> {
    let array = column::<ListArray>(batch, name)?.value(row);
    if !array.as_any().is::<T>() {
        return Err(format!("source route {name} has wrong list type"));
    }
    Ok(array)
}
fn sibling(directory: &Path, kind: &str) -> Result<std::path::PathBuf, String> {
    let actual = directory
        .canonicalize()
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    crate::osm::year_sibling_path(&actual, kind).map_err(|error| error.to_string())
}

impl Routes {
    pub fn read(directory: &Path) -> Result<Self, String> {
        let reader = FileReader::try_new(
            File::open(sibling(directory, "train-routes")?).map_err(|error| error.to_string())?,
            None,
        )
        .map_err(|error| error.to_string())?;
        if reader
            .schema()
            .metadata()
            .get("train_routes_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err("unsupported train route contract".into());
        }
        let mut raw = Vec::new();
        let mut required = HashSet::new();
        for batch in reader {
            let batch = batch.map_err(|error| error.to_string())?;
            for row in 0..batch.num_rows() {
                let ids = list::<Int64Array>(&batch, "member_id", row)?;
                let ids = ids.as_any().downcast_ref::<Int64Array>().unwrap();
                let kinds = list::<UInt8Array>(&batch, "member_kind", row)?;
                let kinds = kinds.as_any().downcast_ref::<UInt8Array>().unwrap();
                let roles = list::<StringArray>(&batch, "member_role", row)?;
                let roles = roles.as_any().downcast_ref::<StringArray>().unwrap();
                let mut members = Vec::new();
                let mut valid = true;
                for index in 0..ids.len() {
                    let role = roles.value(index);
                    if [
                        "stop",
                        "stop_entry_only",
                        "stop_exit_only",
                        "platform",
                        "platform_entry_only",
                        "platform_exit_only",
                    ]
                    .contains(&role)
                    {
                        continue;
                    }
                    if kinds.value(index) != b'w' || !["", "forward", "backward"].contains(&role) {
                        valid = false;
                        break;
                    }
                    members.push((ids.value(index), role.to_string()));
                    required.insert(ids.value(index));
                }
                if valid && !members.is_empty() {
                    raw.push(members);
                }
            }
        }
        let reader = StreamReader::try_new(
            BufReader::new(
                File::open(sibling(directory, "railway-ways")?)
                    .map_err(|error| error.to_string())?,
            ),
            None,
        )
        .map_err(|error| error.to_string())?;
        if reader
            .schema()
            .metadata()
            .get("railway_ways_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err("unsupported railway way contract".into());
        }
        let mut ways = HashMap::new();
        for batch in reader {
            let batch = batch.map_err(|error| error.to_string())?;
            let ids = column::<Int64Array>(&batch, "way_id")?;
            for row in 0..batch.num_rows() {
                let id = ids.value(row);
                if !required.contains(&id) || column::<ListArray>(&batch, "node_m")?.is_null(row) {
                    continue;
                }
                let nodes = list::<Int64Array>(&batch, "node_id", row)?;
                let nodes = nodes
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .values()
                    .to_vec();
                let latitudes = list::<Int32Array>(&batch, "lat_e7", row)?;
                let latitudes = latitudes.as_any().downcast_ref::<Int32Array>().unwrap();
                let longitudes = list::<Int32Array>(&batch, "lon_e7", row)?;
                let longitudes = longitudes.as_any().downcast_ref::<Int32Array>().unwrap();
                let metres = list::<Float64Array>(&batch, "node_m", row)?;
                let metres = metres
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .values()
                    .to_vec();
                let points = (0..nodes.len())
                    .map(|i| {
                        [
                            f64::from(latitudes.value(i)) / 1e7,
                            f64::from(longitudes.value(i)) / 1e7,
                        ]
                    })
                    .collect();
                if nodes.len() >= 2 {
                    ways.insert(
                        id,
                        Arc::new(Way {
                            id,
                            nodes,
                            points,
                            metres,
                        }),
                    );
                }
            }
        }
        let mut routes = Self {
            routes: Vec::new(),
            cells: HashMap::new(),
        };
        for members in raw {
            let Some(oriented) = orient(&members, &ways) else {
                continue;
            };
            let mut points = Vec::new();
            for (index, directed) in oriented.iter().enumerate() {
                let mut chain = directed.way.points.clone();
                if directed.reverse {
                    chain.reverse();
                }
                points.extend(chain.into_iter().skip(usize::from(index > 0)));
            }
            let mut metres = vec![0.0];
            for pair in points.windows(2) {
                metres.push(metres.last().unwrap() + distance(pair[0], pair[1]));
            }
            let index = routes.routes.len();
            let mut seen = HashSet::new();
            for point in &points {
                if seen.insert(cell(*point)) {
                    routes.cells.entry(cell(*point)).or_default().push(index);
                }
            }
            routes.routes.push(Itinerary {
                ways: oriented,
                points,
                metres,
            });
        }
        eprintln!(
            "railways: {} complete OSM train itineraries",
            routes.routes.len()
        );
        Ok(routes)
    }

    pub fn unique(&self, stops: &[Stop]) -> Option<&Itinerary> {
        if stops.len() < 2 {
            return None;
        }
        let mut seen = HashSet::new();
        let mut result = None;
        for stop in stops {
            let (y, x) = cell([stop.latitude, stop.longitude]);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(routes) = self.cells.get(&(y + dy, x + dx)) {
                        for &index in routes {
                            if !seen.insert(index) {
                                continue;
                            }
                            if fits(&self.routes[index], stops) {
                                if result.is_some() {
                                    return None;
                                }
                                result = Some(&self.routes[index]);
                            }
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
        (point[0] / 0.05).floor() as i32,
        (point[1] / 0.05).floor() as i32,
    )
}
fn orient(members: &[(i64, String)], ways: &HashMap<i64, Arc<Way>>) -> Option<Vec<DirectedWay>> {
    let mut layers = Vec::<[(u8, usize, i64); 2]>::new();
    for (id, role) in members {
        let way = ways.get(id)?;
        let mut layer = [(0, usize::MAX, 0); 2];
        for (reverse, current) in layer.iter_mut().enumerate() {
            if role == "forward" && reverse == 1 || role == "backward" && reverse == 0 {
                continue;
            }
            let start = if reverse == 1 {
                *way.nodes.last()?
            } else {
                way.nodes[0]
            };
            let end = if reverse == 1 {
                way.nodes[0]
            } else {
                *way.nodes.last()?
            };
            if let Some(previous) = layers.last() {
                for (index, state) in previous.iter().enumerate() {
                    if state.0 > 0 && state.2 == start {
                        *current = ((current.0 + state.0).min(2), index, end);
                    }
                }
            } else {
                *current = (1, usize::MAX, end);
            }
        }
        layers.push(layer);
    }
    let last = layers.last()?;
    if last[0].0 + last[1].0 != 1 {
        return None;
    }
    let mut orientation = usize::from(last[1].0 == 1);
    let mut ordered = Vec::new();
    for index in (0..members.len()).rev() {
        ordered.push(DirectedWay {
            way: ways[&members[index].0].clone(),
            reverse: orientation == 1,
        });
        orientation = layers[index][orientation].1;
    }
    ordered.reverse();
    Some(ordered)
}

fn fits(route: &Itinerary, stops: &[Stop]) -> bool {
    let mut previous: Vec<(f64, f64)> = Vec::new();
    for stop in stops {
        let mut candidates = Vec::new();
        for (index, pair) in route.points.windows(2).enumerate() {
            let (error, fraction) = project([stop.latitude, stop.longitude], pair[0], pair[1]);
            if error <= 300.0 {
                candidates.push((
                    route.metres[index]
                        + fraction * (route.metres[index + 1] - route.metres[index]),
                    error * error,
                ));
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cursor = 0;
        let mut best = f64::INFINITY;
        for candidate in &mut candidates {
            if previous.is_empty() {
                continue;
            }
            while cursor < previous.len() && previous[cursor].0 <= candidate.0 {
                best = best.min(previous[cursor].1);
                cursor += 1;
            }
            candidate.1 += best;
        }
        if !candidates.iter().any(|state| state.1.is_finite()) {
            return false;
        }
        previous = candidates;
    }
    true
}

#[cfg(test)]
#[path = "route_path_tests.rs"]
mod route_path_tests;
