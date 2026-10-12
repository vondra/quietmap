//! A square painted as dev1 painted it, in blocks of 16 x 16 pixels. At every block corner each
//! ground source loud there (`popup::candidates::loud`) is evaluated exactly. A block's pixels
//! evaluate exactly the loud sources crossing it or the eight blocks around and, layer by layer,
//! the loudest at its corners until what is left out weighs at most a share of its quietest
//! corner's ([`crate::blocks`]); what is left out is blended from the corners, the quiet hum from
//! its points ([`Hum`]), the flights from the corners. A building's pixels take its loudest façade
//! from the outdoor pixels along its outline, as a click inside it answers ([`crate::buildings`]).

use crate::blocks::{Block, Corner, admit, meets};
use crate::buildings::{fill_buildings, own_reflection_db};
use crate::exact::{Batch, LAYERS, Point, evaluate};
use crate::flights::flights_at_corners;
use crate::hum::Hum;
use crate::square::Square;
use physics::bands::level_db;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// A cell not assessed: a building with no painted pixel beside it, or nothing painted there.
pub const NO_LEVEL: u8 = 255;
/// A cell assessed and silent: under 0 dB (dev4's HM3 v4; the quiet zones count it quiet, a zoom out
/// averages it as no energy).
pub const SILENCE: u8 = 254;

/// How a square is cut: pixels per side, per block and between the hum's points.
#[derive(Clone, Copy)]
pub struct Grid {
    pub pixels: usize,
    pub block: usize,
    pub hum: usize,
}

/// Block rows one pass of pixels takes (4 rows of 64 blocks at z13, 65,536 pixels).
const PIXEL_PASS_BLOCK_ROWS: usize = 4;

/// Per pixel (row-major from the north-west) its cell per layer and the total's last: twice the
/// Lden rounded, [`SILENCE`] under 0 dB, or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

/// A pass's pixels: the outdoor ones' place in the square and energy per layer (the flights not yet
/// added), and the indoor ones' place and building.
type PassLayers = (Vec<(usize, [f64; LAYERS])>, Vec<(usize, u64)>);

fn cell(energy: f64) -> u8 {
    if energy < 1.0 {
        SILENCE
    } else {
        (2.0 * level_db(energy)).round().min(f64::from(SILENCE - 1)) as u8
    }
}

/// The cells of a pixel's energy per layer: each layer's and the total's.
pub fn cells_of(layers: &[f64; LAYERS]) -> [u8; LAYERS + 1] {
    let mut cells = [NO_LEVEL; LAYERS + 1];
    for (l, &value) in layers.iter().enumerate() {
        cells[l] = cell(value);
    }
    cells[LAYERS] = cell(layers.iter().sum());
    cells
}

/// Bilinear between four corners (north-west, north-east, south-west, south-east).
pub(crate) fn bilinear(corners: [f64; 4], fx: f64, fy: f64) -> f64 {
    let north = corners[0] * (1.0 - fx) + corners[1] * fx;
    let south = corners[2] * (1.0 - fx) + corners[3] * fx;
    north * (1.0 - fy) + south * fy
}

