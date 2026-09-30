//! A national terrain model laid over the dev4 heights (step 5, terrain first): a raster of one
//! arc-second pixels centred on the lattice nodes, heights above EGM2008, prepared from the
//! national source by `fetch/` (area average, horizontal and vertical datum through PROJ with its
//! grids). It joins the base with dev4's feather, `w = min(max(d - 1, 0) / 256, 1)` with `d` the
//! distance in nodes to the nearest node the model lacks (its edge and its voids), so no step
//! shows where it ends.

use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;
use tiles::terrain::NODES_PER_DEGREE;

/// Nodes over which the national model fades in from its edge (dev4 `terrain_seams.py`).
const FEATHER_NODES: f32 = 256.0;
/// GeoTIFF georeferencing and GDAL's no-data tags.
const MODEL_PIXEL_SCALE: u16 = 33_550;
const MODEL_TIEPOINT: u16 = 33_922;
const GDAL_NODATA: u16 = 42_113;

/// A national model in memory: heights (NaN where missing) and feather weights (1/255 steps).
pub struct NationalHeights {
    north_node: i32,
    west_node: i32,
    rows: usize,
    columns: usize,
    heights: Vec<f32>,
    weights: Vec<u8>,
}

/// Distances in nodes to the nearest missing node (outside the raster counts as missing), by two
/// chamfer passes with steps 1 and sqrt 2 (at most 8 % long, which a feather does not notice).
fn distances_to_missing(valid: &[bool], rows: usize, columns: usize) -> Vec<f32> {
    const DIAGONAL: f32 = std::f32::consts::SQRT_2;
    let mut distance: Vec<f32> = valid
        .iter()
        .map(|&v| if v { f32::MAX } else { 0.0 })
        .collect();
    let at = |row: isize, column: isize, distance: &[f32]| {
        if row < 0 || column < 0 || row as usize >= rows || column as usize >= columns {
            0.0
        } else {
            distance[row as usize * columns + column as usize]
        }
    };
    for row in 0..rows as isize {
        for column in 0..columns as isize {
            let index = row as usize * columns + column as usize;
            if distance[index] == 0.0 {
                continue;
            }
            let best = [
                at(row - 1, column - 1, &distance) + DIAGONAL,
                at(row - 1, column, &distance) + 1.0,
                at(row - 1, column + 1, &distance) + DIAGONAL,
                at(row, column - 1, &distance) + 1.0,
            ]
            .into_iter()
            .fold(distance[index], f32::min);
            distance[index] = best;
        }
    }
    for row in (0..rows as isize).rev() {
        for column in (0..columns as isize).rev() {
            let index = row as usize * columns + column as usize;
            if distance[index] == 0.0 {
                continue;
            }
            let best = [
                at(row + 1, column + 1, &distance) + DIAGONAL,
                at(row + 1, column, &distance) + 1.0,
                at(row + 1, column - 1, &distance) + DIAGONAL,
                at(row, column + 1, &distance) + 1.0,
            ]
            .into_iter()
            .fold(distance[index], f32::min);
            distance[index] = best;
        }
    }
    distance
}

/// The feather weight of a node `distance` nodes from the nearest missing one, in 1/255 steps.
fn weight_code(distance: f32) -> u8 {
    ((distance - 1.0).max(0.0) / FEATHER_NODES)
        .min(1.0)
        .mul_add(255.0, 0.5) as u8
}

impl NationalHeights {
    /// Reads a prepared model: float32, pixels of exactly one arc-second centred on the nodes.
    pub fn read(path: &Path) -> Result<Self, String> {
        let failed = |error: tiff::TiffError| format!("{}: {error}", path.display());
        let file =
            std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut decoder = Decoder::new(std::io::BufReader::new(file))
            .map_err(failed)?
            .with_limits(tiff::decoder::Limits::unlimited());
        let (columns, rows) = decoder.dimensions().map_err(failed)?;
        let scale = decoder
            .get_tag_f64_vec(Tag::Unknown(MODEL_PIXEL_SCALE))
            .map_err(failed)?;
        let tiepoint = decoder
            .get_tag_f64_vec(Tag::Unknown(MODEL_TIEPOINT))
            .map_err(failed)?;
        let nodata: f32 = decoder
            .get_tag_ascii_string(Tag::Unknown(GDAL_NODATA))
            .ok()
            .and_then(|text| text.trim_end_matches('\0').trim().parse().ok())
            .unwrap_or(f32::NAN);
        let step = 1.0 / f64::from(NODES_PER_DEGREE);
        if scale.len() < 2
            || (scale[0] - step).abs() > 1e-9
            || (scale[1] - step).abs() > 1e-9
            || tiepoint.len() < 6
            || tiepoint[0] != 0.0
            || tiepoint[1] != 0.0
        {
            return Err(format!("{}: not the one arc-second grid", path.display()));
        }
        // The tiepoint is the corner of the first pixel, half a pixel west and north of its node.
        let node = |degrees: f64, half: f64| (degrees / step + half).round() as i32;
        let (west_node, north_node) = (node(tiepoint[3], 0.5), node(tiepoint[4], -0.5));
        let DecodingResult::F32(values) = decoder.read_image().map_err(failed)? else {
            return Err(format!("{}: not float32", path.display()));
        };
        let heights: Vec<f32> = values
            .into_iter()
            .map(|h| {
                if h == nodata || !h.is_finite() {
                    f32::NAN
                } else {
                    h
                }
            })
            .collect();
        let (rows, columns) = (rows as usize, columns as usize);
        let valid: Vec<bool> = heights.iter().map(|h| h.is_finite()).collect();
        let weights = distances_to_missing(&valid, rows, columns)
            .into_iter()
            .map(weight_code)
            .collect();
        Ok(NationalHeights {
            north_node,
            west_node,
            rows,
            columns,
            heights,
            weights,
        })
    }

    /// The model's height at a node and its weight there, or `None` where it has no say.
    pub fn at(&self, north_node: i32, east_node: i32) -> Option<(f64, f64)> {
        let row = self.north_node - north_node;
        let column = east_node - self.west_node;
        if row < 0 || column < 0 || row as usize >= self.rows || column as usize >= self.columns {
            return None;
        }
        let index = row as usize * self.columns + column as usize;
        let weight = self.weights[index];
        (weight > 0).then(|| (f64::from(self.heights[index]), f64::from(weight) / 255.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid block within a missing border: its edge nodes weigh nothing, the weight grows by
    /// 1/256 a node inwards to the full weight past 257 nodes, and a void feathers alike.
    #[test]
    fn a_model_fades_in_from_its_edges_and_voids() {
        let (rows, columns) = (600, 600);
        let mut valid = vec![true; rows * columns];
        for row in 0..rows {
            for column in 0..columns {
                if row < 10 || column < 10 || row >= 590 || column >= 590 {
                    valid[row * columns + column] = false;
                }
            }
        }
        valid[100 * columns + 100] = false;
        let distance = distances_to_missing(&valid, rows, columns);
        let weight = |row: usize, column: usize| weight_code(distance[row * columns + column]);
        assert_eq!(weight(5, 5), 0);
        assert_eq!(weight(10, 400), 0);
        assert_eq!(weight(11, 400), 1);
        assert_eq!(weight(138, 400), 128);
        assert_eq!(weight(100, 101), 0);
        assert_eq!(weight(300, 300), 255);
    }
}
