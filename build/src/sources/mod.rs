//! Builds `sources` tiles: every ground source owned by a tile (the tile holding its midpoint, or
//! its point) with its emission per band and period, from one converter per layer, and one display
//! table per tile with identical records stored once.

pub mod airport;
pub mod building;
pub mod cells;
pub mod country_speeds;
pub mod facilities;
pub mod industry;
pub mod leisure;
pub mod rail;
pub mod road;
pub mod road_junctions;
pub mod road_slope;
pub mod ship;

use crate::climate::Temperature;
use crate::dev4::{Dev4, Square};
use crate::output::write_tile;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use tiles::geo::{GlobalSteps, TileId};
use tiles::sources::{Attribute, Piece, attribute_key, encode};
use tiles::{COMPLETION_MARKER, Kind};

/// int16 steps per tile and the world's width in steps.
const TILE_STEPS: i64 = 32_768;
const WORLD_STEPS: i64 = TILE_STEPS << tiles::geo::ZOOM;

/// The global steps of a z9 square (inclusive): a neighbour's rows matter to the square being
/// built only where they reach into it, and a converter may skip the others before converting
/// them (a road row is some metres long, a square tens of kilometres).
#[derive(Debug, Clone, Copy)]
pub struct Reach {
    min: GlobalSteps,
    max: GlobalSteps,
}

impl Reach {
    pub fn of_square(square: Square) -> Self {
        let side = TILE_STEPS * 8;
        let (x, y) = (i64::from(square.x) * side, i64::from(square.y) * side);
        Reach {
            min: GlobalSteps { x, y },
            max: GlobalSteps {
                x: x + side - 1,
                y: y + side - 1,
            },
        }
    }

    /// Whether the box of a piece from `a` to `b` meets the square (a piece across the
    /// antimeridian spans the world here, so it is always kept).
    pub fn touches(&self, a: GlobalSteps, b: GlobalSteps) -> bool {
        a.x.min(b.x) <= self.max.x
            && a.x.max(b.x) >= self.min.x
            && a.y.min(b.y) <= self.max.y
            && a.y.max(b.y) >= self.min.y
    }
}

/// One converted piece with its owner tile and attribute.
pub struct Converted {
    pub tile: TileId,
    pub ends: [[i16; 2]; 2],
    pub attribute: Attribute,
}

