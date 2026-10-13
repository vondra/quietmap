//! A square painted as dev1 painted it, in blocks of 16 x 16 pixels. At every block corner each
//! ground source loud there (`popup::candidates::loud`) is evaluated exactly. A block's pixels
//! evaluate exactly the loud sources crossing it or the eight blocks around and, layer by layer,
//! the loudest at its corners until what is left out weighs at most a share of its quietest
//! corner's ([`crate::blocks`]); what is left out is blended from the corners, the quiet hum from
//! its points ([`Hum`]), the flights from the corners. Each pixel's loudness (Nden, sone) per layer
//! and in total is the popup's model on those ([`crate::sone`]); a block hearing only the hum and
//! the flights (no source of its own, no wall) blends its corners' ([`crate::points`]). A
//! building's pixels take its loudest façade, painted at its likeliest loudest façade receivers as a
//! click inside it is answered ([`crate::buildings`]).

use crate::blocks::{Block, Corner};
use crate::buildings::{building_levels, facade_candidates, fill_buildings};
use crate::exact::{Batch, LAYERS, Point, PointBands, evaluate_all};
use crate::flights::{FlightSpectra, FlightSums, flights_at_corners};
use crate::hum::Hum;
use crate::points::PointPainter;
use crate::sone::Displays;
use crate::square::Square;
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::AtomicUsize;
use tiles::geo::Mercator;

/// A cell not assessed: a building with no exposed façade in the square, or nothing painted there.
pub const NO_LEVEL: u8 = 255;
/// A cell assessed and silent: under 0 phon, 1/16 sone (the quiet zones count it quiet, a zoom out
/// averages it as no loudness).
pub const SILENCE: u8 = 254;

/// How a square is cut: pixels per side, per block and between the hum's points.
#[derive(Clone, Copy)]
pub struct Grid {
    pub pixels: usize,
    pub block: usize,
    pub hum: usize,
}

impl Grid {
    /// The grid of the map's zoom (12 or 13): blocks of about 100 m and the hum's points about
    /// 400 m apart (790 m put a quiet Dobris pixel's total 1.5 phon over the popup's; at 400 m its
    /// cells over 1 phon went 0.75 -> 0 %; 200 m added five times as much time).
    pub fn at_zoom(zoom: u32) -> Option<Self> {
        match zoom {
            12 => Some(Grid {
                pixels: 512,
                block: 8,
                hum: 32,
            }),
            13 => Some(Grid {
                pixels: 1024,
                block: 16,
                hum: 64,
            }),
            _ => None,
        }
    }
}

/// Blocks one pass takes at most (4 rows of 64 at z13, 65,536 pixels).
pub(crate) const PASS_BLOCKS: usize = 256;

/// Pairs a pass may evaluate, as its points' blocks' exact sources bound them: a pass holds about
/// 180 bytes a bounded pair (four passes of four block rows of a Prague square, 35-55 M bounded
/// pairs each, held 47 GB, 9 GB of it the square and its corners), and Prague's centre ran out of
/// memory; Dobříš's passes stay whole (11.8 M bounded pairs at most).
pub(crate) const PASS_PAIRS: usize = 24_000_000;

/// Consecutive runs of items, `bounds[k]` item k's pairs at most: each of at most `most` items whose
/// bounds sum to at most [`PASS_PAIRS`], or a single item.
pub(crate) fn runs_within(bounds: &[usize], most: usize) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let (mut start, mut sum) = (0, 0);
    for (at, &bound) in bounds.iter().enumerate() {
        if at > start && (at - start == most || sum + bound > PASS_PAIRS) {
            runs.push(start..at);
            (start, sum) = (at, 0);
        }
        sum += bound;
    }
    if start < bounds.len() {
        runs.push(start..bounds.len());
    }
    runs
}

/// Per pixel (row-major from the north-west) its cell per layer and the total's last ([`cell`]),
/// [`SILENCE`] or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

/// An outdoor point painted: per layer its Lden energy (a building's façade is chosen by it) and its
/// loudness per layer and in total (sone).
pub type Painted = ([f64; LAYERS], [f64; LAYERS + 1]);

