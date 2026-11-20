//! A building on the map: its loudest façade, as a click inside it is answered (`popup::building`):
//! buildings crossing a square edge use the popup's complete façade ranking; otherwise the few whose
//! painted outdoor pixel in front is loudest are painted as points of their blocks, and the loudest
//! of them from the ground sources gives every pixel of the building its level.

use crate::exact::{LAYERS, Point};
use crate::levels::Painted;
use crate::paint::{PASS_BLOCKS, metres_at, pixel_of, runs_within, shortest_pixel_side_metres};
use crate::passes::PASSES_AT_ONCE;
use crate::points::PointPainter;
use crate::quiet::QuietBlocks;
use crate::square::Square;
use popup::obstacles::{BuildingParts, Footprint};
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
pub(crate) fn ground_energy(layers: &[f64; LAYERS]) -> f64 {
    (layers.iter().enumerate())
        .filter(|&(layer, _)| layer != Layer::Aircraft as usize)
        .map(|(_, energy)| energy)
        .sum()
}

/// Indoor pixels with one physical exterior, even when the input names several buildings together.
pub(crate) struct BuildingPixels {
    footprint: Footprint,
    pixels: Vec<usize>,
}

pub(crate) fn building_pixels(
    square: &Square,
    n: usize,
    indoor: Vec<(usize, u64)>,
) -> Vec<BuildingPixels> {
    let mut by_record = HashMap::<u64, Vec<usize>>::new();
    for (pixel, id) in indoor {
        by_record.entry(id).or_default().push(pixel);
    }
    let mut records: Vec<_> = by_record.into_iter().collect();
    records.sort_unstable_by_key(|(id, _)| *id);
    records
        .into_par_iter()
        .flat_map_iter(|(id, pixels)| {
            let parts = match square.obstacles.building_parts(id) {
                BuildingParts::One(footprint) => return vec![BuildingPixels { footprint, pixels }],
                BuildingParts::Locate(parts) => parts,
            };
            let mut by_part = vec![Vec::new(); parts.len()];
            for pixel in pixels {
                let point = metres_at(
                    square,
                    n,
                    (pixel % n) as f64 + 0.5,
                    (pixel / n) as f64 + 0.5,
                );
                if let Some(index) = square.obstacles.part_at_indoor_point(&parts, point) {
                    by_part[index].push(pixel);
                }
            }
            parts
                .into_iter()
                .zip(by_part)
                .filter(|(_, pixels)| !pixels.is_empty())
                .map(|(footprint, pixels)| BuildingPixels { footprint, pixels })
                .collect()
        })
        .collect()
}

/// Which of a pass's receivers some building may still read (`buildings[k]` receiver `k`'s
/// building, `ground[k]` its [`ground_energy`]): each building's loudest among them, the first of
/// equals, as [`loudest_by_ground`] keeps it over all the passes; if any energy in the pass is not
/// a number, every receiver is kept so the pass matches the unpruned reduction.
fn loudest_receivers(buildings: &[usize], ground: &[f64]) -> Vec<bool> {
    if ground.iter().any(|value| value.is_nan()) {
        return vec![true; buildings.len()];
    }
    let mut loudest: HashMap<usize, usize> = HashMap::new();
    for (k, &building) in buildings.iter().enumerate() {
        let kept = loudest.entry(building).or_insert(k);
        if ground[k] > ground[*kept] {
            *kept = k;
        }
    }
    (buildings.iter().enumerate())
        .map(|(k, building)| loudest[building] == k)
        .collect()
}

/// A façade receiver chosen to be painted: its building, its point, the pixel it lies in.
pub(crate) struct FacadeCandidate {
    building: usize,
    point: Point,
    pixel: usize,
    exact: bool,
}

/// Each building's façade receivers to paint (`indoor`: its pixels' places and footprints).
/// A split footprint ranks its complete façade with the popup; the others keep the [`FACADE_CANDIDATES`]
/// whose painted pixel half a pixel in front (outward) is loudest from the ground sources; a pixel
/// around would rank a receiver by a street it does not face (a south façade's corner by the road
/// east of it, 3.6 dB under the façade's loudest receiver). A building with none keeps no level.
pub(crate) fn facade_candidates(
    square: &Square,
    n: usize,
    pixels: &[Option<Painted>],
    buildings: &[BuildingPixels],
) -> Result<Vec<FacadeCandidate>, String> {
    // A pixel's shorter side (m): half of it in front of a receiver lies in the pixel it faces.
    let pixel_m = shortest_pixel_side_metres(square, n);
    let in_front = |position: [f64; 2], bearing_deg: f64| -> f64 {
        let (east, north) = bearing_deg.to_radians().sin_cos();
        let front = [
            position[0] + 0.5 * pixel_m * east,
            position[1] + 0.5 * pixel_m * north,
        ];
        let painted = pixel_of(square, n, front).and_then(|at| pixels[at].as_ref());
        painted.map_or(0.0, |(layers, _)| ground_energy(layers))
    };
    let chosen: Vec<Vec<FacadeCandidate>> = buildings
        .par_iter()
        .enumerate()
        .map(|(index, pixels)| -> Result<_, String> {
            let building = &pixels.footprint;
            let receivers = square.obstacles.facade_receivers(building)?;
            if receivers.is_empty() {
                return Ok(Vec::new());
            }
            if building
                .rings
                .iter()
                .flat_map(|ring| &ring.points)
                .any(|&position| pixel_of(square, n, position).is_none())
            {
                let point = crate::boundary_facades::choose(square, building, &receivers)?;
                let pixel = crate::boundary_facades::background_pixel(square, n, point.position);
                return Ok(vec![FacadeCandidate {
                    building: index,
                    point,
                    pixel,
                    exact: true,
                }]);
            }
            let mut ranked: Vec<(f64, [f64; 2], usize)> = Vec::new();
            for receiver in receivers {
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
                        building: index,
                        point: Point::on_facade(square, position, building.id)?,
                        pixel,
                        exact: false,
                    })
                })
                .collect()
        })
        .collect::<Result<_, String>>()?;
    Ok(chosen.into_iter().flatten().collect())
}

