//! A square painted. A pixel evaluates exactly the loud ground sources (bound there at least the
//! edge, `popup::candidates::loud`) crossing its 16-pixel block and the blocks around. Every other
//! loud source is blended from where it was evaluated exactly, its near share from the block's
//! corners and its far share from the points of the block's far cell (64 pixels), in groups by
//! layer, direction and distance whose energy one exact ray from the loudest member sets right at
//! the pixel ([`crate::groups`]). The quiet hum is blended from the lattice ([`Lattice`]), the
//! flights from the corners. A pixel inside an enclosed building has no level: the map shows the
//! sound outdoors, and a click there answers at the loudest façade.

use crate::exact::{Batch, LAYERS, Point, evaluate};
use crate::flights::flights_at_corners;
use crate::groups::{Four, Frame, frame, frame_weight, group_energy, meets};
use crate::lattice::Lattice;
use crate::square::Square;
use physics::bands::level_db;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// A cell with no level: quiet (under 0 dB) or inside a building.
pub const NO_LEVEL: u8 = 255;

/// How a square is cut: pixels per side, pixels per block, pixels per far cell (the far shares'
/// lattice and the groups' frames) and per cell of the quiet hum's lattice.
#[derive(Clone, Copy)]
pub struct Grid {
    pub pixels: usize,
    pub block: usize,
    pub far: usize,
    pub coarse: usize,
}

/// Block rows one pass of pixels takes (4 rows of 64 blocks at z13, 65,536 pixels).
const PIXEL_PASS_BLOCK_ROWS: usize = 4;

/// A source's near share by its distance from a block's centre, in far cells: whole within the
/// first, none beyond the second, linear between (at z13 about 300 and 800 m). Against the etalon
/// in Dobříš a ramp from 300 to 800 m had industry over 1 dB at 2.8 % where a switch at 400 m had
/// 4.3 % (`evidence/2026-10-10/painter-plan`).
const NEAR_SHARE_CELLS: (f64, f64) = (0.75, 2.0);

/// What a block's pixels share: the loud sources they evaluate exactly and the frames their
/// groups are blended in.
struct Block {
    local: Vec<u32>,
    frames: Vec<Frame>,
}

/// Per pixel (row-major from the north-west) its cell per layer and the total's last: twice the
/// Lden rounded, or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

/// A pass's painted pixels: each one's place in the square and its cells.
type PassCells = Vec<(usize, [u8; LAYERS + 1])>;

fn cell(energy: f64) -> u8 {
    if energy < 1.0 {
        NO_LEVEL
    } else {
        (2.0 * level_db(energy)).round().min(254.0) as u8
    }
}

