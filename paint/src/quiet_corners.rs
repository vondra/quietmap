//! Shared background corner loudness for quiet-block interpolation, using the painter's existing sound model.
//!
//! Shared background corners need this calculation once; pixels rejected by the rectangle proof
//! or a patch probe retain their ordinary receiver sound.

use crate::exact::LAYERS;
use crate::points::PointPainter;
use crate::sone::Rest;
use popup::percentiles::Resolution;
use rayon::prelude::*;

pub(crate) fn loudness_at_corners(
    painter: &PointPainter,
    needed: Vec<bool>,
) -> Result<Vec<[f64; LAYERS + 1]>, String> {
    needed
        .into_par_iter()
        .enumerate()
        .map(|(at, needed)| {
            if !needed {
                return Ok([0.0; LAYERS + 1]);
            }
            loudness_at_corner(painter, at)
        })
        .collect()
}

pub(crate) fn loudness_at_corner(
    painter: &PointPainter,
    at: usize,
) -> Result<[f64; LAYERS + 1], String> {
    let grid = painter.grid;
    let side = grid.pixels / grid.block + 1;
    let corners = &painter.corners;
    let (x, y) = ((at % side) * grid.block, (at / side) * grid.block);
    let (steady, bands) = corners.hum.at(x, y);
    let rest = Rest {
        steady,
        bands,
        left_groups: (&[], [0.0; 4]),
        left_reach: 1.0,
        flights: corners.flights[at],
    };
    crate::sone::pixel_ndens(
        painter.square,
        &corners.displays,
        (corners.corner_points[at].position, 1.0),
        (&[], &[], &crate::exact::PointBands::default()),
        (&rest, &corners.flight_spectra.at(x, y)),
        Resolution::MAP,
    )
}
