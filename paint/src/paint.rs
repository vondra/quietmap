//! A square painted as dev1 painted it, in blocks of 16 x 16 pixels. At every block corner each
//! ground source loud there (`popup::candidates::loud`) is evaluated exactly. A block's pixels
//! evaluate exactly the loud sources crossing it or the eight blocks around and, layer by layer,
//! the loudest at its corners until what is left out weighs at most a share of its quietest
//! corner's ([`crate::blocks`]); what is left out is blended from the corners, the quiet hum from
//! its points ([`Hum`]), the flights from the corners. Each pixel's loudness (Nden, sone) per layer
//! and in total is the popup's model on those ([`crate::sone`]); a block hearing only the hum and
//! the flights (no source of its own, no wall) blends its corners'. A building's pixels take its
//! loudest façade from the outdoor pixels along its outline, as a click inside it answers
//! ([`crate::buildings`]).

use crate::blocks::{Block, Corner, admit, meets};
use crate::buildings::{fill_buildings, own_reflection_db};
use crate::exact::{Batch, LAYERS, Point, PointBands, evaluate_all};
use crate::flights::{FlightSpectra, blend_flights, flights_at_corners};
use crate::hum::Hum;
use crate::sone::{Displays, Rest, pixel_ndens};
use crate::square::Square;
use physics::bands::{BANDS, PERIODS, lden_energy};
use popup::percentiles::Resolution;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// A cell not assessed: a building with no painted pixel beside it, or nothing painted there.
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
    /// cells over 1 phon went 0.75 -> 0 % for 7 s more a square, at 200 m for 36 s).
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

/// Block rows one pass of pixels takes (4 rows of 64 blocks at z13, 65,536 pixels).
const PIXEL_PASS_BLOCK_ROWS: usize = 4;

/// Per pixel (row-major from the north-west) its cell per layer and the total's last ([`cell`]),
/// [`SILENCE`] or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

