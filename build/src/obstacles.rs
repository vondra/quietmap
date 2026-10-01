//! Builds `obstacles` tiles from dev4 structures (the rows and footprint ids in
//! [`crate::structures`], the rules in [`crate::screening`]): for each target square, the outlines of the square and of its eight
//! neighbours (whose outlines cross into its border tiles), each stored whole in every child tile
//! whose cells it crosses. A wall longer than a tile's int16 frame (half a tile beyond each edge)
//! is stored there as its parts inside the frame; a building ring that large is a mapping error
//! (no building spans kilometres) and is left out, counted on standard error.

use crate::dev4::{Dev4, Square};
use crate::output::write_tile;
use crate::screening::ScreeningOutline;
use crate::structures::read_square;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tiles::Kind;
use tiles::geo::{GlobalSteps, STEPS_PER_TILE, TileId};
use tiles::obstacles::{
    MAXIMUM_HEIGHT_M, MAXIMUM_VERTICES, Outline, OutlineKind, encode, tiles_crossed,
};

/// z12 tiles per z9 square side.
const TILES_PER_SQUARE_SIDE: u32 = 8;
/// Squares built at once: each holds the outlines of nine squares (up to about 2 GB in a dense
/// city), and its reading is serial.
const SQUARES_AT_ONCE: usize = 8;

/// Writes the obstacles tiles of `squares`, [`SQUARES_AT_ONCE`] at a time; returns the number
/// written (empty tiles get no file).
pub fn build(dev4: &Dev4, squares: &[Square], out: &Path) -> Result<usize, String> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..SQUARES_AT_ONCE)
            .map(|_| {
                scope.spawn(|| {
                    let mut written = 0;
                    while let Some(&square) = squares.get(next.fetch_add(1, Ordering::Relaxed)) {
                        written += build_square(dev4, square, out)?;
                    }
                    Ok::<usize, String>(written)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "an obstacles worker panicked".to_string())?
            })
            .sum()
    })
}