/// A pass's pixels: the outdoor ones' place in the square and paint, and the indoor ones' place and
/// building.
type PassPixels = (Vec<(usize, Painted)>, Vec<(usize, u64)>);

/// The cell of a loudness (sone): twice its loudness level in phon (40 + 10 log2 sone), so a step
/// is half a phon (3.5 % of loudness) and a doubling 20 steps; [`SILENCE`] under 0 phon.
pub fn cell(sone: f64) -> u8 {
    let byte = 80.0 + 20.0 * sone.log2();
    if byte.is_nan() || byte < 0.0 {
        SILENCE
    } else {
        byte.round().min(f64::from(SILENCE - 1)) as u8
    }
}

/// The cells of a pixel's loudness per layer and in total.
pub fn cells_of(ndens: &[f64; LAYERS + 1]) -> [u8; LAYERS + 1] {
    ndens.map(cell)
}

/// The weights of four corners (north-west, north-east, south-west, south-east) at `(fx, fy)`.
pub(crate) fn corner_weights(fx: f64, fy: f64) -> [f64; 4] {
    [
        (1.0 - fx) * (1.0 - fy),
        fx * (1.0 - fy),
        (1.0 - fx) * fy,
        fx * fy,
    ]
}

/// Bilinear between four corners (north-west, north-east, south-west, south-east).
pub(crate) fn bilinear(corners: [f64; 4], fx: f64, fy: f64) -> f64 {
    let north = corners[0] * (1.0 - fx) + corners[1] * fx;
    let south = corners[2] * (1.0 - fx) + corners[3] * fx;
    north * (1.0 - fy) + south * fy
}

/// What a square's corners know before its pixels are painted: owned, so the next square's
/// corners can be taken on the cores while this one's pixels are on the card.
pub struct Corners {
    pub(crate) corner_points: Vec<Point>,
    pub(crate) flights: Vec<FlightSums>,
    pub(crate) flight_spectra: FlightSpectra,
    pub(crate) hum: Hum,
    pub(crate) displays: Displays,
    pub(crate) corners: Vec<Corner>,
    pub(crate) corner_bands: Vec<PointBands>,
    corner_pairs: usize,
    seconds: f64,
}

/// Paints the square: every pixel's cells, the exact evaluations here or by `batch`.
pub fn paint(square: &Square, grid: Grid, batch: Option<&dyn Batch>) -> Result<Cells, String> {
    match corners_of(square, grid, batch)? {
        Some(corners) => paint_pixels(square, grid, batch, corners),
        None => Ok(vec![[SILENCE; LAYERS + 1]; grid.pixels * grid.pixels]),
    }
}

/// The square's point at pixel coordinates (x, y), in the square's frame.
pub(crate) fn metres_at(square: &Square, n: usize, x: f64, y: f64) -> [f64; 2] {
    square.frame.to_metres(Mercator {
        x: f64::from(square.tile.x) + x / n as f64,
        y: f64::from(square.tile.y) + y / n as f64,
    })
}

/// The pixel (row-major) holding `position` (frame metres), `None` outside the square.
pub(crate) fn pixel_of(square: &Square, n: usize, position: [f64; 2]) -> Option<usize> {
    let mercator = square.frame.to_mercator(position);
    let x = (mercator.x - f64::from(square.tile.x)) * n as f64;
    let y = (mercator.y - f64::from(square.tile.y)) * n as f64;
    let inside = |v: f64| (0.0..n as f64).contains(&v);
    (inside(x) && inside(y)).then(|| y as usize * n + x as usize)
}

