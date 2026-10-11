//! Builds `terrain-fine` tiles: at every 1/6" node a national model's detail, its 1/6" height less
//! its own one arc-second average about the node, faded to nothing towards the model's edges and
//! voids. The detail holds what the 1" lattice cannot (a berm, a cutting, a road's shelf); the
//! reader adds it to the served 1" heights, so the low frequencies and the datum stay theirs, no
//! step shows where the model ends, and the model's own datum cancels (`fetch/national-dem.sh`
//! keeps its heights for this). Every node is computed from global lattice indices alone, so a node
//! two tiles share gets one code.

use crate::dev4::Square;
use crate::output::write_tile;
use crate::terrain::national::Raster;
use rayon::prelude::*;
use std::path::Path;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::terrain::Window;
use tiles::terrain_fine::{FINE_NODES_PER_DEGREE, FINE_PER_NODE, detail_code, encode, fine_window};

/// Fine nodes within which of a node every model height must exist for its detail to stand at all:
/// the 1" averages around it reach this far (the nearest 1" nodes six intervals away, each averaging
/// three intervals about it).
const DETAIL_NEEDS_NODES: u8 = 9;
/// Fine nodes over which the detail then fades in, one 1" interval: a void or the model's edge
/// leaves no step in the ground.
const DETAIL_FADE_NODES: u8 = 6;
/// Fine nodes the square's rasters reach beyond its tiles' fine windows (`fetch/national-dem.sh`
/// bounds): a node's fade sees every missing height that can reach it.
pub const RASTER_MARGIN_NODES: i32 = (DETAIL_NEEDS_NODES + DETAIL_FADE_NODES) as i32 + 1;

/// A model's square: its raster and, per node, the chessboard distance in nodes to the nearest
/// missing height (beyond the raster none is known), capped past where the fade ends.
struct Model {
    raster: Raster,
    distance: Vec<u8>,
}

impl Model {
    fn new(raster: Raster) -> Self {
        let (rows, columns) = (raster.rows, raster.columns);
        let cap = DETAIL_NEEDS_NODES + DETAIL_FADE_NODES + 1;
        let mut distance: Vec<u8> = raster
            .heights
            .iter()
            .map(|h| if h.is_finite() { cap } else { 0 })
            .collect();
        // Two chamfer passes with unit steps on all eight neighbours: the chessboard distance.
        let at = |row: usize, column: usize| row * columns + column;
        for row in 0..rows {
            for column in 0..columns {
                let mut best = distance[at(row, column)];
                for (dr, dc) in [(-1, -1), (-1, 0), (-1, 1), (0, -1)] {
                    let (r, c) = (row as isize + dr, column as isize + dc);
                    if r >= 0 && c >= 0 && (c as usize) < columns {
                        best = best.min(distance[at(r as usize, c as usize)].saturating_add(1));
                    }
                }
                distance[at(row, column)] = best;
            }
        }
        for row in (0..rows).rev() {
            for column in (0..columns).rev() {
                let mut best = distance[at(row, column)];
                for (dr, dc) in [(1, 1), (1, 0), (1, -1), (0, 1)] {
                    let (r, c) = (row as isize + dr, column as isize + dc);
                    if (r as usize) < rows && c >= 0 && (c as usize) < columns {
                        best = best.min(distance[at(r as usize, c as usize)].saturating_add(1));
                    }
                }
                distance[at(row, column)] = best;
            }
        }
        Model { raster, distance }
    }

    /// The node's fade weight in [0, 1] (0 outside the raster).
    fn weight(&self, north: i32, east: i32) -> f64 {
        let row = self.raster.north_node - north;
        let column = east - self.raster.west_node;
        if row < 0
            || column < 0
            || row as usize >= self.raster.rows
            || column as usize >= self.raster.columns
        {
            return 0.0;
        }
        let distance = self.distance[row as usize * self.raster.columns + column as usize];
        (f64::from(distance.saturating_sub(DETAIL_NEEDS_NODES)) / f64::from(DETAIL_FADE_NODES))
            .min(1.0)
    }

    /// The model's one arc-second average at a 1" node (global north and east indices): the mean
    /// of its heights over the node's 1" pixel, six fine intervals each way, the nodes on the
    /// pixel's edge half weighted.
    fn average(&self, north: i32, east: i32) -> Option<f64> {
        let half = FINE_PER_NODE / 2;
        let edge = |offset: i32| if offset.abs() == half { 0.5 } else { 1.0 };
        let mut sum = 0.0;
        for dr in -half..=half {
            for dc in -half..=half {
                let height = self
                    .raster
                    .height(north * FINE_PER_NODE + dr, east * FINE_PER_NODE + dc)?;
                sum += edge(dr) * edge(dc) * height;
            }
        }
        Some(sum / f64::from(FINE_PER_NODE * FINE_PER_NODE))
    }

