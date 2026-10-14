//! A building on the map: its loudest façade, as a click inside it is answered (`popup::building`):
//! of its exposed façade receivers in the square (CNOSSOS-EU 2.8, `popup::obstacles`), the few whose
//! painted outdoor pixel in front is loudest are painted as points of their blocks, and the loudest
//! of them from the ground sources gives every pixel of the building its level.

use crate::blocks::Block;
use crate::exact::{LAYERS, Point};
use crate::paint::{PASS_BLOCKS, Painted, metres_at, pixel_of, runs_within};
use crate::passes::PASSES_AT_ONCE;
use crate::points::PointPainter;
use crate::square::Square;
use rayon::prelude::*;
use std::collections::HashMap;
use std::ops::Range;
use tiles::sources::Layer;

/// Façade receivers painted for each building: those whose painted outdoor pixel in front is
/// loudest. Over the 201 clicks inside buildings of a Prague square three put 8.0 % of them over 1
/// phon from the popup for 6 % more pairs, every receiver 4.0 % for 23 % more, the loudest in front
/// per way faced 14.9 % (its neighbour pixel 32.8 %).
const FACADE_CANDIDATES: usize = 3;

/// The ground layers' energy of a point: what the popup ranks façades by (the flights apart).
fn ground_energy(layers: &[f64; LAYERS]) -> f64 {
    (layers.iter().enumerate())
        .filter(|&(layer, _)| layer != Layer::Aircraft as usize)
        .map(|(_, energy)| energy)
        .sum()
}

/// A façade receiver chosen to be painted: its building, its point, the pixel it lies in.
pub(crate) struct FacadeCandidate {
    footprint: u64,
    point: Point,
    pixel: usize,
}

/// Each building's façade receivers to paint (`indoor`: its pixels' places and footprints): of its
/// exposed receivers inside the square, in the popup's canonical order, the [`FACADE_CANDIDATES`]
/// whose painted pixel half a pixel in front (outward) is loudest from the ground sources; a pixel
/// around would rank a receiver by a street it does not face (a south façade's corner by the road
/// east of it, 3.6 dB under the façade's loudest receiver). A building with none keeps no level.
pub(crate) fn facade_candidates(
    square: &Square,
    n: usize,
    pixels: &[Option<Painted>],
    indoor: &[(usize, u64)],
) -> Result<Vec<FacadeCandidate>, String> {
    let mut first: HashMap<u64, usize> = HashMap::new();
    for &(at, footprint) in indoor {
        first.entry(footprint).or_insert(at);
    }
    let mut buildings: Vec<(u64, usize)> = first.into_iter().collect();
    buildings.sort_unstable();
    // A pixel's shorter side (m): half of it in front of a receiver lies in the pixel it faces.
    let pixel_m = {
        let [a, b] = [0.0, 1.0].map(|at| metres_at(square, n, at, at));
        (b[0] - a[0]).abs().min((b[1] - a[1]).abs())
    };
    let in_front = |position: [f64; 2], bearing_deg: f64| -> f64 {
        let (east, north) = bearing_deg.to_radians().sin_cos();
        let front = [
            position[0] + 0.5 * pixel_m * east,
            position[1] + 0.5 * pixel_m * north,
        ];
        let painted = pixel_of(square, n, front).and_then(|at| pixels[at].as_ref());
        painted.map_or(0.0, |(layers, _)| ground_energy(layers))
    };
    let chosen: Vec<Vec<FacadeCandidate>> = (buildings.par_iter())
        .map(|&(footprint, at)| -> Result<_, String> {
            let inside = metres_at(square, n, (at % n) as f64 + 0.5, (at / n) as f64 + 0.5);
            let Some(building) = square.obstacles.enclosing_building(inside)? else {
                return Ok(Vec::new());
            };
            let mut ranked: Vec<(f64, [f64; 2], usize)> = Vec::new();
            for receiver in square.obstacles.facade_receivers(&building)? {
                if let Some(pixel) = pixel_of(square, n, receiver.position) {
                    let estimate = in_front(receiver.position, receiver.outward_bearing_deg);
                    ranked.push((estimate, receiver.position, pixel));
                }
            }
            // A stable sort: equal estimates keep the canonical order.
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
            ranked.truncate(FACADE_CANDIDATES);
            (ranked.into_iter())
                .map(|(_, position, pixel)| {
                    Ok(FacadeCandidate {
                        footprint,
                        point: Point::on_facade(square, position, footprint)?,
                        pixel,
                    })
                })
                .collect()
        })
        .collect::<Result<_, String>>()?;
    Ok(chosen.into_iter().flatten().collect())
}