/// The frame's rectangle over pixel coordinates (x0, y0) to (x1, y1).
pub(crate) fn rectangle_at(
    square: &Square,
    n: usize,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> ([f64; 2], [f64; 2]) {
    let (a, b) = (metres_at(square, n, x0, y0), metres_at(square, n, x1, y1));
    (
        [a[0].min(b[0]), a[1].min(b[1])],
        [a[0].max(b[0]), a[1].max(b[1])],
    )
}

/// The square's corners: each one's sources loud there evaluated exactly and ranked, its flights,
/// the hum, the flights' spectra; `None` for a square that is silent everywhere.
pub fn corners_of(
    square: &Square,
    grid: Grid,
    batch: Option<&dyn Batch>,
) -> Result<Option<Corners>, String> {
    let n = grid.pixels;
    let metres = |x: f64, y: f64| metres_at(square, n, x, y);
    let rectangle = |x0: f64, y0: f64, x1: f64, y1: f64| rectangle_at(square, n, x0, y0, x1, y1);
    let started = std::time::Instant::now();
    let blocks = n / grid.block;
    let corner_side = blocks + 1;
    let corner_points: Vec<Point> = (0..corner_side * corner_side)
        .into_par_iter()
        .map(|at| {
            let (i, j) = (at % corner_side, at / corner_side);
            Point::at(
                square,
                metres((i * grid.block) as f64, (j * grid.block) as f64),
            )
        })
        .collect::<Result<_, String>>()?;
    // The flights are the cores' work alone: they run beside the hum and the corners on the card.
    let ((flights, flight_spectra), hum) = rayon::join(
        || {
            rayon::join(
                || flights_at_corners(square, grid, &metres, &corner_points),
                || FlightSpectra::new(square, n, 2 * grid.block),
            )
        },
        || Hum::new(square, n, grid.hum, batch),
    );
    let (flights, flight_spectra, hum) = (flights?, flight_spectra?, hum?);
    // A square no loud source reaches, no flight crosses and whose hum stays under 0 dB (the
    // reflection adding at most 3 dB) has no level: most of the world.
    let mut near = Vec::new();
    let (low, high) = rectangle(0.0, 0.0, n as f64, n as f64);
    square.index.reaching(low, high, &mut near);
    let no_flights = (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none());
    if near.is_empty() && no_flights && hum.silent() {
        return Ok(None);
    }
    let displays = Displays::of(square)?;
    // Every block corner evaluates every source loud there.
    let pairs: Vec<(u32, u32)> = (0..corner_points.len())
        .into_par_iter()
        .map_init(Vec::new, |near, k| {
            let point = &corner_points[k];
            square.index.reaching(point.position, point.position, near);
            (near.iter())
                .filter(|&&index| square.loud(index, point.position, &point.bound))
                .map(|&index| (k as u32, index))
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect();
    let corner_pairs = pairs.len();
    let mut corners: Vec<Corner> = (0..corner_points.len())
        .map(|_| Corner::default())
        .collect();
    let evaluated = evaluate_all(square, &corner_points, &pairs, batch)?;
    for (&(k, index), energy) in pairs.iter().zip(&evaluated.pairs) {
        let candidate = &square.candidates[index as usize];
        let distance_m = candidate.distance_from(corner_points[k as usize].position);
        corners[k as usize].add(
            candidate.layer as usize,
            (index, candidate.group_key),
            (*energy, distance_m),
        );
    }
    // A corner's bands: the shape of what its blocks leave out there.
    let corner_bands = evaluated.bands;
    corners.par_iter_mut().for_each(|corner| {
        for ranked in &mut corner.ranked {
            ranked.sort_unstable_by(|a, b| b.lden.total_cmp(&a.lden).then(a.index.cmp(&b.index)));
        }
    });
    Ok(Some(Corners {
        corner_points,
        flights,
        flight_spectra,
        hum,
        displays,
        corners,
        corner_bands,
        corner_pairs,
        seconds: started.elapsed().as_secs_f64(),
    }))
}

/// Paints the square's pixels from its corners (`corners_of`).
pub fn paint_pixels(
    square: &Square,
    grid: Grid,
    batch: Option<&dyn Batch>,
    corners: Corners,
) -> Result<Cells, String> {
    let n = grid.pixels;
    let metres = |x: f64, y: f64| metres_at(square, n, x, y);
    let started = std::time::Instant::now();
    let (corners_s, corner_pairs) = (corners.seconds, corners.corner_pairs);
    let painter = PointPainter {
        square,
        grid,
        batch,
        corners,
        pairs: AtomicUsize::new(0),
    };
    let blocks = n / grid.block;
    let area = grid.block * grid.block;
    // Every pixel, a run of blocks at a time (four block rows, fewer where their pairs would pass
    // PASS_PAIRS): the outdoor ones painted, the indoor ones' place and building kept for their
    // building's façade.
    let pass_of = |run: Range<usize>| -> Result<PassPixels, String> {
        let block_ids: Vec<usize> = run.collect();
        let pass: Vec<Block> = (block_ids.par_iter())
            .map_init(Vec::new, |near, &at| painter.block(near, at))
            .collect();
        // The pass's pixels block by block: a pixel's block (its slot in the pass) and its place.
        let places: Vec<(usize, usize)> = (0..block_ids.len() * area)
            .map(|k| {
                let (slot, inside) = (k / area, k % area);
                let at = block_ids[slot];
                let x = (at % blocks) * grid.block + inside % grid.block;
                let y = (at / blocks) * grid.block + inside / grid.block;
                (slot, y * n + x)
            })
            .collect();
        let points: Vec<Point> = (places.par_iter())
            .map(|&(_, at)| Point::at(square, metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5)))
            .collect::<Result<_, String>>()?;
        let walls: Vec<bool> = (0..block_ids.len())
            .map(|slot| {
                (slot * area..(slot + 1) * area)
                    .any(|k| points[k].own_footprint != 0 || points[k].reflection_db != 0.0)
            })
            .collect();
        let painted = painter.paint(&block_ids, &pass, &points, &places, &walls)?;
        let indoor = (points.iter().zip(&places))
            .filter(|(point, _)| point.own_footprint != 0)
            .map(|(point, &(_, at))| (at, point.own_footprint))
            .collect();
        let outdoor = (painted.into_iter().zip(&places))
            .filter_map(|(paint, &(_, at))| paint.map(|paint| (at, paint)))
            .collect();
        Ok((outdoor, indoor))
    };
    // Each block's pairs at most, its exact sources at every pixel: the block computed a row at a
    // time and dropped (a dense square's blocks all at once hold gigabytes), again in its pass.
    let bounds: Vec<usize> = (0..blocks)
        .flat_map(|row| {
            (row * blocks..(row + 1) * blocks)
                .into_par_iter()
                .map_init(Vec::new, |near, at| {
                    painter.block(near, at).exact.len() * area
                })
                .collect::<Vec<usize>>()
        })
        .collect();
    let painted = painter.in_passes(runs_within(&bounds, PASS_BLOCKS), pass_of)?;
    let mut pixels: Vec<Option<Painted>> = vec![None; n * n];
    let mut indoor = Vec::new();
    for (_, (outdoor, inside)) in painted {
        indoor.extend(inside);
        for (at, paint) in outdoor {
            pixels[at] = Some(paint);
        }
    }
    let candidates = facade_candidates(square, n, &pixels, &indoor)?;
    let levels = building_levels(&painter, &bounds, candidates)?;
    fill_buildings(&mut pixels, &indoor, &levels);
    let cells = (pixels.par_iter())
        .map(|pixel| {
            pixel
                .as_ref()
                .map_or([NO_LEVEL; LAYERS + 1], |(_, ndens)| cells_of(ndens))
        })
        .collect();
    eprintln!(
        "qm-paint: hum and corners {corners_s:.1} s ({corner_pairs} pairs), pixels {:.1} s ({} pairs)",
        started.elapsed().as_secs_f64(),
        painter.pairs.into_inner(),
    );
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs keep the items' order and cut at `most` items or before the pairs pass [`PASS_PAIRS`];
    /// an item over it stands alone.
    #[test]
    fn runs_cut_at_their_count_and_their_pairs() {
        let half = PASS_PAIRS / 2;
        assert_eq!(runs_within(&[1, 1, 1, 1, 1], 2), vec![0..2, 2..4, 4..5]);
        assert_eq!(runs_within(&[half, half, 1, half], 10), vec![0..2, 2..4]);
        assert_eq!(
            runs_within(&[1, 3 * PASS_PAIRS, 1], 10),
            vec![0..1, 1..2, 2..3]
        );
        assert!(runs_within(&[], 4).is_empty());
    }
}
