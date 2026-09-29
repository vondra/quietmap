//! Buildings and walls of the tiles read so far, queried in the click's metre frame ([east, north]
//! of its [`LocalFrame`]): ray crossings, the building a point stands in, façade receivers, the
//! receiver reflection bonus and the skyline of the line quadrature.
//!
//! Internally positions are "scene steps": int16 steps (y south) from the north-west corner of
//! the cell holding the click origin. Stored vertices are exact integers there whichever tile
//! stores them, cell `k` spans `[256 k, 256 (k + 1))` on each axis, and the metre frame is affine
//! in them (the frame scales Mercator), so metres follow from the frame's two scales.

mod containment;
mod crossings;
mod facades;
mod skyline;
#[cfg(test)]
mod tests;
mod tile;

pub use containment::{Footprint, FootprintRing};
pub use facades::FacadeReceiver;

use tile::SceneTile;

use tiles::geo::{LATTICE_SNAP_STEPS, LocalFrame, STEPS_PER_TILE, TILES_PER_AXIS, TileId};
use tiles::obstacles::{CELL_STEPS, CELLS_PER_SIDE, Obstacles};

const WORLD_STEPS: i64 = STEPS_PER_TILE as i64 * TILES_PER_AXIS as i64;

/// The click's cell lattice and its metre scales.
#[derive(Clone, Copy, Debug)]
struct Lattice {
    /// Global cell holding the click origin.
    origin_cell: [i64; 2],
    /// The click origin in scene steps.
    origin: [f64; 2],
    /// Metres per step east and north at the click.
    metres_per_step: [f64; 2],
}

impl Lattice {
    fn new(frame: &LocalFrame) -> Self {
        let origin_steps = frame.steps_of_metres([0.0, 0.0]);
        let origin_cell = origin_steps.map(|steps| (steps / CELL_STEPS as f64).floor() as i64);
        Lattice {
            origin_cell,
            origin: [0, 1].map(|axis| origin_steps[axis] - (origin_cell[axis] * CELL_STEPS) as f64),
            metres_per_step: [
                frame.east_m_per_unit / STEPS_PER_TILE,
                frame.north_m_per_unit / STEPS_PER_TILE,
            ],
        }
    }

    /// Scene steps of a point; a lattice point (a stored vertex or source end) comes back as its
    /// exact integer, so a source on a tile's north or west edge stays in its tile's cells.
    fn steps(&self, metres: [f64; 2]) -> [f64; 2] {
        let snap = |steps: f64| {
            let nearest = steps.round();
            if (steps - nearest).abs() < LATTICE_SNAP_STEPS {
                nearest
            } else {
                steps
            }
        };
        [
            snap(self.origin[0] + metres[0] / self.metres_per_step[0]),
            snap(self.origin[1] - metres[1] / self.metres_per_step[1]),
        ]
    }

    fn metres(&self, steps: [f64; 2]) -> [f64; 2] {
        [
            (steps[0] - self.origin[0]) * self.metres_per_step[0],
            (self.origin[1] - steps[1]) * self.metres_per_step[1],
        ]
    }

    /// Scene steps of a tile's local origin (its centre), taken the short way around the world.
    fn tile_offset(&self, tile: TileId) -> [i64; 2] {
        let centre = tile.centre_steps();
        let dx = centre.x - self.origin_cell[0] * CELL_STEPS;
        [
            (dx + WORLD_STEPS / 2).rem_euclid(WORLD_STEPS) - WORLD_STEPS / 2,
            centre.y - self.origin_cell[1] * CELL_STEPS,
        ]
    }
}

fn cell_of(steps: [f64; 2]) -> [i64; 2] {
    steps.map(|s| (s / CELL_STEPS as f64).floor() as i64)
}

enum Located<'s, 'a> {
    Read(&'s SceneTile<'a>, usize),
    Empty,
    NotRead(TileId),
}

enum Slot<'a> {
    NotRead,
    /// Read, and no obstacles file.
    Empty,
    Read(SceneTile<'a>),
}

/// The obstacles of the tiles read so far, by tile offset from the click's tile. A cell of a tile
/// never inserted is "not read": a query that needs it fails, never answers quieter.
pub struct Scene<'a> {
    lattice: Lattice,
    centre: TileId,
    /// The click's cell counted from the north-west corner of its tile.
    origin_cell_in_centre: [i64; 2],
    radius: i64,
    /// Row-major over tile offsets `-radius..=radius`.
    slots: Vec<Slot<'a>>,
    widest_footprint_steps: i64,
}

impl<'a> Scene<'a> {
    pub fn new(frame: LocalFrame) -> Self {
        let lattice = Lattice::new(&frame);
        let [column, row] = lattice.origin_cell;
        Scene {
            lattice,
            centre: TileId {
                x: (column / CELLS_PER_SIDE) as u32,
                y: (row / CELLS_PER_SIDE) as u32,
            },
            origin_cell_in_centre: [column % CELLS_PER_SIDE, row % CELLS_PER_SIDE],
            radius: 0,
            slots: vec![Slot::NotRead],
            widest_footprint_steps: 0,
        }
    }

