//! Builds `obstacles` tiles from dev4 structures (the rows and footprint ids in
//! [`crate::structures`], the rules in [`crate::screening`]): for each target square, the outlines of the square and of its eight
//! neighbours (whose outlines cross into its border tiles), each stored whole in every child tile
//! whose cells it crosses. An outline that does not fit a tile's int16 frame fails the build.

use crate::dev4::{Dev4, Square};
use crate::output::write_tile;
use crate::screening::ScreeningOutline;
use crate::structures::read_square;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use tiles::Kind;
use tiles::geo::{GlobalSteps, STEPS_PER_TILE, TileId};
use tiles::obstacles::{MAXIMUM_HEIGHT_M, MAXIMUM_VERTICES, Outline, encode, tiles_crossed};

/// z12 tiles per z9 square side.
const TILES_PER_SQUARE_SIDE: u32 = 8;

/// Writes the obstacles tiles of `squares`; returns the number written (empty tiles get no file).
pub fn build(dev4: &Dev4, squares: &[Square], out: &Path) -> Result<usize, String> {
    let mut written = 0;
    for &square in squares {
        let side = i64::from(TILES_PER_SQUARE_SIDE) * STEPS_PER_TILE as i64;
        let (west, north) = (i64::from(square.x) * side, i64::from(square.y) * side);
        // An outline can cross the square's cells only if its box meets the square's.
        let near = |vertices: &[GlobalSteps]| {
            let (min, max) =
                vertices
                    .iter()
                    .fold(([i64::MAX; 2], [i64::MIN; 2]), |(min, max), v| {
                        (
                            [min[0].min(v.x), min[1].min(v.y)],
                            [max[0].max(v.x), max[1].max(v.y)],
                        )
                    });
            min[0] <= west + side && max[0] >= west && min[1] <= north + side && max[1] >= north
        };
        let mut outlines = Vec::new();
        for owner in square.with_neighbours() {
            outlines.extend(read_square(dev4, owner, west + side / 2, &near)?);
        }
        let crossed: Vec<Vec<TileId>> = outlines
            .par_iter()
            .map(|outline| tiles_crossed(&outline.vertices))
            .collect();
        let mut members: HashMap<TileId, Vec<usize>> = HashMap::new();
        for (index, tiles) in crossed.iter().enumerate() {
            for &tile in tiles
                .iter()
                .filter(|tile| (tile.x >> 3, tile.y >> 3) == (square.x, square.y))
            {
                members.entry(tile).or_default().push(index);
            }
        }
        drop(crossed);
        written += members
            .par_iter()
            .map(|(&tile, members)| {
                let bytes = encode(&tile_outlines(tile, &outlines, members)?);
                write_tile(out, tile, Kind::Obstacles, &bytes).map(|()| 1)
            })
            .collect::<Result<Vec<usize>, String>>()?
            .into_iter()
            .sum::<usize>();
    }
    Ok(written)
}

/// A tile's outlines in its int16 frame, sorted by footprint id with each footprint's rings in
/// their stored order.
fn tile_outlines(
    tile: TileId,
    outlines: &[ScreeningOutline],
    members: &[usize],
) -> Result<Vec<Outline>, String> {
    let mut order = members.to_vec();
    order.sort_unstable_by_key(|&index| (outlines[index].footprint_id, index));
    order
        .into_iter()
        .map(|index| {
            let outline = &outlines[index];
            let fail = |what: &str| {
                format!(
                    "obstacles {}/{}: footprint {:#x} {what}",
                    tile.x, tile.y, outline.footprint_id
                )
            };
            if outline.vertices.len() > MAXIMUM_VERTICES {
                return Err(fail("has more vertices than an outline holds"));
            }
            if outline.height_m > MAXIMUM_HEIGHT_M {
                return Err(fail("is taller than an outline holds"));
            }
            let vertices = outline
                .vertices
                .iter()
                .map(|&vertex| tile.local(vertex))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| fail("does not fit the tile's int16 frame"))?;
            Ok(Outline {
                footprint_id: outline.footprint_id,
                kind: outline.kind,
                envelope: outline.envelope,
                height_m: outline.height_m,
                vertices,
            })
        })
        .collect()
}
