//! Where a flight piece belongs: per z12 tile the clearance bands of its latitude, the highest
//! terrain within one edge of a cell (from a max-pyramid of the terrain built once), the box of a
//! point (the highest band whose floor, counted above that terrain, it reaches), and a segment cut
//! into the pieces of consecutive boxes.

use physics::doc29::box_geometry::{ClearanceBand, clearance_bands};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use tiles::geo::{Mercator, TILES_PER_AXIS, TileId};
use tiles::terrain::Terrain;

/// The equatorial circumference the web-map cell edges derive from (m).
const EQUATOR_M: f64 = 40_075_016.686;
/// Bands reach this clearance; anything higher joins the top band.
pub const TOP_CLEARANCE_M: f64 = 20_000.0;
/// Samples per box edge along a segment: pieces end within a quarter edge of a box boundary.
const SAMPLES_PER_EDGE: f64 = 4.0;
/// Terrain samples of a finest cell are this far apart at most (m): the lattice spacing.
const TERRAIN_SAMPLE_M: f64 = 30.0;

/// One box: the z12 tile, the band (its zoom, clearance and edge follow from the tile's table),
/// the cell at the band's zoom (global cell coordinates) and the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoxKey {
    pub tile: TileId,
    pub band: u8,
    pub cell: [u32; 2],
    pub helicopter: bool,
}

/// The web-map cell edge (m) at `zoom` on the latitude of `tile`'s centre.
pub fn cell_edge_m(tile: TileId, zoom: u8) -> f64 {
    let (lat, _) = tile.centre().to_degrees();
    EQUATOR_M * lat.to_radians().cos() / f64::from(1u32 << zoom)
}

/// The clearance bands of a tile.
pub fn tile_bands(tile: TileId) -> Vec<ClearanceBand> {
    clearance_bands(|zoom| cell_edge_m(tile, zoom), 12, TOP_CLEARANCE_M)
}

/// A tile's highest terrain per cell, from its finest band's zoom up to the tile itself:
/// `levels[k]` is the zoom 12 + k grid, 2^k cells a side, row-major from the north-west; and the
/// same grids holding the highest terrain within one edge of each cell (its 3 x 3 cells).
struct TileGround {
    levels: Vec<Vec<f32>>,
    within_one_edge: Vec<Vec<f32>>,
}

impl TileGround {
    fn build(tile: TileId, finest_zoom: u8, terrain: &HashMap<TileId, Terrain<'_>>) -> Self {
        let finest = usize::from(finest_zoom - 12);
        let side = 1usize << finest;
        let per_axis =
            ((cell_edge_m(tile, finest_zoom) / TERRAIN_SAMPLE_M).ceil() as usize).max(1) + 1;
        let cell_units = 1.0 / side as f64;
        let height = |position: Mercator| {
            terrain
                .get(&TileId::containing(position))
                .and_then(|terrain| terrain.sample(position))
                .map_or(0.0, |sample| sample.height_m)
        };
        let finest_grid: Vec<f32> = (0..side * side)
            .map(|index| {
                let (row, column) = (index / side, index % side);
                let mut highest = f64::NEG_INFINITY;
                for i in 0..per_axis {
                    for j in 0..per_axis {
                        let fraction = |k: usize| k as f64 / (per_axis - 1) as f64;
                        highest = highest.max(height(Mercator {
                            x: f64::from(tile.x) + (column as f64 + fraction(j)) * cell_units,
                            y: f64::from(tile.y) + (row as f64 + fraction(i)) * cell_units,
                        }));
                    }
                }
                highest as f32
            })
            .collect();
        let mut levels = vec![finest_grid];
        for k in (0..finest).rev() {
            let finer = levels.last().expect("the finest level");
            let (side, finer_side) = (1usize << k, 1usize << (k + 1));
            let coarser = (0..side * side)
                .map(|index| {
                    let (row, column) = (index / side, index % side);
                    let at =
                        |r: usize, c: usize| finer[(2 * row + r) * finer_side + 2 * column + c];
                    at(0, 0).max(at(0, 1)).max(at(1, 0)).max(at(1, 1))
                })
                .collect();
            levels.push(coarser);
        }
        levels.reverse();
        TileGround {
            levels,
            within_one_edge: Vec::new(),
        }
    }
}