/// Each building's level: its candidates painted as points of their blocks, each standing at a wall
/// with a run of their blocks at a time as a pass of pixels takes them, at least as many runs as
/// passes go at once; the loudest from the ground sources kept.
pub(crate) fn building_levels(
    painter: &PointPainter,
    quiet: &QuietBlocks,
    candidates: Vec<FacadeCandidate>,
) -> Result<HashMap<usize, Painted>, String> {
    let (boundary, mut candidates): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|candidate| candidate.exact);
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
        .map(|(at, range)| range.len() * quiet.most_source_counts[*at])
        .collect();
    let mut footprints = Vec::with_capacity(candidates.len());
    let mut pixels = Vec::with_capacity(candidates.len());
    let points: Vec<Point> = (candidates.into_iter())
        .map(|c| {
            footprints.push(c.building);
            pixels.push(c.pixel);
            c.point
        })
        .collect();
    let most = blocks.len().div_ceil(PASSES_AT_ONCE).min(PASS_BLOCKS);
    let painted = painter.in_passes(runs_within(&bounds, most), |run| {
        let pass: Vec<_> = blocks[run.clone()]
            .par_iter()
            .map_init(Vec::new, |near, (at, _)| quiet.block(painter, near, *at))
            .collect();
        let (first, last) = (&blocks[run.start].1, &blocks[run.end - 1].1);
        let points_run = first.start..last.end;
        let places: Vec<(usize, usize)> = (blocks[run].iter().enumerate())
            .flat_map(|(slot, (_, range))| range.clone().map(move |k| (slot, k)))
            .map(|(slot, k)| (slot, pixels[k]))
            .collect();
        // Only a building's loudest receiver of the pass can be the one its pixels read.
        let paint =
            painter.paint_chosen(&pass, &points[points_run.clone()], &places, &|ground| {
                loudest_receivers(&footprints[points_run.clone()], ground)
            })?;
        Ok((points_run, paint))
    })?;
    let painted = (painted.into_iter())
        .flat_map(|(_, (points_run, paint))| footprints[points_run].iter().copied().zip(paint));
    let mut levels = loudest_by_ground(painted);
    let (footprints, points, pixels) = boundary.into_iter().fold(
        (Vec::new(), Vec::new(), Vec::new()),
        |(mut footprints, mut points, mut pixels), candidate| {
            footprints.push(candidate.building);
            points.push(candidate.point);
            pixels.push(candidate.pixel);
            (footprints, points, pixels)
        },
    );
    let paint = crate::etalon::paint_points(
        painter.square,
        n,
        &points,
        &pixels,
        (
            &painter.corners.hum,
            &painter.corners.flight_spectra,
            &painter.corners.displays,
        ),
        painter.batch,
    )?;
    levels.extend(footprints.into_iter().zip(paint));
    Ok(levels)
}

/// Per building the paint of its loudest candidate from the ground sources (the flights apart),
/// equal energies to the first.
fn loudest_by_ground(
    painted: impl Iterator<Item = (usize, Option<Painted>)>,
) -> HashMap<usize, Painted> {
    let mut levels: HashMap<usize, Painted> = HashMap::new();
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

/// Pixels without an exposed receiver keep their missing level.
pub(crate) fn fill_buildings(
    pixels: &mut [Option<Painted>],
    buildings: Vec<BuildingPixels>,
    levels: &HashMap<usize, Painted>,
) {
    for (index, building) in buildings.into_iter().enumerate() {
        if let Some(level) = levels.get(&index) {
            for at in building.pixels {
                pixels[at] = Some(*level);
            }
        }
    }
}

#[cfg(test)]
#[path = "building_boundary_tests.rs"]
mod boundary_tests;

#[cfg(test)]
#[path = "buildings_tests.rs"]
mod tests;
