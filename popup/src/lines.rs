//! The whole of each shown contributor as the map draws it: every piece of its group within the
//! reach in the sources files read, not only the pieces evaluated (a road sampled or cut by the
//! stop rule would show with gaps), joined at shared ends into lines and simplified together, so
//! the final update stays within its budget (100 KB, ARCHITECTURE.md).

use crate::release::RingFiles;
use rayon::prelude::*;
use std::collections::HashMap;
use tiles::Kind;
use tiles::geo::{GlobalSteps, LocalFrame, TileId};
use tiles::sources::Sources;

/// Lines are simplified to this distance (Douglas-Peucker): under a pixel at street zoom.
const TOLERANCE_M: f64 = 1.0;
/// The most points the shown contributors send together (about 44 KB): their lines are simplified
/// with one tolerance, coarser until they fit, and point sources beyond it are left out, the
/// farthest first.
const MOST_POINTS: usize = 2_000;

/// A contributor's lines in click metres; a point source is a line of one point.
pub type Lines = Vec<Vec<[f64; 2]>>;

/// The lines of the groups `keys` (at most 255) within `reach_m` of `receiver` (click metres),
/// one entry per key in its order.
pub fn whole_lines(
    rings: &[&RingFiles],
    frame: &LocalFrame,
    keys: &[u64],
    receiver: [f64; 2],
    reach_m: f64,
) -> Result<Vec<Lines>, String> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    assert!(keys.len() < usize::from(u8::MAX));
    let mut sorted: Vec<(u64, u8)> = keys
        .iter()
        .enumerate()
        .map(|(position, &key)| (key, position as u8))
        .collect();
    sorted.sort_unstable();
    let files: Vec<(TileId, &[u8])> = rings
        .iter()
        .flat_map(|ring| {
            ring.tiles.iter().enumerate().filter_map(|(index, &tile)| {
                ring.file(index, Kind::Sources).map(|bytes| (tile, bytes))
            })
        })
        .collect();
    let found = files
        .par_iter()
        .map(|&(tile, bytes)| group_pieces(tile, bytes, &sorted))
        .collect::<Result<Vec<_>, String>>()?;
    let mut pieces: Vec<Vec<[GlobalSteps; 2]>> = vec![Vec::new(); keys.len()];
    for (position, ends) in found.into_iter().flatten() {
        pieces[usize::from(position)].push(ends);
    }
    let metres = |steps: GlobalSteps| frame.metres_of_steps([steps.x as f64, steps.y as f64]);
    let away = |at: [f64; 2]| (at[0] - receiver[0]).hypot(at[1] - receiver[1]);
    // Per group its chains (the lines before simplifying) and its point sources.
    let groups: Vec<(Lines, Vec<[f64; 2]>)> = pieces
        .into_par_iter()
        .map(|mut group| {
            group.retain(|ends| {
                let [a, b] = ends.map(metres);
                distance_to_segment(
                    [a[0] - receiver[0], a[1] - receiver[1]],
                    [b[0] - receiver[0], b[1] - receiver[1]],
                ) <= reach_m
            });
            let (lines, points): (Vec<_>, Vec<_>) =
                group.into_iter().partition(|ends| ends[0] != ends[1]);
            let chains = join(&lines)
                .into_iter()
                .map(|chain| chain.into_iter().map(metres).collect())
                .collect();
            (chains, points.iter().map(|ends| metres(ends[0])).collect())
        })
        .collect();
    let chains: Vec<&[Vec<[f64; 2]>]> = groups.iter().map(|(chains, _)| &chains[..]).collect();
    let mut out = simplified(&chains, MOST_POINTS);
    // Point sources fill the room left, the nearest first over every group.
    let mut points: Vec<(usize, [f64; 2])> = groups
        .iter()
        .enumerate()
        .flat_map(|(group, (_, points))| points.iter().map(move |&at| (group, at)))
        .collect();
    points.sort_by(|a, b| away(a.1).total_cmp(&away(b.1)).then(a.0.cmp(&b.0)));
    let room = MOST_POINTS.saturating_sub(out.iter().flatten().map(Vec::len).sum());
    for &(group, at) in points.iter().take(room) {
        out[group].push(vec![at]);
    }
    Ok(out)
}

/// The pieces of one sources file whose group is among `sorted` (key, position), as positions and
/// global ends.
fn group_pieces(
    tile: TileId,
    bytes: &[u8],
    sorted: &[(u64, u8)],
) -> Result<Vec<(u8, [GlobalSteps; 2])>, String> {
    let sources = Sources::parse(bytes).map_err(|error| error.to_string())?;
    let mut group = vec![u8::MAX; sources.attribute_count()];
    let mut any = false;
    for (index, position) in group.iter_mut().enumerate() {
        let key = sources
            .group_key(index as u32)
            .map_err(|error| error.to_string())?;
        if let Ok(at) = sorted.binary_search_by(|&(k, _)| k.cmp(&key)) {
            *position = sorted[at].1;
            any = true;
        }
    }
    if !any {
        return Ok(Vec::new());
    }
    let mut pieces = Vec::new();
    for index in 0..sources.piece_count() {
        let piece = sources.piece(index).map_err(|error| error.to_string())?;
        let position = group[piece.attribute as usize];
        if position != u8::MAX {
            pieces.push((position, piece.ends.map(|local| tile.global(local))));
        }
    }
    Ok(pieces)
}