/// The highest terrain within one edge of `cell` (global cells at `zoom`) from its own tile's
/// grids; a zoom finer than the tile's finest reads the parent cell.
fn ground_within_one_edge(ground: &TileGround, zoom: u8, cell: [u32; 2]) -> f64 {
    let level = usize::from(zoom - 12).min(ground.within_one_edge.len() - 1);
    let shift = usize::from(zoom - 12) - level;
    let per_tile = 1u32 << (zoom - 12);
    let side = 1u32 << level;
    let (column, row) = ((cell[0] % per_tile) >> shift, (cell[1] % per_tile) >> shift);
    f64::from(ground.within_one_edge[level][(row * side + column) as usize])
}

/// The bands of every tile row (they depend on the latitude alone) and the terrain pyramid of the
/// tiles near the flights, built once and shared.
pub struct Placement {
    bands: Vec<std::sync::OnceLock<Vec<ClearanceBand>>>,
    ground: HashMap<TileId, TileGround>,
}

impl Placement {
    /// For `tiles` (the boxes' tiles and a ring around them), from the terrain read for them.
    pub fn new(tiles: &HashSet<TileId>, terrain: &HashMap<TileId, Terrain<'_>>) -> Self {
        let mut placement = Placement {
            bands: (0..TILES_PER_AXIS)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
            ground: HashMap::new(),
        };
        let built: Vec<(TileId, TileGround)> = tiles
            .par_iter()
            .map(|&tile| {
                let finest = placement
                    .bands(tile)
                    .iter()
                    .map(|band| band.zoom)
                    .max()
                    .expect("a band");
                (tile, TileGround::build(tile, finest, terrain))
            })
            .collect();
        placement.ground.extend(built);
        let neighbourhoods: Vec<(TileId, Vec<Vec<f32>>)> = placement
            .ground
            .par_iter()
            .map(|(&tile, ground)| {
                let levels = (0..ground.levels.len())
                    .map(|level| {
                        let side = 1i64 << level;
                        let zoom = 12 + level as u8;
                        (0..side * side)
                            .map(|index| {
                                let cell = [
                                    i64::from(tile.x) * side + index % side,
                                    i64::from(tile.y) * side + index / side,
                                ];
                                let mut highest = f64::NEG_INFINITY;
                                for dy in -1..=1 {
                                    for dx in -1..=1 {
                                        let neighbour = [cell[0] + dx, cell[1] + dy];
                                        highest =
                                            highest.max(placement.cell_ground_m(zoom, neighbour));
                                    }
                                }
                                highest as f32
                            })
                            .collect()
                    })
                    .collect();
                (tile, levels)
            })
            .collect();
        for (tile, levels) in neighbourhoods {
            placement
                .ground
                .get_mut(&tile)
                .expect("a prepared tile")
                .within_one_edge = levels;
        }
        placement
    }

    /// The clearance bands of a tile (of its row: they depend on the latitude alone).
    pub fn bands(&self, tile: TileId) -> &[ClearanceBand] {
        self.bands[tile.y as usize].get_or_init(|| tile_bands(tile))
    }

    /// The highest terrain of one cell at `zoom` (0 beyond the prepared tiles).
    fn cell_ground_m(&self, zoom: u8, cell: [i64; 2]) -> f64 {
        let per_tile = 1i64 << (zoom - 12);
        let world = i64::from(TILES_PER_AXIS) * per_tile;
        if !(0..world).contains(&cell[1]) {
            return 0.0;
        }
        let x = cell[0].rem_euclid(world);
        let tile = TileId {
            x: (x / per_tile) as u32,
            y: (cell[1] / per_tile) as u32,
        };
        let Some(ground) = self.ground.get(&tile) else {
            return 0.0;
        };
        // A tile whose finest zoom is coarser than `zoom` answers with the parent cell.
        let level = usize::from(zoom - 12).min(ground.levels.len() - 1);
        let shift = usize::from(zoom - 12) - level;
        let side = 1i64 << level;
        let (column, row) = ((x % per_tile) >> shift, (cell[1] % per_tile) >> shift);
        f64::from(ground.levels[level][(row * side + column) as usize])
    }

