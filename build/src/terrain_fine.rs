//! Builds `terrain-fine` tiles: at every 1/6" node the served `terrain` tile's height plus a
//! national model's detail there, its 1/6" height less its own one arc-second average about the
//! node. The detail holds what the 1" lattice cannot (a berm, a cutting, a road's shelf); the low
//! frequencies stay the served tile's, so no step shows where the model ends or the 1" heights
//! were blended (`terrain/national.rs`'s feather), and the model's own datum cancels
//! (`fetch/national-dem.sh` keeps its heights for this).

use crate::dev4::Square;
use crate::output::write_tile;
use crate::terrain::national::Raster;
use rayon::prelude::*;
use std::path::Path;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::terrain::{HEIGHT_MISSING, Terrain, code_of_height_m, height_m_of_code};
use tiles::terrain_fine::{FINE_NODES_PER_DEGREE, FINE_PER_NODE, encode, fine_window};

/// The bilinear value at fractional node coordinates (rows southward, columns eastward) of a
/// lattice read by `node`, `None` where a corner it weighs is.
fn bilinear(row: f64, column: f64, node: impl Fn(i64, i64) -> Option<f64>) -> Option<f64> {
    let (r, c) = (row.floor(), column.floor());
    let (fr, fc) = (row - r, column - c);
    let (r, c) = (r as i64, c as i64);
    let mut value = 0.0;
    for (dr, dc, weight) in [
        (0, 0, (1.0 - fr) * (1.0 - fc)),
        (0, 1, (1.0 - fr) * fc),
        (1, 0, fr * (1.0 - fc)),
        (1, 1, fr * fc),
    ] {
        if weight > 0.0 {
            value += weight * node(r + dr, c + dc)?;
        }
    }
    Some(value)
}

/// A model's one arc-second average at a 1" node (global north and east indices): the mean of its
/// 1/6" heights over the node's 1" pixel, six fine intervals each way, the nodes on the pixel's
/// edge half weighted; `None` where one is missing.
fn average_at(model: &Raster, north: i32, east: i32) -> Option<f64> {
    let half = FINE_PER_NODE / 2;
    let weight = |offset: i32| if offset.abs() == half { 0.5 } else { 1.0 };
    let mut sum = 0.0;
    for dr in -half..=half {
        for dc in -half..=half {
            let height = model.height(north * FINE_PER_NODE + dr, east * FINE_PER_NODE + dc)?;
            sum += weight(dr) * weight(dc) * height;
        }
    }
    Some(sum / f64::from(FINE_PER_NODE * FINE_PER_NODE))
}

/// The fine heights of the tile whose served terrain is `terrain`, or `None` where no model has
/// detail in it.
fn fine_heights(terrain: &Terrain, models: &[Raster]) -> Option<Vec<u16>> {
    let coarse = terrain.window();
    let window = fine_window(coarse);
    let coarse_columns = coarse.columns as usize;
    // Each model's 1" average at every node of the tile's 1" window.
    let averages: Vec<Vec<Option<f64>>> = models
        .iter()
        .map(|model| {
            (0..coarse.rows as i32)
                .flat_map(|r| (0..coarse.columns as i32).map(move |c| (r, c)))
                .map(|(r, c)| average_at(model, coarse.north_node - r, coarse.west_node + c))
                .collect()
        })
        .collect();
    let per = f64::from(FINE_PER_NODE);
    let mut heights = Vec::with_capacity(window.node_count());
    let mut detailed = false;
    for row in 0..window.rows {
        for column in 0..window.columns {
            let (coarse_row, coarse_column) = (f64::from(row) / per, f64::from(column) / per);
            let served = bilinear(coarse_row, coarse_column, |r, c| {
                let code = terrain.node(r as u32, c as u32).height_code;
                (code != HEIGHT_MISSING).then(|| height_m_of_code(code))
            });
            let Some(served) = served else {
                heights.push(HEIGHT_MISSING);
                continue;
            };
            let (north, east) = (
                window.north_node - row as i32,
                window.west_node + column as i32,
            );
            let detail = models.iter().zip(&averages).find_map(|(model, average)| {
                let fine_m = model.height(north, east)?;
                let average_m = bilinear(coarse_row, coarse_column, |r, c| {
                    average[r as usize * coarse_columns + c as usize]
                })?;
                Some(fine_m - average_m)
            });
            detailed |= detail.is_some();
            heights.push(code_of_height_m(served + detail.unwrap_or(0.0)));
        }
    }
    detailed.then_some(heights)
}

/// Writes the fine terrain tiles of `squares` whose served terrain in `terrain_root` a model has
/// detail in; returns the number written. Each model is a directory of 1/6" rasters, one a z9
/// square (`<x9>_<y9>.tif`), each bracketing its tiles' fine windows and half a 1" pixel more.
pub fn build(
    squares: &[Square],
    models: &[&Path],
    terrain_root: &Path,
    out: &Path,
) -> Result<usize, String> {
    let mut written = 0;
    for &square in squares {
        let mut rasters = Vec::new();
        for model in models {
            let path = model.join(format!("{}_{}.tif", square.x, square.y));
            if path.exists() {
                rasters.push(Raster::read(&path, FINE_NODES_PER_DEGREE)?);
            }
        }
        if rasters.is_empty() {
            continue;
        }
        written += (0..64u32)
            .into_par_iter()
            .map(|i| {
                let tile = TileId {
                    x: square.x * 8 + i % 8,
                    y: square.y * 8 + i / 8,
                };
                let path = tiles::tile_path(terrain_root, tile, Kind::Terrain);
                let bytes = match std::fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                    Err(error) => return Err(format!("{}: {error}", path.display())),
                };
                let terrain = Terrain::parse(&bytes).map_err(|e| format!("{tile:?}: {e}"))?;
                let Some(heights) = fine_heights(&terrain, &rasters) else {
                    return Ok(0);
                };
                let window = fine_window(terrain.window());
                write_tile(out, tile, Kind::TerrainFine, &encode(window, &heights))?;
                Ok(1)
            })
            .collect::<Result<Vec<usize>, String>>()?
            .into_iter()
            .sum::<usize>();
    }
    Ok(written)
}