/// The cells of a pixel's energy per layer: each layer's and the total's.
pub(crate) fn cells_of(layers: &[f64; LAYERS]) -> [u8; LAYERS + 1] {
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
    let lattice = Lattice::new(square, n, (grid.coarse, 0), true, batch)?;
    // One far cell beyond the square, so its edge blends with the next square's groups too.
    let far_lattice = Lattice::new(square, n, (grid.far, 1), false, batch)?;
    // A square no loud source reaches, no flight crosses and whose hum stays under 0 dB (the
    // reflection adding at most 3 dB) has no level: most of the world.
    let mut near = Vec::new();
    let (low, high) = rectangle(0.0, 0.0, n as f64, n as f64);
    square.index.reaching(low, high, &mut near);
    let no_flights = (square.aircraft.iter()).all(|tile| tile.fine.is_none() && tile.far.is_none());
    if near.is_empty() && no_flights && lattice.silent() {
        return Ok(vec![[NO_LEVEL; LAYERS + 1]; n * n]);
    }
    // The block corners evaluate every loud source with a near share for a block they bound.
    let pixel_m = square.frame.east_m_per_unit / n as f64;
    let cell_m = pixel_m * grid.far as f64;
    let (whole_m, none_m) = (NEAR_SHARE_CELLS.0 * cell_m, NEAR_SHARE_CELLS.1 * cell_m);
    let corner_reach_m = none_m + std::f64::consts::SQRT_2 * pixel_m * grid.block as f64;
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
                .filter(|&&index| {
                    square.candidates[index as usize].distance_from(point.position) < corner_reach_m
                        && square.loud(index, point.position, &point.bound)
                })
                .map(|&index| (k as u32, index))
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect();
    let corner_pairs = pairs.len();
    let energies = evaluate(square, &corner_points, &pairs, batch)?;
    let mut corners: Vec<Vec<(u32, f64)>> = vec![Vec::new(); corner_points.len()];
    for (&(k, index), energy) in pairs.iter().zip(energies) {
        corners[k as usize].push((index, energy));
    }
    corners
        .par_iter_mut()
        .for_each(|corner| corner.sort_unstable_by_key(|&(index, _)| index));
    let flights = flights_at_corners(square, grid, &metres, &corner_points)?;
    let corners_s = started.elapsed().as_secs_f64();

    // A block: its local sources, and its groups in its own far cell's frame and, near the cell's
    // edges, in the neighbouring cells' frames.
    let ratio = grid.far / grid.block;
    let band = grid.block as f64 / 4.0;
    // The far lattice's points of the far cell `(ci, cj)` (-1 to the square's cells: the margin).
    let four = |ci: i64, cj: i64| {
        let margin = far_lattice.margin as i64;
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| {
            ((cj + margin + dj) * far_lattice.side as i64 + ci + margin + di) as usize
        })
    };
    let block_of = |near: &mut Vec<u32>, at: usize| -> Block {
        let (i, j) = (at % blocks, at / blocks);
        let b = grid.block as f64;
        let (x, y) = (i as f64 * b, j as f64 * b);
        let (low, high) = rectangle(x - b, y - b, x + 2.0 * b, y + 2.0 * b);
        square.index.reaching(low, high, near);
        let mut local: Vec<u32> = (near.iter().copied())
            .filter(|&index| {
                let [a, b] = square.candidates[index as usize].ends_m;
                meets(a, b, low, high)
            })
            .collect();
        local.sort_unstable();
        let centre = metres(x + b / 2.0, y + b / 2.0);
        let near_share = |index: u32| {
            local.binary_search(&index).is_err().then(|| {
                let distance = square.candidates[index as usize].distance_from(centre);
                ((none_m - distance) / (none_m - whole_m)).clamp(0.0, 1.0)
            })
        };
        let corner_lists: Four = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .map(|(di, dj)| corners[(j + dj) * corner_side + i + di].as_slice());
        let (ci, cj) = ((i / ratio) as i64, (j / ratio) as i64);
        let cell_lists: Four = four(ci, cj).map(|k| far_lattice.loud[k].as_slice());
        let along = |start: usize| {
            let mut offsets = vec![0i64];
            if (start as f64) < band {
                offsets.push(-1);
            }
            if (start + grid.block) as f64 > grid.far as f64 - band {
                offsets.push(1);
            }
            offsets
        };
        let mut frames = Vec::new();
        for dy in along((j % ratio) * grid.block) {
            for dx in along((i % ratio) * grid.block) {
                let (fi, fj) = (ci + dx, cj + dy);
                let frame_lists: Four = four(fi, fj).map(|k| far_lattice.loud[k].as_slice());
                let centre = metres(
                    (fi as f64 + 0.5) * grid.far as f64,
                    (fj as f64 + 0.5) * grid.far as f64,
                );
                frames.push(frame(
                    square,
                    (corner_lists, cell_lists, frame_lists),
                    &near_share,
                    (centre, [dx, dy]),
                ));
            }
        }
        Block { local, frames }
    };

    // Every pixel, a few block rows at a time: its loud local sources and its block's probes.
    let pass_of = |first_row: usize| -> Result<PassCells, String> {
        let block_ids: Vec<usize> = (first_row..(first_row + PIXEL_PASS_BLOCK_ROWS).min(blocks))
            .flat_map(|by| (0..blocks).map(move |bx| by * blocks + bx))
            .collect();
        let pass: Vec<Block> = (block_ids.par_iter())
            .map_init(Vec::new, |near, &at| block_of(near, at))
            .collect();
        let outdoor: Vec<(usize, usize, Point)> = (0..block_ids.len() * grid.block * grid.block)
            .into_par_iter()
            .map(|k| {
                let (slot, inside) = (k / (grid.block * grid.block), k % (grid.block * grid.block));
                let at = block_ids[slot];
                let x = (at % blocks) * grid.block + inside % grid.block;
                let y = (at / blocks) * grid.block + inside / grid.block;
                let point = Point::at(square, metres(x as f64 + 0.5, y as f64 + 0.5))?;
                Ok((point.own_footprint == 0).then_some((slot, y * n + x, point)))
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect();
        // A pixel's frames: its own cell's, and within the band of an edge the neighbour's.
        let side = grid.far as f64;
        // A pixel's centre within its far cell, in pixels.
        let cell_pixel = |at: usize| {
            (
                (at % n % grid.far) as f64 + 0.5,
                (at / n % grid.far) as f64 + 0.5,
            )
        };
        // Per pixel its loud local sources, then the probes of its frames (sorted).
        let lists: Vec<(Vec<u32>, usize)> = (outdoor.par_iter())
            .map(|(slot, at, point)| {
                let block = &pass[*slot];
                let mut list: Vec<u32> = (block.local.iter().copied())
                    .filter(|&index| square.loud(index, point.position, &point.bound))
                    .collect();
                let local = list.len();
                let mut probes: Vec<u32> = (block.frames.iter())
                    .filter(|frame| frame_weight(frame.offset, cell_pixel(*at), side, band) > 0.0)
                    .flat_map(|frame| frame.probed.iter().map(|group| group.probe.index))
                    .collect();
                probes.sort_unstable();
                probes.dedup();
                list.extend(probes);
                (list, local)
            })
            .collect();
        let pairs: Vec<(u32, u32)> = (lists.iter().enumerate())
            .flat_map(|(k, (list, _))| list.iter().map(move |&index| (k as u32, index)))
            .collect();
        let (places, points): (Vec<(usize, usize)>, Vec<Point>) = outdoor
            .into_iter()
            .map(|(slot, at, point)| ((slot, at), point))
            .unzip();
        let energies = evaluate(square, &points, &pairs, batch)?;
        let mut offset = 0;
        let starts: Vec<usize> = (lists.iter())
            .map(|(list, _)| {
                offset += list.len();
                offset - list.len()
            })
            .collect();
        Ok((0..points.len())
            .into_par_iter()
            .map(|k| {
                let ((slot, at), point) = (places[k], &points[k]);
                let block = &pass[slot];
                let (x, y) = (at % n, at / n);
                let within = |side: usize| {
                    (
                        ((x % side) as f64 + 0.5) / side as f64,
                        ((y % side) as f64 + 0.5) / side as f64,
                    )
                };
                let (in_block, in_cell) = (within(grid.block), within(grid.far));
                let (list, local) = (&lists[k].0, lists[k].1);
                let values = &energies[starts[k]..starts[k] + list.len()];
                let (probes, probe_values) = (&list[local..], &values[local..]);
                let mut layers = lattice.quiet_at(x, y);
                for (&index, &energy) in list[..local].iter().zip(&values[..local]) {
                    layers[square.candidates[index as usize].layer as usize] += energy;
                }
                let (mut grouped, mut weights) = ([0.0; LAYERS], 0.0);
                for frame in &block.frames {
                    let weight = frame_weight(frame.offset, cell_pixel(at), side, band);
                    if weight == 0.0 {
                        continue;
                    }
                    weights += weight;
                    for group in &frame.probed {
                        let here = probes
                            .binary_search(&group.probe.index)
                            .map_or(0.0, |slot| probe_values[slot]);
                        grouped[group.layer] +=
                            weight * group_energy(group, here, in_block, in_cell);
                    }
                    for (layer, energy) in grouped.iter_mut().enumerate() {
                        let rest = |points: &[[f64; LAYERS]; 4], (fx, fy): (f64, f64)| {
                            bilinear(points.map(|point| point[layer]), fx, fy)
                        };
                        *energy += weight
                            * (rest(&frame.rest_corners, in_block)
                                + rest(&frame.rest_cell, in_cell));
                    }
                }
                for (layer, energy) in layers.iter_mut().enumerate() {
                    *energy = (*energy + grouped[layer] / weights) * point.reflection();
                }
                let (i, j) = (x / grid.block, y / grid.block);
                let around = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .map(|(di, dj)| flights[(j + dj) * corner_side + i + di]);
                layers[Layer::Aircraft as usize] += bilinear(around, in_block.0, in_block.1);
                (at, cells_of(&layers))
            })
            .collect())
    };
    // With a card, two passes run at once: one's lists on the cores while the other's pairs are
    // on the card.
    let first_rows: Vec<usize> = (0..blocks).step_by(PIXEL_PASS_BLOCK_ROWS).collect();
    let painted: Vec<Result<PassCells, String>> = match batch {
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
    let mut cells = vec![[NO_LEVEL; LAYERS + 1]; n * n];
    for pass in painted {
        for (at, value) in pass? {
            cells[at] = value;
        }
    }
    eprintln!(
        "qm-paint: lattice and corners {corners_s:.1} s ({corner_pairs} corner pairs), pixels {:.1} s",
        started.elapsed().as_secs_f64() - corners_s
    );
    Ok(cells)
}
