//! Square corners, block pixels and facade loudness from exact sources, sampled hum and aircraft fields.
//!
//! A square painted as dev1 painted it, in blocks of 16 x 16 pixels. At every block corner each
//! ground source loud there (`popup::candidates::loud`) is evaluated exactly. A block's pixels
//! evaluate exactly the loud sources crossing it or the eight blocks around and, layer by layer,
//! the loudest at its corners until what is left out weighs at most a share of its quietest
//! corner's ([`crate::blocks`]); what is left out is blended from the corners, the quiet hum from
//! its points ([`Hum`]), the flights from the corners. Each pixel's loudness (Nden, sone) per layer
//! and in total is the popup's model on those ([`crate::sone`]); a block hearing only the hum and
//! the flights (no source of its own, no wall) blends its corners' ([`crate::quiet`]). Quiet
//! blocks skip pixel receivers through [`crate::quiet`]; patches away from crossing sources and
//! screening edges interpolate after their loudness lattice and pixel probe agree within half a phon.
//! A building's pixels take its loudest façade, painted at its likeliest loudest façade receivers as a
//! click inside it is answered ([`crate::buildings`]).

use crate::blocks::Corner;
use crate::buildings::{building_levels, facade_candidates, fill_buildings};
use crate::exact::{Batch, LAYERS, Point, PointBands, evaluate_all, loud_pairs};
use crate::flight_spectra::FlightSpectra;
use crate::flights::{FlightSums, flights_at_corners};
use crate::hum::Hum;
use crate::levels::{Painted, PaintedCells};
use crate::points::PointPainter;
use crate::quiet::QuietBlocks;
use crate::sone::Displays;
use crate::square::Square;
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::AtomicUsize;
use tiles::geo::Mercator;

/// How a square is cut: pixels per side, per block and between the hum's points.
#[derive(Clone, Copy)]
pub struct Grid {
    pub pixels: usize,
    pub block: usize,
    pub hum_spacing: usize,
}

impl Grid {
    /// Indices of a coarser lattice in the block corners, in the same row-major order.
    pub(crate) fn corner_indices(self, spacing: usize) -> impl Iterator<Item = usize> {
        let side = self.pixels / spacing + 1;
        let corner_side = self.pixels / self.block + 1;
        let ratio = spacing / self.block;
        (0..side * side).map(move |at| (at / side) * ratio * corner_side + (at % side) * ratio)
    }