/// An outdoor pixel painted: per layer its Lden energy (a building's façade is chosen by it) and its
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
        return Ok(vec![[SILENCE; LAYERS + 1]; n * n]);
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
    let pass_of = |first_row: usize| -> Result<PassPixels, String> {
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
        let evaluated = evaluate_all(square, &points, &pairs, batch)?;
        let mut offset = 0;
        let starts: Vec<usize> = (lists.iter())
            .map(|list| {
                offset += list.len();
                offset - list.len()
            })
            .collect();
        let within = |t: usize| ((t % grid.block) as f64 + 0.5) / grid.block as f64;
        // A block hearing no source of its own, left out or near, and no wall: only the hum and the
        // flights, both blended, so its loudness is blended from its corners'.
        let area = grid.block * grid.block;
        let smooth: Vec<Option<[[f64; LAYERS + 1]; 4]>> = (0..pass.len())
            .into_par_iter()
            .map(|slot| -> Result<_, String> {
                let block = &pass[slot];
                let walls = (slot * area..(slot + 1) * area)
                    .any(|k| points[k].own_footprint != 0 || points[k].reflection_db != 0.0);
                if !block.exact.is_empty() || !block.left_groups.is_empty() || walls {
                    return Ok(None);
                }
                let (i, j) = (block_ids[slot] % blocks, block_ids[slot] / blocks);
                let silent: PointBands = [[[0.0; BANDS]; PERIODS]; LAYERS];
                let corner = |(di, dj): (usize, usize)| {
                    let at = (j + dj) * corner_side + i + di;
                    let (x, y) = ((i + di) * grid.block, (j + dj) * grid.block);
                    let (steady, bands) = hum.at(x, y);
                    let rest = Rest {
                        steady,
                        bands,
                        left_groups: (&[], [0.0; 4]),
                        flights: flights[at],
                    };
                    pixel_ndens(
                        square,
                        &displays,
                        (corner_points[at].position, 1.0),
                        (&[], &[], &silent),
                        (&rest, &flight_spectra.at(x, y)),
                        Resolution::MAP,
                    )
                };
                Ok(Some([
                    corner((0, 0))?,
                    corner((1, 0))?,
                    corner((0, 1))?,
                    corner((1, 1))?,
                ]))
            })
            .collect::<Result<_, String>>()?;
        let painted = (0..points.len())
            .into_par_iter()
            .filter(|&k| points[k].own_footprint == 0)
            .map(|k| -> Result<_, String> {
                let ((slot, at), point) = (place(k), &points[k]);
                let (x, y) = (at % n, at / n);
                let (fx, fy) = (within(x), within(y));
                let (i, j) = (x / grid.block, y / grid.block);
                let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .map(|(di, dj)| (j + dj) * corner_side + i + di);
                let weights = corner_weights(fx, fy);
                // What the block leaves out beside the hum, each corner's in the shape of all it
                // hears in that layer; of it each group's own.
                let (mut steady, mut bands) = hum.at(x, y);
                let left_out = &pass[slot].left_out;
                for layer in 0..LAYERS {
                    for p in 0..PERIODS {
                        for (c, &corner) in around.iter().enumerate() {
                            let energy = weights[c] * left_out[c][layer][p];
                            steady[layer][p] += energy;
                            let shape = &corner_bands[corner][layer][p];
                            let sum: f64 = shape.iter().sum();
                            if energy > 0.0 && sum > 0.0 {
                                for b in 0..BANDS {
                                    bands[layer][p][b] += energy * shape[b] / sum;
                                }
                            }
                        }
                    }
                }
                let rest = Rest {
                    steady,
                    bands,
                    left_groups: (&pass[slot].left_groups, weights),
                    flights: blend_flights(around.map(|corner| flights[corner]), fx, fy),
                };
                let range = starts[k]..starts[k] + lists[k].len();
                let reflection = point.reflection();
                let mut layers: [f64; LAYERS] =
                    std::array::from_fn(|layer| lden_energy(&rest.steady[layer]));
                for (&index, energy) in lists[k].iter().zip(&evaluated.pairs[range.clone()]) {
                    layers[square.candidates[index as usize].layer as usize] +=
                        lden_energy(&energy.mixed);
                }
                let mut layers = layers.map(|energy| reflection * energy);
                layers[Layer::Aircraft as usize] += lden_energy(&rest.flights[0]);
                let ndens = match &smooth[slot] {
                    Some(corners) => std::array::from_fn(|l| {
                        (0..4).map(|c| weights[c] * corners[c][l]).sum::<f64>()
                    }),
                    None => pixel_ndens(
                        square,
                        &displays,
                        (point.position, reflection),
                        (&lists[k], &evaluated.pairs[range], &evaluated.bands[k]),
                        (&rest, &flight_spectra.at(x, y)),
                        Resolution::MAP,
                    )?,
                };
                Ok((at, (layers, ndens)))
            })
            .collect::<Result<_, String>>()?;
        Ok((painted, indoor))
    };
    // With a card, two passes run at once: one's lists and loudness on the cores while the other's
    // pairs are on the card.
    let first_rows: Vec<usize> = (0..blocks).step_by(PIXEL_PASS_BLOCK_ROWS).collect();
    let painted: Vec<Result<PassPixels, String>> = match batch {
        None => first_rows.iter().map(|&row| pass_of(row)).collect(),
        Some(_) => std::thread::scope(|scope| {
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
        }),
    };
    let mut pixels: Vec<Option<Painted>> = vec![None; n * n];
    let mut indoor = Vec::new();
    for pass in painted {
        let (outdoor, inside) = pass?;
        indoor.extend(inside);
        for (at, paint) in outdoor {
            pixels[at] = Some(paint);
        }
    }
    fill_buildings(&mut pixels, n, &indoor, &|at, footprint| {
        own_reflection_db(
            square,
            metres((at % n) as f64 + 0.5, (at / n) as f64 + 0.5),
            footprint,
        )
    })?;
    let cells = (pixels.par_iter())
        .map(|pixel| {
            pixel
                .as_ref()
                .map_or([NO_LEVEL; LAYERS + 1], |(_, ndens)| cells_of(ndens))
        })
        .collect();
    eprintln!(
        "qm-paint: hum and corners {corners_s:.1} s ({corner_pairs} pairs), pixels {:.1} s ({} pairs)",
        started.elapsed().as_secs_f64() - corners_s,
        pixel_pairs.into_inner(),
    );
    Ok(cells)
}