/// A straight piece split at tile edges into parts that each lie inside one tile, so a ring's
/// sources never reach into a ring not yet read. Each part's ends are clamped inside its tile
/// (local -16,384..=16,383; the far edge belongs to the neighbour), losing under one step.
pub fn split_at_tile_edges(start: GlobalSteps, end: GlobalSteps) -> Vec<(TileId, [[i16; 2]; 2])> {
    let dx = (end.x - start.x + WORLD_STEPS / 2).rem_euclid(WORLD_STEPS) - WORLD_STEPS / 2;
    let dy = end.y - start.y;
    let mut cuts = vec![0.0, 1.0];
    for (from, delta) in [(start.x, dx), (start.y, dy)] {
        if delta != 0 {
            let (lo, hi) = (from.min(from + delta), from.max(from + delta));
            let mut edge = (lo.div_euclid(TILE_STEPS) + 1) * TILE_STEPS;
            while edge < hi {
                cuts.push((edge - from) as f64 / delta as f64);
                edge += TILE_STEPS;
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    let at = |t: f64| {
        [
            start.x as f64 + t * dx as f64,
            start.y as f64 + t * dy as f64,
        ]
    };
    let mut parts = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b, middle) = (at(pair[0]), at(pair[1]), at(0.5 * (pair[0] + pair[1])));
        let tile = TileId::containing(
            GlobalSteps {
                x: middle[0].floor() as i64,
                y: middle[1].floor() as i64,
            }
            .to_mercator(),
        );
        let centre = tile.centre_steps();
        let local = |p: [f64; 2]| {
            let x = (p[0].round() as i64 - centre.x + WORLD_STEPS / 2).rem_euclid(WORLD_STEPS)
                - WORLD_STEPS / 2;
            [
                x.clamp(-16_384, 16_383) as i16,
                (p[1].round() as i64 - centre.y).clamp(-16_384, 16_383) as i16,
            ]
        };
        let ends = [local(a), local(b)];
        if ends[0] != ends[1] {
            parts.push((tile, ends));
        }
    }
    parts
}

/// A display group's key, stable across tiles and builds: FNV-1a over the parts.
pub fn group_key(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Writes the sources tiles of `squares`, with the airport ground operations of the complete
/// traffic pass under `airport_traffic`. A dev4 row is owned by the dev4 square of its midpoint,
/// which may round into a neighbour's tile here, and an area source spreads over its site, so each
/// square also converts its neighbours and keeps the sources its own tiles own (industry reads the
/// neighbours itself, for its facility joins). Returns the number of tiles written.
pub fn build(
    (dev4, temperature): (&Dev4, &Temperature),
    airport_traffic: &Path,
    squares: &[Square],
    out: &Path,
) -> Result<usize, String> {
    if !airport_traffic.join(COMPLETION_MARKER).exists() {
        return Err(format!(
            "{}: no complete airport traffic",
            airport_traffic.display()
        ));
    }
    squares
        .par_iter()
        .map(|&square| build_square((dev4, temperature), airport_traffic, square, out))
        .sum()
}

/// The sources tiles of one square (the squares build in parallel, each within its own memory).
fn build_square(
    (dev4, temperature): (&Dev4, &Temperature),
    airport_traffic: &Path,
    square: Square,
    out: &Path,
) -> Result<usize, String> {
    let owned = |item: &Converted| (item.tile.x >> 3, item.tile.y >> 3) == (square.x, square.y);
    let mut converted = Vec::new();
    let reach = Reach::of_square(square);
    for neighbour in square.with_neighbours() {
        let road_reach = (neighbour != square).then_some(reach);
        road::convert((dev4, temperature), neighbour, road_reach, &mut converted)?;
        rail::convert(dev4, neighbour, &mut converted)?;
        leisure::convert(dev4, neighbour, &mut converted)?;
        building::convert(dev4, neighbour, &mut converted)?;
        ship::convert(dev4, neighbour, &mut converted)?;
        airport::convert(airport_traffic, neighbour, &mut converted)?;
        converted.retain(owned);
    }
    industry::convert(dev4, square, &mut converted)?;
    converted.retain(owned);
    let mut by_tile: BTreeMap<TileId, Vec<Converted>> = BTreeMap::new();
    for item in converted {
        by_tile.entry(item.tile).or_default().push(item);
    }
    let mut written = 0;
    for (tile, items) in by_tile {
        let mut attributes: Vec<Attribute> = Vec::new();
        let mut index: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut pieces = Vec::with_capacity(items.len());
        for Converted {
            ends, attribute, ..
        } in items
        {
            let attribute = *index.entry(attribute_key(&attribute)).or_insert_with(|| {
                attributes.push(attribute);
                (attributes.len() - 1) as u32
            });
            pieces.push(Piece { ends, attribute });
        }
        write_tile(out, tile, Kind::Sources, &encode(&pieces, &attributes))?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A neighbour's row counts when its box meets the square: inside, across the edge, or
    /// touching it; a row one step outside does not.
    #[test]
    fn a_reach_keeps_the_rows_that_meet_its_square() {
        let reach = Reach::of_square(Square { x: 276, y: 173 });
        let west = 276 * 8 * TILE_STEPS;
        let north = 173 * 8 * TILE_STEPS;
        let at = |x: i64, y: i64| GlobalSteps { x, y };
        assert!(reach.touches(at(west + 5, north + 5), at(west + 50, north + 9)));
        assert!(reach.touches(at(west - 40, north + 5), at(west + 10, north + 5)));
        assert!(reach.touches(at(west - 40, north + 5), at(west, north + 5)));
        assert!(!reach.touches(at(west - 40, north + 5), at(west - 1, north + 5)));
        let south = north + 8 * TILE_STEPS;
        assert!(!reach.touches(at(west + 5, south), at(west + 9, south + 30)));
        assert!(reach.touches(at(west + 5, south - 1), at(west + 9, south + 30)));
    }

    #[test]
    fn pieces_split_at_tile_edges_stay_inside_their_tiles() {
        let tile = TileId { x: 2212, y: 1387 };
        let (start, end) = (tile.global([16_000, 0]), tile.global([17_000, 100]));
        let parts = split_at_tile_edges(start, end);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, tile);
        assert_eq!(parts[0].1[0], [16_000, 0]);
        assert_eq!(parts[0].1[1][0], 16_383, "clamped inside the owner tile");
        assert_eq!(parts[1].0, TileId { x: 2213, y: 1387 });
        assert_eq!(parts[1].1[0][0], -16_384);
        assert_eq!(
            split_at_tile_edges(start, tile.global([16_100, 50])).len(),
            1
        );
        // Across the antimeridian: two parts, one on each side.
        let (west_end, east_start) = (TileId { x: 4095, y: 5 }, TileId { x: 0, y: 5 });
        let parts = split_at_tile_edges(
            west_end.global([16_000, 0]),
            east_start.global([-16_000, 0]),
        );
        assert_eq!(
            parts.iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![west_end, east_start]
        );
    }
}
