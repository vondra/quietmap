//! A building on the map: every pixel of it takes its loudest façade's paint, as a click inside it is
//! answered (`popup::building`). The façade is chosen by the popup's rule ([`crate::facade_choice`])
//! and painted with its own flights ([`crate::facade_flights`]): as a point of the block it stands
//! in, its ranked sources exact, or, for a building crossing the square's edge, with every loud
//! source exact and every aircraft box summed, so that the square beside answers it alike.

use crate::etalon::paint_points;
use crate::exact::Point;
use crate::facade_choice::{buildings, choose, facade_point};
use crate::facade_flights::flights_at_facades;
use crate::flight_boxes::flights_at_points;
use crate::flights::FlightSums;
use crate::levels::Painted;
use crate::paint::{PASS_BLOCKS, runs_within};
use crate::passes::PASSES_AT_ONCE;
use crate::points::PointPainter;
use crate::quiet::QuietBlocks;
use crate::square::Square;
use rayon::prelude::*;
use std::collections::HashMap;

/// Each building's level by footprint, from its `indoor` pixels (their places and footprints); a
/// building without an exposed façade has none.
pub(crate) fn building_levels(
    painter: &PointPainter,
    quiet: &QuietBlocks,
    indoor: &[(usize, u64)],
) -> Result<HashMap<u64, Painted>, String> {
    let (square, n) = (painter.square, painter.grid.pixels);
    let buildings = buildings(square, n, indoor)?;
    if buildings.is_empty() {
        return Ok(HashMap::new());
    }
    let chosen = choose(painter, &buildings)?;
    let points: Vec<Point> = (buildings.par_iter().zip(&chosen))
        .map(|(building, &facade)| facade_point(square, &building.facades[facade]))
        .collect::<Result<_, String>>()?;
    let pixels: Vec<usize> = (points.iter())
        .map(|point| background_pixel(square, n, point.position))
        .collect();
    let (edge, inner): (Vec<usize>, Vec<usize>) =
        (0..buildings.len()).partition(|&k| buildings[k].crosses_edge);
    let take = |members: &[usize]| -> (Vec<Point>, Vec<usize>) {
        (
            members.iter().map(|&k| points[k].clone()).collect(),
            members.iter().map(|&k| pixels[k]).collect(),
        )
    };
    let ((edge_points, edge_pixels), (inner_points, inner_pixels)) = (take(&edge), take(&inner));
    let boxes = &painter.corners.flight_boxes;
    // A building across the square's edge sums every box, as the square beside it does.
    let edge_flights = || flights_at_points(square, boxes, &edge_points, painter.batch);
    let inner_flights = flights_at_facades(square, boxes, &inner_points, painter.batch)?;
    let corners = &painter.corners;
    let edge_paint = paint_points(
        square,
        n,
        (&edge_points, edge_flights),
        &edge_pixels,
        (&corners.hum, &corners.flight_spectra, &corners.displays),
        painter.batch,
    )?;
    // Of the ranked sources those loud at the chosen façade, as a pixel's exact sources are.
    let heard: Vec<Vec<u32>> = (inner.iter())
        .map(|&k| {
            let point = &points[k];
            (buildings[k].sources.iter().copied())
                .filter(|&index| square.loud(index, point.position, &point.bound))
                .collect()
        })
        .collect();
    let inner_paint = paint_in_blocks(
        painter,
        quiet,
        (inner_points, inner_flights, inner_pixels),
        &heard,
    )?;
    let mut levels: HashMap<u64, Painted> = HashMap::new();
    for (members, paint) in [(&edge, edge_paint), (&inner, inner_paint)] {
        for (&k, paint) in members.iter().zip(paint) {
            levels.insert(buildings[k].footprint, paint);
        }
    }
    Ok(levels)
}

/// Façade points painted as points of the blocks their pixels lie in, each with its own flights and
/// its own admission there, its building's ranked sources loud there (`heard`) exact beside the
/// block's own
/// ([`PointPainter::facade_block`]): a façade hears what its block's corners, behind its building or
/// across it, may not. A run of them at a time, as a pass of pixels goes, at least as many runs as
/// passes go at once.
fn paint_in_blocks(
    painter: &PointPainter,
    quiet: &QuietBlocks,
    (points, flights, pixels): (Vec<Point>, Vec<FlightSums>, Vec<usize>),
    heard: &[Vec<u32>],
) -> Result<Vec<Painted>, String> {
    let (n, side) = (painter.grid.pixels, painter.grid.block);
    let block_of = |pixel: usize| (pixel / n / side) * (n / side) + (pixel % n) / side;
    let bounds: Vec<usize> = (pixels.iter().zip(heard))
        .map(|(&pixel, heard)| quiet.exact_source_counts[block_of(pixel)] + heard.len())
        .collect();
    let most = points.len().div_ceil(PASSES_AT_ONCE).min(PASS_BLOCKS);
    let painted = painter.in_passes(runs_within(&bounds, most), |run| {
        let blocks: Vec<_> = (run.clone().into_par_iter())
            .map_init(Vec::new, |near, k| {
                painter.facade_block(near, block_of(pixels[k]), &heard[k])
            })
            .collect();
        let places: Vec<(usize, usize)> = (run.clone().enumerate())
            .map(|(slot, k)| (slot, pixels[k]))
            .collect();
        let flights = &flights[run.clone()];
        painter.paint(&blocks, &points[run], &places, Some(flights))
    })?;
    (painted.into_iter().flat_map(|(_, run)| run))
        .map(|paint| paint.ok_or_else(|| "a façade point has no paint".to_string()))
        .collect()
}

/// The pixel whose background a façade at `position` takes, clamped into the square: a façade
/// outside it keeps its own rays and flights.
fn background_pixel(square: &Square, n: usize, position: [f64; 2]) -> usize {
    let pixel = [
        (0.5 + position[0] / square.frame.east_m_per_unit) * n as f64,
        (0.5 - position[1] / square.frame.north_m_per_unit) * n as f64,
    ]
    .map(|value| value.floor().clamp(0.0, (n - 1) as f64) as usize);
    pixel[1] * n + pixel[0]
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
#[path = "building_boundary_tests.rs"]
mod boundary_tests;
