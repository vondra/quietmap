//! Builds `terrain-fine` tiles: at every 1/6" node a national model's detail, its 1/6" height less
//! its own one arc-second average about the node, faded to nothing towards the model's edges and
//! voids, where two models cover a node their details blended by their fades. The detail holds what
//! the 1" lattice cannot (a berm, a cutting, a road's shelf); the reader adds it to the served 1"
//! heights, so the low frequencies and the datum stay theirs, no step shows where a model ends, and
//! the model's own datum cancels (`fetch/national-dem.sh` keeps its heights for this). Every node
//! is computed from global lattice indices alone, so a node two tiles share gets one code.

use crate::dev4::{SQUARES_PER_AXIS, Square};
use crate::terrain::national::{Raster, distances_to_missing};
use rayon::prelude::*;
use std::path::Path;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::square::{TILES_PER_SQUARE, tiles_held, write_square};
use tiles::terrain::Window;
use tiles::terrain_fine::{FINE_NODES_PER_DEGREE, FINE_PER_NODE, detail_code, encode, fine_window};

/// Fine nodes (chessboard) within which of a node every model height must exist for its detail to
/// stand at all: the 1" averages it takes reach this far (a node one to five fine intervals from
/// its cell's corners, each averaging three intervals about it; a node on a 1" node takes that
/// node's average alone).
const DETAIL_NEEDS_NODES: f32 = 8.0;
/// Fine nodes over which the detail then fades in, one 1" interval: a void or the model's edge
/// leaves no step in the ground.
const DETAIL_FADE_NODES: f32 = 6.0;
/// Fine nodes the square's rasters reach beyond its tiles' fine windows (`fetch/national-dem.sh`
/// bounds): beyond the raster counts as missing, so this keeps a stored node's fade off the crop.
const RASTER_MARGIN_NODES: i32 = (DETAIL_NEEDS_NODES + DETAIL_FADE_NODES) as i32;

/// A model's square: its raster and its fade weight at every node.
struct Model {
    raster: Raster,
    weights: Vec<f32>,
}