    pub fn insert(&mut self, tile: TileId, obstacles: Obstacles<'a>) {
        let (scene_tile, widest) = SceneTile::new(obstacles, self.lattice.tile_offset(tile));
        self.widest_footprint_steps = self.widest_footprint_steps.max(widest);
        self.put(tile, Slot::Read(scene_tile));
    }

    /// A tile that was read and has no obstacles file.
    pub fn insert_empty(&mut self, tile: TileId) {
        self.put(tile, Slot::Empty);
    }

    fn slot_index(&self, offset: [i64; 2]) -> Option<usize> {
        let side = 2 * self.radius + 1;
        (offset[0].abs() <= self.radius && offset[1].abs() <= self.radius)
            .then(|| ((offset[1] + self.radius) * side + offset[0] + self.radius) as usize)
    }

    /// Stores a slot, growing the square of slots to reach it.
    fn put(&mut self, tile: TileId, slot: Slot<'a>) {
        let tiles = i64::from(TILES_PER_AXIS);
        let dx = (i64::from(tile.x) - i64::from(self.centre.x) + tiles / 2).rem_euclid(tiles)
            - tiles / 2;
        let offset = [dx, i64::from(tile.y) - i64::from(self.centre.y)];
        let needed = offset[0].abs().max(offset[1].abs());
        if needed > self.radius {
            let (old_radius, old_slots) = (self.radius, std::mem::take(&mut self.slots));
            let side = 2 * needed + 1;
            self.radius = needed;
            self.slots = (0..side * side).map(|_| Slot::NotRead).collect();
            let old_side = 2 * old_radius + 1;
            for (index, old) in old_slots.into_iter().enumerate() {
                let index = index as i64;
                let old_offset = [index % old_side - old_radius, index / old_side - old_radius];
                let new_index = self.slot_index(old_offset).expect("the square only grows");
                self.slots[new_index] = old;
            }
        }
        let index = self.slot_index(offset).expect("grown to reach the tile");
        self.slots[index] = slot;
    }

    /// Where a scene cell lies: in a read tile (with the cell's index there), in a tile read
    /// empty or beyond the poles, or in a tile not read.
    fn locate(&self, cell: [i64; 2]) -> Located<'_, 'a> {
        let column = self.origin_cell_in_centre[0] + cell[0];
        let row = self.origin_cell_in_centre[1] + cell[1];
        let offset = [
            column.div_euclid(CELLS_PER_SIDE),
            row.div_euclid(CELLS_PER_SIDE),
        ];
        let y = i64::from(self.centre.y) + offset[1];
        if !(0..i64::from(TILES_PER_AXIS)).contains(&y) {
            return Located::Empty;
        }
        match self.slot_index(offset).map(|index| &self.slots[index]) {
            Some(Slot::Read(tile)) => {
                let index = row.rem_euclid(CELLS_PER_SIDE) * CELLS_PER_SIDE
                    + column.rem_euclid(CELLS_PER_SIDE);
                Located::Read(tile, index as usize)
            }
            Some(Slot::Empty) => Located::Empty,
            _ => {
                let tiles = i64::from(TILES_PER_AXIS);
                let x = (i64::from(self.centre.x) + offset[0]).rem_euclid(tiles);
                Located::NotRead(TileId {
                    x: x as u32,
                    y: y as u32,
                })
            }
        }
    }

    /// The read tile holding a scene cell and the cell's index there; `Ok(None)` for an empty
    /// tile or beyond the poles, an error for a tile that was not read.
    fn tile_at(&self, cell: [i64; 2]) -> Result<Option<(&SceneTile<'a>, usize)>, String> {
        match self.locate(cell) {
            Located::Read(tile, index) => Ok(Some((tile, index))),
            Located::Empty => Ok(None),
            Located::NotRead(tile) => Err(format!(
                "obstacles of tile {}/{} were not read",
                tile.x, tile.y
            )),
        }
    }

    /// Every read tile with obstacles, ordered by tile id (whatever the click).
    fn read_tiles(&self) -> Vec<(TileId, &SceneTile<'a>)> {
        let (side, tiles) = (2 * self.radius + 1, i64::from(TILES_PER_AXIS));
        let mut read: Vec<_> = (self.slots.iter().enumerate())
            .filter_map(|(index, slot)| match slot {
                Slot::Read(tile) => {
                    let (dx, dy) = (
                        index as i64 % side - self.radius,
                        index as i64 / side - self.radius,
                    );
                    let x = (i64::from(self.centre.x) + dx).rem_euclid(tiles) as u32;
                    Some((
                        TileId {
                            x,
                            y: (i64::from(self.centre.y) + dy) as u32,
                        },
                        tile,
                    ))
                }
                _ => None,
            })
            .collect();
        read.sort_unstable_by_key(|(tile, _)| *tile);
        read
    }
}