/// Writes the obstacles tiles of one square; returns the number written.
fn build_square(dev4: &Dev4, square: Square, out: &Path) -> Result<usize, String> {
    let side = i64::from(TILES_PER_SQUARE_SIDE) * STEPS_PER_TILE as i64;
    let (west, north) = (i64::from(square.x) * side, i64::from(square.y) * side);
    // An outline can cross the square's cells only if its box meets the square's.
    let near = |vertices: &[GlobalSteps]| {
        let (min, max) = vertices
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
    members
        .par_iter()
        .map(|(&tile, members)| {
            let bytes = encode(&tile_outlines(tile, &outlines, members)?);
            write_tile(out, tile, Kind::Obstacles, &bytes).map(|()| 1)
        })
        .sum()
}

/// A tile's outlines in its int16 frame, sorted by footprint id with each footprint's rings in
/// their stored order (a wall cut by the frame as its parts, in order).
fn tile_outlines(
    tile: TileId,
    outlines: &[ScreeningOutline],
    members: &[usize],
) -> Result<Vec<Outline>, String> {
    let mut order = members.to_vec();
    order.sort_unstable_by_key(|&index| (outlines[index].footprint_id, index));
    let mut stored = Vec::with_capacity(order.len());
    let mut oversized = 0;
    for index in order {
        let outline = &outlines[index];
        let fail = |what: &str| {
            format!(
                "obstacles {}/{}: footprint {:#x} {what}",
                tile.x, tile.y, outline.footprint_id
            )
        };
        if outline.height_m > MAXIMUM_HEIGHT_M {
            return Err(fail("is taller than an outline holds"));
        }
        let whole = outline
            .vertices
            .iter()
            .map(|&vertex| tile.local(vertex))
            .collect::<Option<Vec<_>>>();
        let parts = match (whole, outline.kind) {
            (Some(vertices), _) if vertices.len() <= MAXIMUM_VERTICES => vec![vertices],
            (_, OutlineKind::Wall) => wall_parts(tile, &outline.vertices),
            (Some(_), _) => return Err(fail("has more vertices than an outline holds")),
            (None, _) => {
                oversized += 1;
                Vec::new()
            }
        };
        stored.extend(parts.into_iter().map(|vertices| Outline {
            footprint_id: outline.footprint_id,
            kind: outline.kind,
            envelope: outline.envelope,
            height_m: outline.height_m,
            vertices,
        }));
    }
    if oversized > 0 {
        eprintln!(
            "obstacles {}/{}: {oversized} building rings larger than the tile's frame left out",
            tile.x, tile.y
        );
    }
    Ok(stored)
}

/// The parts of a wall inside a tile's int16 frame: each edge clipped to the frame (Liang-Barsky),
/// the edges outside it dropped, consecutive kept edges joined, a part longer than an outline
/// holds cut into outlines that share their joints.
fn wall_parts(tile: TileId, vertices: &[GlobalSteps]) -> Vec<Vec<[i16; 2]>> {
    const WORLD_STEPS: i64 = (STEPS_PER_TILE as i64) << tiles::geo::ZOOM;
    let centre = tile.centre_steps();
    let local = |v: GlobalSteps| {
        [
            ((v.x - centre.x + WORLD_STEPS / 2).rem_euclid(WORLD_STEPS) - WORLD_STEPS / 2) as f64,
            (v.y - centre.y) as f64,
        ]
    };
    let (low, high) = (f64::from(i16::MIN + 1), f64::from(i16::MAX));
    let mut parts: Vec<Vec<[i16; 2]>> = Vec::new();
    let mut current: Vec<[i16; 2]> = Vec::new();
    for pair in vertices.windows(2) {
        let (a, b) = (local(pair[0]), local(pair[1]));
        let Some((t0, t1)) = clip(a, b, low, high) else {
            if current.len() >= 2 {
                parts.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        };
        let at = |t: f64| {
            [
                (a[0] + t * (b[0] - a[0])).round().clamp(low, high) as i16,
                (a[1] + t * (b[1] - a[1])).round().clamp(low, high) as i16,
            ]
        };
        let (start, end) = (at(t0), at(t1));
        if current.last() != Some(&start) {
            if current.len() >= 2 {
                parts.push(std::mem::take(&mut current));
            }
            current = vec![start];
        }
        if end != start {
            current.push(end);
        }
        if t1 < 1.0 {
            if current.len() >= 2 {
                parts.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    if current.len() >= 2 {
        parts.push(current);
    }
    // A part longer than an outline holds: outlines sharing their joints.
    let mut outlines = Vec::new();
    for part in parts {
        let mut from = 0;
        while from + 1 < part.len() {
            let to = (from + MAXIMUM_VERTICES).min(part.len());
            outlines.push(part[from..to].to_vec());
            from = to - 1;
        }
    }
    outlines
}

/// The parameters (`t0 <= t1` in 0..=1) of the segment `a`-`b` inside the box `low..=high` on both
/// axes, `None` outside it.
fn clip(a: [f64; 2], b: [f64; 2], low: f64, high: f64) -> Option<(f64, f64)> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for axis in 0..2 {
        let delta = b[axis] - a[axis];
        for (p, q) in [(-delta, a[axis] - low), (delta, high - a[axis])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_longer_than_the_frame_keeps_its_parts_inside() {
        let tile = TileId { x: 2426, y: 1619 };
        let centre = tile.centre_steps();
        let at = |dx: i64, dy: i64| GlobalSteps {
            x: centre.x + dx,
            y: centre.y + dy,
        };
        // A straight wall from 3 tiles west to 3 tiles east through the tile's centre.
        let wall = [at(-98_304, 0), at(0, 0), at(98_304, 0)];
        let parts = wall_parts(tile, &wall);
        assert_eq!(parts, vec![vec![[-32_767, 0], [0, 0], [32_767, 0]]]);
        // Leaving the frame and coming back: two parts.
        let detour = [at(-100, 0), at(-100, 50_000), at(100, 50_000), at(100, 0)];
        let parts = wall_parts(tile, &detour);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], vec![[-100, 0], [-100, 32_767]]);
        assert_eq!(parts[1], vec![[100, 32_767], [100, 0]]);
        // Wholly outside: nothing.
        assert!(wall_parts(tile, &[at(40_000, 0), at(50_000, 0)]).is_empty());
    }
}
