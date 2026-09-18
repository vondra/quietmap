//! Original-way metres and interpolation from the square's `railways.pieces.arrow`.

use arrow::array::{Array, Float64Array, Int16Array, Int32Array, Int64Array, ListArray};
use arrow::ipc::reader::FileReader;
use grid::geo::{cumulative_flat_metres, interpolate_longitude_short_arc};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

pub const PIECES_FILE: &str = "railways.pieces.arrow";

/// One piece's interval in whole-way metres and its own clipped chain of original vertices.
#[derive(Clone, Debug)]
pub struct Piece {
    pub from_m: f64,
    pub to_m: f64,
    nodes: Vec<[f64; 2]>,
    /// Continues the extractor's left-to-right additions from the stored first-vertex value,
    /// so every entry is bit-identical to the whole-way prefix sum.
    distances: Vec<f64>,
}

impl Piece {
    pub fn new(first_vertex_m: f64, from_m: f64, to_m: f64, nodes: Vec<[f64; 2]>) -> Self {
        let distances = cumulative_flat_metres(first_vertex_m, &nodes);
        Self {
            from_m,
            to_m,
            nodes,
            distances,
        }
    }

    pub fn coordinates_at(&self, metres: f64) -> [f64; 2] {
        let target = metres.clamp(self.distances[0], *self.distances.last().unwrap());
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

/// A square without the file has no pieces. A piece of an incomplete way carries null metres and
/// is left out: it can hold no interval, and evidence naming it fails in `split_parent`.
pub fn load_square_pieces(square_dir: &Path) -> Result<HashMap<(i64, i16), Piece>, String> {
    let path = square_dir.join(PIECES_FILE);
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let failure = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let reader = FileReader::try_new(File::open(&path).map_err(|e| failure(&e))?, None)
        .map_err(|e| failure(&e))?;
    if reader
        .schema()
        .metadata()
        .get("transport_pieces_contract")
        .map(String::as_str)
        != Some("1")
    {
        return Err(failure(&"unsupported transport pieces contract"));
    }
    let mut pieces = HashMap::new();
    for batch in reader {
        let batch = batch.map_err(|e| failure(&e))?;
        macro_rules! column {
            ($name:literal, $kind:ty) => {
                batch
                    .column_by_name($name)
                    .and_then(|column| column.as_any().downcast_ref::<$kind>())
                    .ok_or_else(|| failure(&concat!("missing ", $name)))?
            };
        }
        let way_id = column!("way_id", Int64Array);
        let segment_idx = column!("segment_idx", Int16Array);
        let first_vertex_m = column!("first_vertex_m", Float64Array);
        let from_m = column!("from_m", Float64Array);
        let to_m = column!("to_m", Float64Array);
        let chain_lat = column!("chain_lat_e7", ListArray);
        let chain_lon = column!("chain_lon_e7", ListArray);
        for row in 0..batch.num_rows() {
            if first_vertex_m.is_null(row) {
                continue;
            }
            let (lat, lon) = (chain_lat.value(row), chain_lon.value(row));
            let degrees = |list: &dyn Array| -> Result<Vec<f64>, String> {
                let values = list
                    .as_any()
                    .downcast_ref::<Int32Array>()
                    .ok_or_else(|| failure(&"chain items must be Int32"))?;
                Ok(values.values().iter().map(|e7| *e7 as f64 / 1e7).collect())
            };
            let (lat, lon) = (degrees(&lat)?, degrees(&lon)?);
            let key = (way_id.value(row), segment_idx.value(row));
            if lat.len() < 2 || lat.len() != lon.len() || to_m.value(row) <= from_m.value(row) {
                return Err(failure(&format!(
                    "invalid source piece {}:{}",
                    key.0, key.1
                )));
            }
            let nodes = lat
                .into_iter()
                .zip(lon)
                .map(|(lat, lon)| [lat, lon])
                .collect();
            pieces.insert(
                key,
                Piece::new(
                    first_vertex_m.value(row),
                    from_m.value(row),
                    to_m.value(row),
                    nodes,
                ),
            );
        }
    }
    Ok(pieces)
}
