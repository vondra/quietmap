//! Where a flight piece belongs: per z12 tile the clearance bands of its latitude, the highest
//! terrain within one edge of a cell (from a max-pyramid of the terrain built once), the box of a
//! point (the highest band whose floor, counted above that terrain, it reaches), and a segment cut
//! into the pieces of consecutive boxes.

use physics::doc29::box_geometry::{ClearanceBand, clearance_bands};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
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

/// The clearance bands of a tile for a level step D of `level_step_db`.
pub fn tile_bands(tile: TileId, level_step_db: f64) -> Vec<ClearanceBand> {
    clearance_bands(
        |zoom| cell_edge_m(tile, zoom),
        12,
        TOP_CLEARANCE_M,
        level_step_db,
    )
}

/// A tile's highest terrain per cell, from its finest band's zoom up to the tile itself:
/// `levels[k]` is the zoom 12 + k grid, 2^k cells a side, row-major from the north-west; the same
/// grids holding the highest terrain within one edge of each cell (its 3 x 3 cells); and per grid
/// the highest of those, over which a point clears a band anywhere in the tile.
struct TileGround {
    levels: Vec<Vec<f32>>,
    within_one_edge: Vec<Vec<f32>>,
    highest_within_one_edge: Vec<f64>,
}

impl TileGround {
    fn build(tile: TileId, finest_zoom: u8, terrain: &HashMap<TileId, Terrain>) -> Self {
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
        // Each lattice point sampled once: a point on the edge between two cells is both cells'.
        let steps = per_axis - 1;
        let lattice = side * steps + 1;
        let along = |index: usize| (index / steps) as f64 + (index % steps) as f64 / steps as f64;
        let heights: Vec<f64> = (0..lattice * lattice)
            .map(|index| {
                height(Mercator {
                    x: f64::from(tile.x) + along(index % lattice) * cell_units,
                    y: f64::from(tile.y) + along(index / lattice) * cell_units,
                })
            })
            .collect();
        let finest_grid: Vec<f32> = (0..side * side)
            .map(|index| {
                let (row, column) = (index / side, index % side);
                let mut highest = f64::NEG_INFINITY;
                for i in 0..per_axis {
                    for j in 0..per_axis {
                        highest =
                            highest.max(heights[(row * steps + i) * lattice + column * steps + j]);
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
            highest_within_one_edge: Vec::new(),
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

/// The highest terrain of one cell at `zoom` (0 beyond the prepared tiles).
fn cell_ground_m(ground: &Ground, zoom: u8, cell: [i64; 2]) -> f64 {
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
    let Some(tile_ground) = ground.get(&tile) else {
        return 0.0;
    };
    // A tile whose finest zoom is coarser than `zoom` answers with the parent cell.
    let level = usize::from(zoom - 12).min(tile_ground.levels.len() - 1);
    let shift = usize::from(zoom - 12) - level;
    let side = 1i64 << level;
    let (column, row) = ((x % per_tile) >> shift, (cell[1] % per_tile) >> shift);
    f64::from(tile_ground.levels[level][(row * side + column) as usize])
}

/// The terrain pyramids of the tiles near the flights.
type Ground = HashMap<TileId, TileGround, BuildHasherDefault<TileHasher>>;

/// The bands of every tile row (they depend on the latitude alone) and the terrain pyramid of the
/// tiles near the flights, built once and shared by the level steps.
pub struct Placement {
    level_step_db: f64,
    bands: Vec<std::sync::OnceLock<Vec<ClearanceBand>>>,
    ground: std::sync::Arc<Ground>,
}

/// Hashes tiles and boxes word by word, mixed once at the end: a sample's tile is looked up for
/// each of its bands' terrain, where the default SipHash cost as much as the band climb.
#[derive(Default)]
pub struct TileHasher(u64);

impl Hasher for TileHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn finish(&self) -> u64 {
        // The splitmix64 finalizer: every bit of the words reaches the low bits a table indexes by.
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

impl Placement {
    /// For `tiles` (the boxes' tiles and a ring around them), from the terrain read for them, with
    /// the bands of a level step D of `level_step_db`.
    pub fn new(
        tiles: &HashSet<TileId>,
        terrain: &HashMap<TileId, Terrain>,
        level_step_db: f64,
    ) -> Self {
        let mut placement = Placement {
            level_step_db,
            bands: (0..TILES_PER_AXIS)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
            ground: Default::default(),
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
        let mut ground = Ground::default();
        ground.extend(built);
        let neighbourhoods: Vec<(TileId, Vec<Vec<f32>>)> = ground
            .par_iter()
            .map(|(&tile, tile_ground)| {
                let levels = (0..tile_ground.levels.len())
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
                                            highest.max(cell_ground_m(&ground, zoom, neighbour));
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
            let tile_ground = ground.get_mut(&tile).expect("a prepared tile");
            tile_ground.highest_within_one_edge = levels
                .iter()
                .map(|grid| f64::from(grid.iter().copied().fold(f32::NEG_INFINITY, f32::max)))
                .collect();
            tile_ground.within_one_edge = levels;
        }
        placement.ground = std::sync::Arc::new(ground);
        placement
    }

    /// The same ground with the bands of a level step D of `level_step_db`: the lowest band of
    /// every step takes the first layer's zoom (`clearance_bands`), the zoom of the ground's finest
    /// grid, so the ground is the one the step would build (a test holds it on every row).
    pub fn with_level_step(&self, level_step_db: f64) -> Self {
        Placement {
            level_step_db,
            bands: (0..TILES_PER_AXIS)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
            ground: std::sync::Arc::clone(&self.ground),
        }
    }

    /// The clearance bands of a tile (of its row: they depend on the latitude alone).
    pub fn bands(&self, tile: TileId) -> &[ClearanceBand] {
        self.bands[tile.y as usize].get_or_init(|| tile_bands(tile, self.level_step_db))
    }

    /// The box of a point (Mercator position, altitude above sea level): the highest band whose
    /// floor, above the highest terrain within one edge of its cell, the point reaches (band 0 at
    /// or below it), with that band's terrain.
    pub fn box_of(&self, position: Mercator, altitude_m: f64, helicopter: bool) -> (BoxKey, f64) {
        let tile = TileId::containing(position);
        let bands = self.bands(tile);
        let ground = self.ground.get(&tile);
        let cell_and_terrain = |band: &ClearanceBand| {
            let scale = 1u32 << (band.zoom - 12);
            let cell = [
                ((position.x * f64::from(scale)).floor() as u32).min(TILES_PER_AXIS * scale - 1),
                ((position.y * f64::from(scale)).floor() as u32).min(TILES_PER_AXIS * scale - 1),
            ];
            let terrain_m = ground.map_or(0.0, |ground| {
                ground_within_one_edge(ground, band.zoom, cell)
            });
            (cell, terrain_m)
        };
        // The first band the point does not clear ends the climb; a band it clears over the
        // tile's highest terrain at that zoom it clears in any cell.
        let mut chosen = 0;
        for (index, band) in bands.iter().enumerate().skip(1) {
            let highest_m = ground.map_or(0.0, |ground| {
                let level = usize::from(band.zoom - 12).min(ground.levels.len() - 1);
                ground.highest_within_one_edge[level]
            });
            if altitude_m - highest_m < band.clearance_m
                && altitude_m - cell_and_terrain(band).1 < band.clearance_m
            {
                break;
            }
            chosen = index;
        }
        let (cell, terrain_m) = cell_and_terrain(&bands[chosen]);
        let key = BoxKey {
            tile,
            band: chosen as u8,
            cell,
            helicopter,
        };
        (key, terrain_m)
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
/// the boxes it crosses: first at z12 tile edges (so a tile's pieces depend on nothing outside
/// it and its neighbours), then within each tile sampled every quarter of the edge of the box a
/// stretch's middle lies in, each stretch going to that box, consecutive stretches of one box
/// joined: a piece reaches past its box by an eighth of its edge at most.
pub fn cut_into_pieces(
    placement: &Placement,
    start: (Mercator, f64),
    end: (Mercator, f64),
    helicopter: bool,
) -> Vec<BoxPiece> {
    cut_where(placement, start, end, helicopter, |_, _| true)
}

/// The pieces [`cut_into_pieces`] cuts in `tile`, cutting only there: a stretch's middle lies
/// between the ends of its stretch between two tile edges (positions along a segment rise or fall
/// with the fraction, rounded alike), so a stretch whose ends' tiles do not frame `tile` has no
/// piece there; a tile's pieces follow each other along a segment, so the joins are the same.
pub fn cut_within_tile(
    placement: &Placement,
    start: (Mercator, f64),
    end: (Mercator, f64),
    helicopter: bool,
    tile: TileId,
) -> Vec<BoxPiece> {
    let between = |a: u32, b: u32, value: u32| a.min(b) <= value && value <= a.max(b);
    let mut pieces = cut_where(placement, start, end, helicopter, |from, to| {
        between(from.x, to.x, tile.x) && between(from.y, to.y, tile.y)
    });
    pieces.retain(|piece| piece.key.tile == tile);
    pieces
}

/// The cut of [`cut_into_pieces`] over the stretches between tile edges whose ends' tiles
/// `wanted` takes.
fn cut_where(
    placement: &Placement,
    start: (Mercator, f64),
    end: (Mercator, f64),
    helicopter: bool,
    wanted: impl Fn(TileId, TileId) -> bool,
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
    for pair in tile_crossings(start.0, end.0).windows(2) {
        let (mut t, until) = (pair[0], pair[1]);
        if !wanted(TileId::containing(at(t).0), TileId::containing(at(until).0)) {
            continue;
        }
        let mut step_m = {
            let (probe, probe_altitude) = at(0.5 * (t + until));
            placement.edge_m(&placement.box_of(probe, probe_altitude, helicopter).0)
                / SAMPLES_PER_EDGE
        };
        while t < until {
            // A stretch is a quarter of the edge of the box its middle lies in: where the step
            // carried from a coarser box puts the middle in a finer one, the stretch shortens to
            // the finer box's step, so no piece reaches past its box by more than an eighth.
            let (next, key, ground_m) = loop {
                let next = if length_m > 0.0 {
                    (t + step_m / length_m).min(until)
                } else {
                    until
                };
                let (middle, middle_altitude) = at(0.5 * (t + next));
                let (key, ground_m) = placement.box_of(middle, middle_altitude, helicopter);
                let box_step_m = placement.edge_m(&key) / SAMPLES_PER_EDGE;
                if box_step_m >= step_m || next == t {
                    break (next, key, ground_m);
                }
                step_m = box_step_m;
            };
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
    }
    pieces
}

/// The fractions 0, the crossings of z12 tile edges in order, and 1 along a segment.
fn tile_crossings(start: Mercator, end: Mercator) -> Vec<f64> {
    let mut crossings = vec![0.0, 1.0];
    for (from, to) in [(start.x, end.x), (start.y, end.y)] {
        let (low, high) = (from.min(to), from.max(to));
        let mut edge = low.floor() + 1.0;
        while edge < high {
            crossings.push((edge - from) / (to - from));
            edge += 1.0;
        }
    }
    crossings.sort_by(f64::total_cmp);
    crossings
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