/// Each building's level: its candidates painted as points of their blocks, each standing at a wall
/// (its block never blends its corners' loudness), a run of their blocks at a time as a pass of
/// pixels takes them (`block_pairs`: each block's pairs at most at its every pixel), at least as
/// many runs as passes go at once; the loudest from the ground sources kept.
pub(crate) fn building_levels(
    painter: &PointPainter,
    block_pairs: &[usize],
    mut candidates: Vec<FacadeCandidate>,
) -> Result<HashMap<u64, Painted>, String> {
    let (n, side) = (painter.grid.pixels, painter.grid.block);
    let block_of = |pixel: usize| (pixel / n / side) * (n / side) + (pixel % n) / side;
    candidates.sort_by_key(|c| block_of(c.pixel));
    // The candidates' blocks in order, each with its candidates' range and their pairs at most.
    let mut blocks: Vec<(usize, Range<usize>)> = Vec::new();
    for (k, candidate) in candidates.iter().enumerate() {
        let at = block_of(candidate.pixel);
        match blocks.last_mut() {
            Some((last, range)) if *last == at => range.end = k + 1,
            _ => blocks.push((at, k..k + 1)),
        }
    }
    let bounds: Vec<usize> = (blocks.iter())
        .map(|(at, range)| range.len() * block_pairs[*at] / (side * side))
        .collect();
    let mut footprints = Vec::with_capacity(candidates.len());
    let mut pixels = Vec::with_capacity(candidates.len());
    let points: Vec<Point> = (candidates.into_iter())
        .map(|c| {
            footprints.push(c.footprint);
            pixels.push(c.pixel);
            c.point
        })
        .collect();
    let most = blocks.len().div_ceil(PASSES_AT_ONCE).min(PASS_BLOCKS);
    let painted = painter.in_passes(runs_within(&bounds, most), |run| {
        let block_ids: Vec<usize> = blocks[run.clone()].iter().map(|(at, _)| *at).collect();
        let pass: Vec<Block> = (block_ids.par_iter())
            .map_init(Vec::new, |near, &at| painter.block(near, at))
            .collect();
        let (first, last) = (&blocks[run.start].1, &blocks[run.end - 1].1);
        let points_run = first.start..last.end;
        let places: Vec<(usize, usize)> = (blocks[run].iter().enumerate())
            .flat_map(|(slot, (_, range))| range.clone().map(move |k| (slot, k)))
            .map(|(slot, k)| (slot, pixels[k]))
            .collect();
        let walls = vec![true; block_ids.len()];
        let paint = painter.paint(
            &block_ids,
            &pass,
            &points[points_run.clone()],
            &places,
            &walls,
        )?;
        Ok((points_run, paint))
    })?;
    let painted = (painted.into_iter())
        .flat_map(|(_, (points_run, paint))| footprints[points_run].iter().copied().zip(paint));
    Ok(loudest_by_ground(painted))
}

/// Per building the paint of its loudest candidate from the ground sources (the flights apart),
/// equal energies to the first.
fn loudest_by_ground(
    painted: impl Iterator<Item = (u64, Option<Painted>)>,
) -> HashMap<u64, Painted> {
    let mut levels: HashMap<u64, Painted> = HashMap::new();
    for (footprint, paint) in painted {
        let Some(paint) = paint else {
            continue;
        };
        let level = levels.entry(footprint).or_insert(paint);
        if ground_energy(&paint.0) > ground_energy(&level.0) {
            *level = paint;
        }
    }
    levels
}

/// Gives each building's pixels (`indoor`: their places and footprints) its level; a building
/// without one keeps no level.
pub(crate) fn fill_buildings(
    pixels: &mut [Option<Painted>],
    indoor: &[(usize, u64)],
    levels: &HashMap<u64, Painted>,
) {
    for &(at, footprint) in indoor {
        if let Some(level) = levels.get(&footprint) {
            pixels[at] = Some(*level);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A building takes its candidate loudest from the ground sources, not the one loudest with
    /// its flights, and every one of its pixels takes it; a building without a painted candidate
    /// keeps no level.
    #[test]
    fn a_building_reads_its_loudest_facade() {
        let aircraft = Layer::Aircraft as usize;
        let mut loud = [1000.0; LAYERS];
        loud[aircraft] = 1.0;
        let mut overflown = [10.0; LAYERS];
        overflown[aircraft] = 1e9;
        let levels = loudest_by_ground(
            [
                (7, Some((overflown, [1.0; LAYERS + 1]))),
                (7, Some((loud, [8.0; LAYERS + 1]))),
                (9, None),
            ]
            .into_iter(),
        );
        let mut pixels: Vec<Option<Painted>> = vec![None; 6];
        let indoor = [(1, 7), (2, 7), (4, 9)];
        fill_buildings(&mut pixels, &indoor, &levels);
        assert_eq!(pixels[1], Some((loud, [8.0; LAYERS + 1])));
        assert_eq!(pixels[2], pixels[1]);
        assert!(pixels[4].is_none() && pixels[0].is_none());
    }
}