    /// The faded detail (m) at a fine node (global indices), `None` where the model has none.
    fn detail(&self, north: i32, east: i32) -> Option<f64> {
        let weight = self.weight(north, east);
        if weight <= 0.0 {
            return None;
        }
        // The 1" cell about the node and the node's place in it, from integers alone.
        let (cell_north, sub_north) = (
            north.div_euclid(FINE_PER_NODE),
            north.rem_euclid(FINE_PER_NODE),
        );
        let (cell_east, sub_east) = (
            east.div_euclid(FINE_PER_NODE),
            east.rem_euclid(FINE_PER_NODE),
        );
        let per = f64::from(FINE_PER_NODE);
        let (toward_north, toward_east) = (f64::from(sub_north) / per, f64::from(sub_east) / per);
        let mut average = 0.0;
        for (dn, de, w) in [
            (0, 0, (1.0 - toward_north) * (1.0 - toward_east)),
            (0, 1, (1.0 - toward_north) * toward_east),
            (1, 0, toward_north * (1.0 - toward_east)),
            (1, 1, toward_north * toward_east),
        ] {
            if w > 0.0 {
                average += w * self.average(cell_north + dn, cell_east + de)?;
            }
        }
        Some(weight * (self.raster.height(north, east)? - average))
    }
}

/// Whether a square's raster reaches `RASTER_MARGIN_NODES` beyond its tiles' fine windows, so every
/// node's fade sees each missing height that reaches it.
fn check_margin(raster: &Raster, square: Square) -> Result<(), String> {
    let corner = |dx: u32, dy: u32| {
        fine_window(Window::of_tile(TileId {
            x: square.x * 8 + dx,
            y: square.y * 8 + dy,
        }))
    };
    let (north_west, south_east) = (corner(0, 0), corner(7, 7));
    // The southernmost and easternmost nodes of the tiles and of the raster.
    let tiles_south = south_east.north_node - south_east.rows as i32 + 1;
    let tiles_east = south_east.west_node + south_east.columns as i32 - 1;
    let raster_south = raster.north_node - raster.rows as i32 + 1;
    let raster_east = raster.west_node + raster.columns as i32 - 1;
    let margin = RASTER_MARGIN_NODES;
    let reaches = raster.north_node >= north_west.north_node + margin
        && raster.west_node <= north_west.west_node - margin
        && raster_south <= tiles_south - margin
        && raster_east >= tiles_east + margin;
    if reaches {
        Ok(())
    } else {
        Err(format!(
            "the raster does not reach {margin} fine nodes beyond the square's tiles"
        ))
    }
}

/// The fine details of `tile`, or `None` where no model has any in it.
fn fine_details(tile: TileId, models: &[Model]) -> Option<Vec<i16>> {
    let window = fine_window(Window::of_tile(tile));
    let mut details = Vec::with_capacity(window.node_count());
    let mut any = false;
    for row in 0..window.rows as i32 {
        for column in 0..window.columns as i32 {
            let (north, east) = (window.north_node - row, window.west_node + column);
            let code = models
                .iter()
                .find_map(|model| model.detail(north, east))
                .map_or(0, detail_code);
            any |= code != 0;
            details.push(code);
        }
    }
    any.then_some(details)
}

/// Writes the fine terrain tiles of `squares` (land tiles: those with a `terrain` file in
/// `terrain_root`) a model has detail in; returns the number written. Each model is a directory of
/// 1/6" rasters, one a z9 square (`<x9>_<y9>.tif`), each bracketing its tiles' fine windows and
/// `RASTER_MARGIN_NODES` more.
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
            match std::fs::metadata(&path) {
                Ok(_) => {
                    let raster = Raster::read(&path, FINE_NODES_PER_DEGREE)?;
                    check_margin(&raster, square)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    rasters.push(Model::new(raster));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
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
                let land = tiles::tile_path(terrain_root, tile, Kind::Terrain);
                match std::fs::metadata(&land) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                    Err(error) => return Err(format!("{}: {error}", land.display())),
                }
                let Some(details) = fine_details(tile, &rasters) else {
                    return Ok(0);
                };
                let window = fine_window(Window::of_tile(tile));
                write_tile(out, tile, Kind::TerrainFine, &encode(window, &details))?;
                Ok(1)
            })
            .collect::<Result<Vec<usize>, String>>()?
            .into_iter()
            .sum::<usize>();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 7 m bank one fine row wide on a flat model with a void beside it: the bank's detail stands
    /// full away from the void and fades to nothing towards it, never stepping by more than a
    /// sixth of it between neighbours; a flat model has no detail.
    #[test]
    fn the_detail_fades_towards_a_void_and_is_nothing_on_a_plane() {
        let (rows, columns) = (120usize, 160usize);
        let (north_node, west_node) = (1_000_000, 300_000);
        let bank_row = 60;
        let void_column = 120;
        let heights: Vec<f32> = (0..rows * columns)
            .map(|at| {
                let (row, column) = (at / columns, at % columns);
                if column >= void_column {
                    f32::NAN
                } else if row == bank_row {
                    107.0
                } else {
                    100.0
                }
            })
            .collect();
        let model = Model::new(Raster {
            north_node,
            west_node,
            rows,
            columns,
            heights,
        });
        let detail = |column: usize| {
            model
                .detail(north_node - bank_row as i32, west_node + column as i32)
                .unwrap_or(0.0)
        };
        let full = detail(40);
        assert!(full > 5.0, "{full}");
        let mut previous = full;
        for column in 40..void_column {
            let here = detail(column);
            assert!((here - previous).abs() <= full / f64::from(DETAIL_FADE_NODES) + 1e-9);
            previous = here;
        }
        assert_eq!(detail(void_column - 1), 0.0);
        let flat = Model::new(Raster {
            north_node,
            west_node,
            rows,
            columns,
            heights: vec![250.0; rows * columns],
        });
        assert!(flat.detail(north_node - 60, west_node + 60).unwrap().abs() < 1e-9);
    }
}