/// Horizontal distance from the origin to the segment `a`-`b`.
fn distance_to_segment(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        (-(a[0] * dx + a[1] * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + t * dx).hypot(a[1] + t * dy)
}

/// Joins line pieces at shared ends (bit-identical on the global lattice, across tiles too) into
/// chains that stop at dead ends and junctions; closed loops come out as one chain each.
fn join(pieces: &[[GlobalSteps; 2]]) -> Vec<Vec<GlobalSteps>> {
    let mut at: HashMap<GlobalSteps, Vec<usize>> = HashMap::new();
    for (index, ends) in pieces.iter().enumerate() {
        for end in ends {
            at.entry(*end).or_default().push(index);
        }
    }
    let mut used = vec![false; pieces.len()];
    let other = |index: usize, end: GlobalSteps| {
        let [a, b] = pieces[index];
        if a == end { b } else { a }
    };
    let walk = |start: GlobalSteps, first: usize, used: &mut Vec<bool>| {
        let mut chain = vec![start];
        let (mut node, mut piece) = (start, first);
        loop {
            used[piece] = true;
            node = other(piece, node);
            chain.push(node);
            let next = &at[&node];
            if next.len() != 2 {
                break;
            }
            match next.iter().find(|&&index| !used[index]) {
                Some(&index) => piece = index,
                None => break,
            }
        }
        chain
    };
    let mut chains = Vec::new();
    // Ends and junctions first, in a fixed order so a click answers the same every time.
    let mut nodes: Vec<&GlobalSteps> = at
        .iter()
        .filter(|(_, incident)| incident.len() != 2)
        .map(|(node, _)| node)
        .collect();
    nodes.sort_unstable();
    for &node in nodes {
        for &index in &at[&node] {
            if !used[index] {
                chains.push(walk(node, index, &mut used));
            }
        }
    }
    for index in 0..pieces.len() {
        if !used[index] {
            chains.push(walk(pieces[index][0], index, &mut used));
        }
    }
    chains
}

/// Every group's chains simplified to [`TOLERANCE_M`], all coarser by halves until together they
/// hold at most `most` points (a chain keeps its two ends at least).
pub(crate) fn simplified(groups: &[&[Vec<[f64; 2]>]], most: usize) -> Vec<Lines> {
    let mut tolerance = TOLERANCE_M;
    loop {
        let lines: Vec<Lines> = groups
            .par_iter()
            .map(|chains| {
                chains
                    .iter()
                    .map(|chain| douglas_peucker(chain, tolerance))
                    .collect()
            })
            .collect();
        if lines.iter().flatten().map(Vec::len).sum::<usize>() <= most || tolerance > 1e5 {
            return lines;
        }
        tolerance *= 2.0;
    }
}

/// The points of `line` within `tolerance` metres of the simplified line (iterative
/// Douglas-Peucker).
fn douglas_peucker(line: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    if line.len() <= 2 {
        return line.to_vec();
    }
    let mut keep = vec![false; line.len()];
    keep[0] = true;
    keep[line.len() - 1] = true;
    let mut stack = vec![(0, line.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let (a, b) = (line[first], line[last]);
        let mut farthest = (0.0, first);
        for (index, point) in line.iter().enumerate().take(last).skip(first + 1) {
            let off = distance_to_segment(
                [a[0] - point[0], a[1] - point[1]],
                [b[0] - point[0], b[1] - point[1]],
            );
            if off > farthest.0 {
                farthest = (off, index);
            }
        }
        if farthest.0 > tolerance {
            keep[farthest.1] = true;
            stack.push((first, farthest.1));
            stack.push((farthest.1, last));
        }
    }
    line.iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i64, y: i64) -> GlobalSteps {
        GlobalSteps { x, y }
    }

    #[test]
    fn pieces_join_into_chains_that_stop_at_junctions() {
        // A road of three pieces given out of order, a branch at its middle node, a loop apart.
        let pieces = [
            [at(2, 0), at(3, 0)],
            [at(0, 0), at(1, 0)],
            [at(2, 0), at(1, 0)],
            [at(2, 0), at(2, 5)],
            [at(10, 10), at(11, 10)],
            [at(11, 10), at(10, 11)],
            [at(10, 11), at(10, 10)],
        ];
        let chains = join(&pieces);
        assert_eq!(chains.len(), 4);
        assert!(chains.contains(&vec![at(0, 0), at(1, 0), at(2, 0)]));
        assert!(
            chains.contains(&vec![at(2, 0), at(3, 0)])
                || chains.contains(&vec![at(3, 0), at(2, 0)])
        );
        assert!(
            chains.contains(&vec![at(2, 0), at(2, 5)])
                || chains.contains(&vec![at(2, 5), at(2, 0)])
        );
        let ring = chains.iter().find(|chain| chain.len() == 4).unwrap();
        assert_eq!(ring.first(), ring.last());
        assert_eq!(
            chains.iter().map(|chain| chain.len() - 1).sum::<usize>(),
            pieces.len()
        );
    }

    #[test]
    fn simplification_keeps_bends_and_drops_straight_points() {
        let line: Vec<[f64; 2]> = (0..=100)
            .map(|k| [f64::from(k), if k <= 50 { 0.0 } else { f64::from(k - 50) }])
            .collect();
        assert_eq!(
            douglas_peucker(&line, TOLERANCE_M),
            vec![[0.0, 0.0], [50.0, 0.0], [100.0, 50.0]]
        );
        let wiggly: Vec<[f64; 2]> = (0..=1_000)
            .map(|k| [f64::from(k), 30.0 * (f64::from(k) / 7.0).sin()])
            .collect();
        // Two contributors share the points: one tolerance for both, so neither keeps its own 100.
        let one = [wiggly.clone()];
        let two = [wiggly.clone(), wiggly];
        let lines = simplified(&[&one[..], &two[..]], 100);
        assert!(lines.iter().flatten().map(Vec::len).sum::<usize>() <= 100);
        assert!(lines.iter().flatten().all(|line| line.len() >= 2));
        assert_eq!(lines[1][0], lines[0][0]);
    }
}