    /// The box of a point (Mercator position, altitude above sea level): the highest band whose
    /// floor, above the highest terrain within one edge of its cell, the point reaches (band 0 at
    /// or below it), with that band's terrain.
    pub fn box_of(&self, position: Mercator, altitude_m: f64, helicopter: bool) -> (BoxKey, f64) {
        let tile = TileId::containing(position);
        let bands = self.bands(tile);
        let ground = self.ground.get(&tile);
        let mut chosen = (0usize, [0u32; 2], 0.0);
        for (index, band) in bands.iter().enumerate() {
            let scale = 1u32 << (band.zoom - 12);
            let cell = [
                ((position.x * f64::from(scale)).floor() as u32).min(TILES_PER_AXIS * scale - 1),
                ((position.y * f64::from(scale)).floor() as u32).min(TILES_PER_AXIS * scale - 1),
            ];
            let terrain_m = ground.map_or(0.0, |ground| {
                ground_within_one_edge(ground, band.zoom, cell)
            });
            if index > 0 && altitude_m - terrain_m < band.clearance_m {
                break;
            }
            chosen = (index, cell, terrain_m);
        }
        let key = BoxKey {
            tile,
            band: chosen.0 as u8,
            cell: chosen.1,
            helicopter,
        };
        (key, chosen.2)
    }

    /// The edge (m) of a box.
    pub fn edge_m(&self, key: &BoxKey) -> f64 {
        self.bands(key.tile)[usize::from(key.band)].edge_m
    }
}

/// One piece of a segment inside one box: its ends (Mercator, altitude above sea level).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxPiece {
    pub key: BoxKey,
    pub ground_m: f64,
    pub start: (Mercator, f64),
    pub end: (Mercator, f64),
}

/// A segment from `start` to `end` (Mercator, altitude above sea level) cut into the pieces of
/// the boxes it crosses: sampled every quarter of the current box's edge, each stretch between
/// two samples going to the box of its middle, consecutive stretches of one box joined.
pub fn cut_into_pieces(
    placement: &Placement,
    start: (Mercator, f64),
    end: (Mercator, f64),
    helicopter: bool,
) -> Vec<BoxPiece> {
    let at = |t: f64| {
        (
            Mercator {
                x: start.0.x + t * (end.0.x - start.0.x),
                y: start.0.y + t * (end.0.y - start.0.y),
            },
            start.1 + t * (end.1 - start.1),
        )
    };
    let metres_per_unit = cell_edge_m(TileId::containing(start.0), 12);
    let length_m = (end.0.x - start.0.x).hypot(end.0.y - start.0.y) * metres_per_unit;
    let mut pieces: Vec<BoxPiece> = Vec::new();
    let mut t = 0.0;
    let mut step_m = {
        let (probe, probe_altitude) = at(0.0);
        placement.edge_m(&placement.box_of(probe, probe_altitude, helicopter).0) / SAMPLES_PER_EDGE
    };
    while t < 1.0 {
        let next = if length_m > 0.0 {
            (t + step_m / length_m).min(1.0)
        } else {
            1.0
        };
        let (middle, middle_altitude) = at(0.5 * (t + next));
        let (key, ground_m) = placement.box_of(middle, middle_altitude, helicopter);
        step_m = placement.edge_m(&key) / SAMPLES_PER_EDGE;
        match pieces.last_mut() {
            Some(piece) if piece.key == key => piece.end = at(next),
            _ => pieces.push(BoxPiece {
                key,
                ground_m,
                start: at(t),
                end: at(next),
            }),
        }
        t = next;
    }
    pieces
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