impl Model {
    fn new(raster: Raster) -> Self {
        let valid: Vec<bool> = raster.heights.iter().map(|h| h.is_finite()).collect();
        let weights = distances_to_missing(&valid, raster.rows, raster.columns, 1.0)
            .into_iter()
            .map(|distance| ((distance - DETAIL_NEEDS_NODES) / DETAIL_FADE_NODES).clamp(0.0, 1.0))
            .collect();
        Model { raster, weights }
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
        f64::from(self.weights[row as usize * self.raster.columns + column as usize])
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

    /// The fade weight and the unfaded detail (m) at a fine node (global indices), `None` where
    /// the model has no say.
    fn detail(&self, north: i32, east: i32) -> Option<(f64, f64)> {
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
        Some((weight, self.raster.height(north, east)? - average))
    }
}

/// The detail at a fine node from every model that has a say: their details weighed by their fades,
/// divided by the weights' sum where it passes 1, so where one model fades out under another the
/// ground goes over from one to the other without a step, and alone it fades to nothing.
fn blended_detail(models: &[Model], north: i32, east: i32) -> f64 {
    let (mut sum, mut weights) = (0.0, 0.0);
    for (weight, detail) in models.iter().filter_map(|model| model.detail(north, east)) {
        sum += weight * detail;
        weights += weight;
    }
    sum / f64::max(weights, 1.0)
}

/// Whether a square's raster fits its neighbours: it reaches `RASTER_MARGIN_NODES` beyond the
/// square's tiles' fine windows, so no stored node fades towards the crop; and where it holds a
/// height beyond them the model goes on into that neighbour, whose own raster must then exist
/// (`has_raster`), or its tiles would get no detail beside full-weight ones here.
fn check_raster(
    raster: &Raster,
    square: Square,
    has_raster: impl Fn(Square) -> bool,
) -> Result<(), String> {
    let corner = |dx: u32, dy: u32| {
        fine_window(Window::of_tile(TileId {
            x: square.x * 8 + dx,
            y: square.y * 8 + dy,
        }))
    };
    let (north_west, south_east) = (corner(0, 0), corner(7, 7));
    let tiles = [
        north_west.north_node,
        north_west.west_node,
        south_east.north_node - south_east.rows as i32 + 1,
        south_east.west_node + south_east.columns as i32 - 1,
    ];
    check_raster_around(raster, tiles, square, has_raster)
}

/// `check_raster` for tiles whose fine windows span `[north, west, south, east]` (global nodes).
fn check_raster_around(
    raster: &Raster,
    [tiles_north, tiles_west, tiles_south, tiles_east]: [i32; 4],
    square: Square,
    has_raster: impl Fn(Square) -> bool,
) -> Result<(), String> {
    let raster_south = raster.north_node - raster.rows as i32 + 1;
    let raster_east = raster.west_node + raster.columns as i32 - 1;
    let margin = RASTER_MARGIN_NODES;
    if raster.north_node < tiles_north + margin
        || raster.west_node > tiles_west - margin
        || raster_south > tiles_south - margin
        || raster_east < tiles_east + margin
    {
        return Err(format!(
            "the raster does not reach {margin} fine nodes beyond the square's tiles"
        ));
    }
    // The raster's nodes beyond the tiles towards a neighbour, west to east and north to south.
    let columns = [
        (raster.west_node, tiles_west - 1),
        (tiles_west, tiles_east),
        (tiles_east + 1, raster_east),
    ];
    let rows = [
        (tiles_north + 1, raster.north_node),
        (tiles_south, tiles_north),
        (raster_south, tiles_south - 1),
    ];
    for (dy, &(south, north)) in rows.iter().enumerate() {
        for (dx, &(west, east)) in columns.iter().enumerate() {
            // x wraps at the antimeridian; beyond a pole there is no square.
            let y = i64::from(square.y) + dy as i64 - 1;
            if (dx, dy) == (1, 1) || !(0..i64::from(SQUARES_PER_AXIS)).contains(&y) {
                continue;
            }
            let neighbour = Square {
                x: (i64::from(square.x) + dx as i64 - 1).rem_euclid(i64::from(SQUARES_PER_AXIS))
                    as u32,
                y: y as u32,
            };
            if has_raster(neighbour) {
                continue;
            }
            let held = (south..=north)
                .any(|row| (west..=east).any(|column| raster.height(row, column).is_some()));
            if held {
                return Err(format!(
                    "the model goes on into square {}_{}, which has no raster",
                    neighbour.x, neighbour.y
                ));
            }
        }
    }
    Ok(())
}

/// The fine details of `tile`, or `None` where no model has any in it.
fn fine_details(tile: TileId, models: &[Model]) -> Option<Vec<i16>> {
    let window = fine_window(Window::of_tile(tile));
    let mut details = Vec::with_capacity(window.node_count());
    let mut any = false;
    for row in 0..window.rows as i32 {
        for column in 0..window.columns as i32 {
            let (north, east) = (window.north_node - row, window.west_node + column);
            let code = detail_code(blended_detail(models, north, east));
            any |= code != 0;
            details.push(code);
        }
    }
    any.then_some(details)
}

/// Writes the fine terrain of `squares`, a file a square: its land tiles (those holding terrain in
/// `terrain_root`) a model has detail in; returns the number of tiles written. Each model is a directory of
/// 1/6" rasters, one a z9 square (`<x9>_<y9>.tif`), each bracketing its tiles' fine windows and
/// `RASTER_MARGIN_NODES` more.
pub fn build(
    squares: &[Square],
    models: &[&Path],
    terrain_root: &Path,
    out: &Path,
) -> Result<usize, String> {
    // A mistyped root would read as a world without models or without land.
    for root in models.iter().copied().chain([terrain_root]) {
        if !root.is_dir() {
            return Err(format!("{}: not a directory", root.display()));
        }
    }
    let mut written = 0;
    for &square in squares {
        let mut rasters = Vec::new();
        for model in models {
            let path = model.join(format!("{}_{}.tif", square.x, square.y));
            match std::fs::metadata(&path) {
                Ok(_) => {
                    let raster = Raster::read(&path, FINE_NODES_PER_DEGREE)?;
                    check_raster(&raster, square, |other| {
                        model.join(format!("{}_{}.tif", other.x, other.y)).exists()
                    })
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                    rasters.push(Model::new(raster));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        if rasters.is_empty() {
            write_square(out, Kind::TerrainFine, square, &[])?;
            continue;
        }
        let land = tiles_held(terrain_root, Kind::Terrain, square)?;
        let tiles: Vec<(TileId, Vec<u8>)> = (0..TILES_PER_SQUARE)
            .into_par_iter()
            .filter(|&index| land[index])
            .filter_map(|index| {
                let tile = square.tile(index);
                let details = fine_details(tile, &rasters)?;
                Some((tile, encode(fine_window(Window::of_tile(tile)), &details)))
            })
            .collect();
        write_square(out, Kind::TerrainFine, square, &tiles)?;
        written += tiles.len();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model of `rows` x `columns` fine nodes at (1,000,000 north, 300,000 east): flat at 100 m,
    /// a bank of `bank_m` along row 60, missing from column `void_column` on.
    fn banked(bank_m: f32, void_column: usize) -> Model {
        let (rows, columns) = (120usize, 160usize);
        let heights = (0..rows * columns)
            .map(|at| match (at / columns, at % columns) {
                (_, column) if column >= void_column => f32::NAN,
                (60, _) => 100.0 + bank_m,
                _ => 100.0,
            })
            .collect();
        Model::new(Raster {
            north_node: 1_000_000,
            west_node: 300_000,
            rows,
            columns,
            heights,
        })
    }

    /// A raster holding a height beyond its square's tiles towards a neighbour without a raster is
    /// refused; with that neighbour's raster, or with nothing beyond, it fits.
    #[test]
    fn a_model_going_on_into_a_square_without_a_raster_is_refused() {
        let margin = RASTER_MARGIN_NODES as usize;
        let (rows, columns) = (20 + 2 * margin, 30 + 2 * margin);
        let (north_node, west_node) = (1_000_000, 300_000);
        let tiles = [
            north_node - margin as i32,
            west_node + margin as i32,
            north_node - (rows - 1 - margin) as i32,
            west_node + (columns - 1 - margin) as i32,
        ];
        let square = Square { x: 10, y: 20 };
        let raster = |east_margin_m: f32| Raster {
            north_node,
            west_node,
            rows,
            columns,
            // The tiles at 100 m, the margin beyond their east edge at `east_margin_m`, no more.
            heights: (0..rows * columns)
                .map(|at| match (at / columns, at % columns) {
                    (row, _) if row < margin || row >= rows - margin => f32::NAN,
                    (_, column) if column < margin => f32::NAN,
                    (_, column) if column >= columns - margin => east_margin_m,
                    _ => 100.0,
                })
                .collect(),
        };
        let east = Square { x: 11, y: 20 };
        let only_east = |other: Square| other == east;
        assert!(check_raster_around(&raster(100.0), tiles, square, |_| false).is_err());
        assert!(check_raster_around(&raster(100.0), tiles, square, only_east).is_ok());
        assert!(check_raster_around(&raster(f32::NAN), tiles, square, |_| false).is_ok());
        // At the antimeridian the east neighbour of square 511 is square 0.
        let last = Square { x: 511, y: 20 };
        let first = Square { x: 0, y: 20 };
        assert!(check_raster_around(&raster(100.0), tiles, last, |other| other == first).is_ok());
    }

    /// Along the bank towards a void: one model's detail fades to nothing, and over another model
    /// the ground goes over to that one's detail, never stepping by more than a sixth of the bank
    /// between neighbours; a flat model has no detail.
    #[test]
    fn the_detail_fades_towards_a_void_and_hands_over_to_another_model_without_a_step() {
        let along = |models: &[Model]| -> Vec<f64> {
            (40..120)
                .map(|column| blended_detail(models, 1_000_000 - 60, 300_000 + column))
                .collect()
        };
        let alone = along(&[banked(7.0, 120)]);
        let over = along(&[banked(7.0, 120), banked(4.0, 160)]);
        for details in [&alone, &over] {
            assert!(details[0] > 3.0, "{}", details[0]);
            for pair in details.windows(2) {
                assert!((pair[1] - pair[0]).abs() <= 7.0 / f64::from(DETAIL_FADE_NODES) + 1e-9);
            }
        }
        assert_eq!(alone[79], 0.0);
        let under = blended_detail(&[banked(4.0, 160)], 1_000_000 - 60, 300_000 + 119);
        assert!((over[79] - under).abs() < 1e-9 && under > 3.0);
        assert!(blended_detail(&[banked(0.0, 160)], 1_000_000 - 60, 300_000 + 60).abs() < 1e-9);
    }
}
