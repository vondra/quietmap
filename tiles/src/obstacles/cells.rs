//! The 256-step cell lattice of `obstacles` tiles: the supercover walk of one edge and the tiles
//! an outline crosses.

use super::{CELL_STEPS, CELLS_PER_SIDE};
use crate::geo::{GlobalSteps, TILES_PER_AXIS, TileId};

/// Visits every cell of the 256-step lattice the edge from `start` to `end` passes through,
/// 4-connected in walking order: dev4's supercover (`grid_build.rs`, Amanatides & Woo), a tie
/// stepping the row, as the popup's ray walk does. `start` and `end` are steps in a frame whose
/// lattice lines are multiples of 256 (global steps or tile-local int16); a cell is
/// `floor(coordinate / 256)`. Where rounding would carry the walk past the end column or row it
/// takes the other axis, as the exact walk does, so it never leaves the box of its end cells.
pub fn for_each_edge_cell(start: [i64; 2], end: [i64; 2], mut visit: impl FnMut([i64; 2])) {
    let cell_steps = CELL_STEPS as f64;
    let mut cell = start.map(|s| s.div_euclid(CELL_STEPS));
    let last = end.map(|s| s.div_euclid(CELL_STEPS));
    let delta = [(end[0] - start[0]) as f64, (end[1] - start[1]) as f64];
    let step = delta.map(|d| if d >= 0.0 { 1 } else { -1 });
    let t_delta = delta.map(|d| {
        if d != 0.0 {
            (cell_steps / d).abs()
        } else {
            f64::INFINITY
        }
    });
    let mut t_max = [0, 1].map(|axis| {
        if delta[axis] == 0.0 {
            return f64::INFINITY;
        }
        let boundary = (cell[axis] + i64::from(delta[axis] >= 0.0)) * CELL_STEPS;
        ((boundary - start[axis]) as f64 / delta[axis]).abs()
    });
    loop {
        visit(cell);
        if cell == last {
            return;
        }
        let axis = if cell[0] == last[0] {
            1
        } else if cell[1] == last[1] || t_max[0] < t_max[1] {
            0
        } else {
            1
        };
        t_max[axis] += t_delta[axis];
        cell[axis] += step[axis];
    }
}

/// The tiles whose cells an outline's edges pass through: every file that must store it.
/// `vertices` run continuously (unwrapped across the antimeridian); rows beyond the poles have no
/// tiles.
pub fn tiles_crossed(vertices: &[GlobalSteps]) -> Vec<TileId> {
    let tiles_per_axis = i64::from(TILES_PER_AXIS);
    let mut tiles = Vec::new();
    for pair in vertices.windows(2) {
        let (start, end) = ([pair[0].x, pair[0].y], [pair[1].x, pair[1].y]);
        for_each_edge_cell(start, end, |[column, row]| {
            let y = row.div_euclid(CELLS_PER_SIDE);
            if !(0..tiles_per_axis).contains(&y) {
                return;
            }
            let x = column.div_euclid(CELLS_PER_SIDE).rem_euclid(tiles_per_axis);
            let tile = TileId {
                x: x as u32,
                y: y as u32,
            };
            if !tiles.contains(&tile) {
                tiles.push(tile);
            }
        });
    }
    tiles.sort_unstable();
    tiles
}