/// Paints the square: every pixel's cells, the exact evaluations here or by `batch`.
pub fn paint(square: &Square, grid: Grid, batch: Option<&dyn Batch>) -> Result<Cells, String> {
    let n = grid.pixels;
    let metres = |x: f64, y: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + x / n as f64,
            y: f64::from(square.tile.y) + y / n as f64,
        })
    };
    let rectangle = |x0: f64, y0: f64, x1: f64, y1: f64| {
        let (a, b) = (metres(x0, y0), metres(x1, y1));
        (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        )
    };
    let started = std::time::Instant::now();
    let hum = Hum::new(square, n, grid.hum, batch)?;
    // A square no loud source reaches, no flight crosses and whose hum stays under 0 dB (the
    // reflection adding at most 3 dB) has no level: most of the world.
    let mut near = Vec::new();
    let (low, high) = rectangle(0.0, 0.0, n as f64, n as f64);
    square.index.reaching(low, high, &mut near);
    let no_flights = (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none());
    if near.is_empty() && no_flights && hum.silent() {
        return Ok(vec![[SILENCE; LAYERS + 1]; n * n]);
    }
    // Every block corner evaluates every source loud there.
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
    let energies = evaluate(square, &corner_points, &pairs, batch)?;
    for (&(k, index), energy) in pairs.iter().zip(energies) {
        let layer = square.candidates[index as usize].layer as usize;
        let corner = &mut corners[k as usize];
        corner.ranked[layer].push((index, energy));
        corner.totals[layer] += energy;
    }
    corners.par_iter_mut().for_each(|corner| {
        for ranked in &mut corner.ranked {
            ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        }
    });
    let corners_s = started.elapsed().as_secs_f64();

    // A block: the loud sources crossing it or the eight around, and those its corners admit.
    let block_of = |near: &mut Vec<u32>, at: usize| -> Block {
        let (i, j) = (at % blocks, at / blocks);
        let b = grid.block as f64;
        let (x, y) = (i as f64 * b, j as f64 * b);
        let (low, high) = rectangle(x - b, y - b, x + 2.0 * b, y + 2.0 * b);
        square.index.reaching(low, high, near);
        let local: Vec<u32> = (near.iter().copied())
            .filter(|&index| {
                let [a, b] = square.candidates[index as usize].ends_m;
                meets(a, b, low, high)
            })
            .collect();
        let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| &corners[(j + dj) * corner_side + i + di]);
        admit(local, around)
    };

    // Every pixel, a few block rows at a time: its block's exact sources loud there.
    let pixel_pairs = std::sync::atomic::AtomicUsize::new(0);
    let pass_of = |first_row: usize| -> Result<PassLayers, String> {
        let block_ids: Vec<usize> = (first_row..(first_row + PIXEL_PASS_BLOCK_ROWS).min(blocks))
            .flat_map(|by| (0..blocks).map(move |bx| by * blocks + bx))
            .collect();
        let pass: Vec<Block> = (block_ids.par_iter())
            .map_init(Vec::new, |near, &at| block_of(near, at))
            .collect();
        // The pass's pixels block by block: a pixel's block (its slot in the pass) and its place in
        // the square. An indoor one takes its building's façade later.
        let place = |k: usize| {
            let (slot, inside) = (k / (grid.block * grid.block), k % (grid.block * grid.block));
            let at = block_ids[slot];
            let x = (at % blocks) * grid.block + inside % grid.block;
            let y = (at / blocks) * grid.block + inside / grid.block;
            (slot, y * n + x)
        };
        let points: Vec<Point> = (0..block_ids.len() * grid.block * grid.block)
            .into_par_iter()
            .map(|k| {
                let at = place(k).1;
                Point::at(square, metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5))
            })
            .collect::<Result<_, String>>()?;
        let indoor: Vec<(usize, u64)> = (points.iter().enumerate())
            .filter(|(_, point)| point.own_footprint != 0)
            .map(|(k, point)| (place(k).1, point.own_footprint))
            .collect();
        let lists: Vec<Vec<u32>> = (points.par_iter().enumerate())
            .map(|(k, point)| {
                if point.own_footprint != 0 {
                    return Vec::new();
                }
                (pass[place(k).0].exact.iter().copied())
                    .filter(|&index| square.loud(index, point.position, &point.bound))
                    .collect()
            })
            .collect();
        let pairs: Vec<(u32, u32)> = (lists.iter().enumerate())
            .flat_map(|(k, list)| list.iter().map(move |&index| (k as u32, index)))
            .collect();
        pixel_pairs.fetch_add(pairs.len(), std::sync::atomic::Ordering::Relaxed);
        let energies = evaluate(square, &points, &pairs, batch)?;
        let mut offset = 0;
        let starts: Vec<usize> = (lists.iter())
            .map(|list| {
                offset += list.len();
                offset - list.len()
            })
            .collect();
        let within = |t: usize| ((t % grid.block) as f64 + 0.5) / grid.block as f64;
        let painted = (0..points.len())
            .into_par_iter()
            .filter(|&k| points[k].own_footprint == 0)
            .map(|k| {
                let ((slot, at), point) = (place(k), &points[k]);
                let (x, y) = (at % n, at / n);
                let mut layers = hum.at(x, y);
                let values = &energies[starts[k]..starts[k] + lists[k].len()];
                for (&index, &energy) in lists[k].iter().zip(values) {
                    layers[square.candidates[index as usize].layer as usize] += energy;
                }
                let left_out = &pass[slot].left_out;
                for (layer, energy) in layers.iter_mut().enumerate() {
                    let rest = bilinear(left_out.map(|corner| corner[layer]), within(x), within(y));
                    *energy = (*energy + rest) * point.reflection();
                }
                (at, layers)
            })
            .collect();
        Ok((painted, indoor))
    };
    // The flights are the cores' work alone: they run beside the pixel passes. With a card, two
    // passes run at once: one's lists on the cores while the other's pairs are on the card.
    let first_rows: Vec<usize> = (0..blocks).step_by(PIXEL_PASS_BLOCK_ROWS).collect();
    let (painted, flights) = std::thread::scope(|scope| {
        let flights = scope.spawn(|| flights_at_corners(square, grid, &metres, &corner_points));
        let painted: Vec<Result<PassLayers, String>> = match batch {
            None => first_rows.iter().map(|&row| pass_of(row)).collect(),
            Some(_) => {
                let (even, odd): (Vec<usize>, Vec<usize>) = (first_rows.iter())
                    .partition(|&&row| (row / PIXEL_PASS_BLOCK_ROWS).is_multiple_of(2));
                let pass_of = &pass_of;
                let other =
                    scope.spawn(move || odd.iter().map(|&row| pass_of(row)).collect::<Vec<_>>());
                let mut done: Vec<_> = even.iter().map(|&row| pass_of(row)).collect();
                done.extend(
                    other
                        .join()
                        .unwrap_or_else(|_| vec![Err("a pixel pass panicked".into())]),
                );
                done
            }
        };
        (painted, flights.join())
    });
    let flights = flights.map_err(|_| "the flights panicked".to_string())??;
    let mut energies: Vec<Option<[f64; LAYERS]>> = vec![None; n * n];
    let mut indoor = Vec::new();
    for pass in painted {
        let (outdoor, inside) = pass?;
        indoor.extend(inside);
        for (at, mut layers) in outdoor {
            let (x, y) = (at % n, at / n);
            let (i, j) = (x / grid.block, y / grid.block);
            let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .map(|(di, dj)| flights[(j + dj) * corner_side + i + di]);
            let within = |t: usize| ((t % grid.block) as f64 + 0.5) / grid.block as f64;
            layers[Layer::Aircraft as usize] += bilinear(around, within(x), within(y));
            energies[at] = Some(layers);
        }
    }
    fill_buildings(&mut energies, n, &indoor, &|at, footprint| {
        own_reflection_db(
            square,
            metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5),
            footprint,
        )
    })?;
    let cells = (energies.par_iter())
        .map(|layers| layers.as_ref().map_or([NO_LEVEL; LAYERS + 1], cells_of))
        .collect();
    eprintln!(
        "qm-paint: hum and corners {corners_s:.1} s ({corner_pairs} pairs), pixels {:.1} s ({} pairs)",
        started.elapsed().as_secs_f64() - corners_s,
        pixel_pairs.into_inner(),
    );
    Ok(cells)
}