    /// The grid of the map's zoom (12 or 13): blocks of about 100 m and the hum's points about
    /// 400 m apart (790 m put a quiet Dobris pixel's total 1.5 phon over the popup's; at 400 m its
    /// cells over 1 phon went 0.75 -> 0 %; 200 m added five times as much time).
    pub fn at_zoom(zoom: u32) -> Option<Self> {
        match zoom {
            12 => Some(Grid {
                pixels: 512,
                block: 8,
                hum_spacing: 32,
            }),
            13 => Some(Grid {
                pixels: 1024,
                block: 16,
                hum_spacing: 64,
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

/// A pass's pixels: the outdoor ones' place in the square and paint, and the indoor ones' place and
/// building.
type PassPixels = (Vec<(usize, Painted)>, Vec<(usize, u64)>);

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
    pub(crate) corner_band_sums: Vec<[[f64; physics::bands::PERIODS]; LAYERS]>,
    corner_pairs: usize,
    seconds: f64,
}

/// The square's point at pixel coordinates (x, y), in the square's frame.
pub fn metres_at(square: &Square, n: usize, x: f64, y: f64) -> [f64; 2] {
    square.frame.to_metres(Mercator {
        x: f64::from(square.tile.x) + x / n as f64,
        y: f64::from(square.tile.y) + y / n as f64,
    })
}

/// The shorter pixel side, shared by façade ranking and the screening-edge protection.
pub(crate) fn shortest_pixel_side_metres(square: &Square, pixels: usize) -> f64 {
    let [a, b] = [0.0, 1.0].map(|at| metres_at(square, pixels, at, at));
    (b[0] - a[0]).abs().min((b[1] - a[1]).abs())
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

/// The square's block corners as points, row-major from the north-west, `pixels / block + 1` a side.
pub fn corner_points(square: &Square, grid: Grid) -> Result<Vec<Point>, String> {
    lattice_points(square, grid.pixels, grid.block)
}

/// Points on a regular pixel lattice, including its shared square edges.
pub(crate) fn lattice_points(
    square: &Square,
    pixels: usize,
    spacing: usize,
) -> Result<Vec<Point>, String> {
    let side = pixels / spacing + 1;
    (0..side * side)
        .into_par_iter()
        .map(|at| {
            let (x, y) = ((at % side) * spacing, (at / side) * spacing);
            Point::at(square, metres_at(square, pixels, x as f64, y as f64))
        })
        .collect()
}

/// The square's corners: each one's sources loud there evaluated exactly and ranked, its flights,
/// the hum, the flights' spectra; `None` for a square that is silent everywhere.
pub fn corners_of(
    square: &Square,
    grid: Grid,
    batch: Option<&dyn Batch>,
) -> Result<Option<Corners>, String> {
    let n = grid.pixels;
    let rectangle = |x0: f64, y0: f64, x1: f64, y1: f64| rectangle_at(square, n, x0, y0, x1, y1);
    let no_flights = (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none())
        && square.rolls.is_empty();
    if square.candidates.is_empty() {
        let (low, high) = rectangle(0.0, 0.0, n as f64, n as f64);
        // Empty aircraft files still assess buildings: an unexposed facade remains unassessed.
        if no_flights
            || (!square.has_aircraft_boxes()
                && square.rolls.is_empty()
                && square.obstacles.is_open_rectangle(low, high))
        {
            return Ok(None);
        }
    }
    let started = std::time::Instant::now();
    let corner_points = corner_points(square, grid)?;
    let spectral_points: Vec<Point> = grid
        .corner_indices(2 * grid.block)
        .map(|at| corner_points[at].clone())
        .collect();
    let ((flights, flight_spectra), ground) = rayon::join(
        || {
            rayon::join(
                || flights_at_corners(square, grid, &corner_points, batch),
                || FlightSpectra::from_points(square, n, 2 * grid.block, &spectral_points),
            )
        },
        || -> Result<_, String> {
            // Every block corner evaluates every source loud there.
            let pairs = loud_pairs(square, &corner_points);
            let corner_pairs = pairs.len();
            let mut corners: Vec<Corner> = (0..corner_points.len())
                .map(|_| Corner::default())
                .collect();
            let evaluated = evaluate_all(square, &corner_points, &pairs, batch)?;
            for (pair, energy) in pairs.iter().zip(&evaluated.pairs) {
                let (k, index) = (pair.point, pair.candidate);
                let candidate = &square.candidates[index as usize];
                let distance_m = candidate.distance_from(corner_points[k as usize].position);
                corners[k as usize].add(
                    candidate.layer as usize,
                    (index, candidate.group_key),
                    (*energy, distance_m),
                );
            }
            let hum_points: Vec<Point> = grid
                .corner_indices(grid.hum_spacing)
                .map(|at| corner_points[at].clone())
                .collect();
            let loud_energy: Vec<_> = grid
                .corner_indices(grid.hum_spacing)
                .map(|at| {
                    std::array::from_fn(|layer| {
                        let mut sum = [0.0; physics::bands::PERIODS];
                        // Before sorting: identical source order to the independent hum's loud pairs.
                        for ranked in &corners[at].ranked[layer] {
                            for (sum, value) in sum.iter_mut().zip(ranked.energy.mixed) {
                                *sum += value;
                            }
                        }
                        sum
                    })
                })
                .collect();
            let hum = Hum::from_loud(
                square,
                n,
                grid.hum_spacing,
                &hum_points,
                &loud_energy,
                batch,
            )?;
            Ok((corners, evaluated.bands, corner_pairs, hum))
        },
    );
    let (flights, flight_spectra) = (flights?, flight_spectra?);
    let (mut corners, corner_bands, corner_pairs, hum) = ground?;
    let mut near = Vec::new();
    let (low, high) = rectangle(0.0, 0.0, n as f64, n as f64);
    square.index.reaching(low, high, &mut near);
    if near.is_empty() && no_flights && hum.silent() {
        return Ok(None);
    }
    // A corner's bands: the shape of what its blocks leave out there.
    let corner_band_sums = corner_bands
        .iter()
        .map(|bands| {
            std::array::from_fn(|layer| {
                std::array::from_fn(|period| bands[layer][period].iter().sum())
            })
        })
        .collect();
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
        displays: Displays::of(square)?,
        corners,
        corner_bands,
        corner_band_sums,
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
) -> Result<PaintedCells, String> {
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
    let quiet = QuietBlocks::new(&painter)?;
    if quiet.known_blocks.iter().all(|&quiet| quiet) {
        let cells = quiet.cells(&painter);
        report_pixels(corners_s, corner_pairs, started, painter.pairs.into_inner());
        return cells;
    }
    // Every pixel, a run of blocks at a time (four block rows, fewer where their pairs would pass
    // PASS_PAIRS): the outdoor ones painted, the indoor ones' place and building kept for their
    // building's façade.
    let pass_of = |run: Range<usize>| -> Result<PassPixels, String> {
        let block_ids: Vec<usize> = run.filter(|&at| !quiet.known_blocks[at]).collect();
        if block_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let pass: Vec<_> = (block_ids.par_iter())
            .map_init(Vec::new, |near, &at| quiet.block(&painter, near, at))
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
            .filter(|&(_, at)| !quiet.has_paint(&painter, at))
            .collect();
        let points: Vec<Point> = (places.par_iter())
            .map(|&(_, at)| Point::at(square, metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5)))
            .collect::<Result<_, String>>()?;
        let painted = painter.paint(&pass, &points, &places)?;
        let indoor = (points.iter().zip(&places))
            .filter(|(point, _)| point.own_footprint != 0)
            .map(|(point, &(_, at))| (at, point.own_footprint))
            .collect();
        let outdoor = (painted.into_iter().zip(&places))
            .filter_map(|(paint, &(_, at))| paint.map(|paint| (at, paint)))
            .collect();
        Ok((outdoor, indoor))
    };
    let bounds: Vec<usize> = quiet
        .exact_source_counts
        .par_iter()
        .enumerate()
        .map(|(block, &count)| {
            if quiet.known_blocks[block] || count == 0 {
                return 0;
            }
            let remaining = (0..area)
                .filter(|&inside| {
                    let x = (block % blocks) * grid.block + inside % grid.block;
                    let y = (block / blocks) * grid.block + inside / grid.block;
                    !quiet.has_paint(&painter, y * n + x)
                })
                .count();
            count * remaining
        })
        .collect();
    let painted = painter.in_passes(runs_within(&bounds, PASS_BLOCKS), pass_of)?;
    let mut pixels: Vec<Option<Painted>> = vec![None; n * n];
    quiet.fill(&painter, &mut pixels);
    let mut indoor = Vec::new();
    for (_, (outdoor, inside)) in painted {
        indoor.extend(inside);
        for (at, paint) in outdoor {
            pixels[at] = Some(paint);
        }
    }
    let candidates = facade_candidates(square, n, &pixels, &indoor)?;
    let levels = building_levels(&painter, &quiet, candidates)?;
    fill_buildings(&mut pixels, &indoor, &levels);
    let cells = PaintedCells::from_painted(pixels.into_par_iter().enumerate().map(
        |(at, pixel)| match pixel {
            Some(paint) => Some(paint),
            None if quiet.has_paint(&painter, at) => Some(quiet.paint_at(&painter, at)),
            None => None,
        },
    ))?;
    report_pixels(corners_s, corner_pairs, started, painter.pairs.into_inner());
    Ok(cells)
}

fn report_pixels(corners_s: f64, corner_pairs: usize, started: std::time::Instant, pairs: usize) {
    eprintln!(
        "qm-paint: hum and corners {corners_s:.1} s ({corner_pairs} pairs), pixels {:.1} s ({} pairs)",
        started.elapsed().as_secs_f64(),
        pairs,
    );
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
