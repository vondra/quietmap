//! A square painted: the far level at the corners of its coarse cells, the mid level and the
//! aircraft at the corners of its pixel blocks, each interpolated bilinearly in energy, and at
//! every pixel the near level evaluated there, all times the pixel's own receiver reflection.
//! A pixel inside an enclosed building has no level: the map shows the sound outdoors, and a
//! click there answers at the loudest façade.

use crate::levels::{LAYERS, Levels, Point, Rule, Work, energies, energy_of};
use crate::square::{Square, gap_to_tile};
use physics::bands::{energy, level_db};
use popup::aircraft::FINE_BOXES_WITHIN_M;
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, AircraftReceiver, tile_lden_energy};
use popup::aircraft::horizons::Horizons;
use rayon::prelude::*;
use tiles::geo::Mercator;
use tiles::sources::Layer;

/// A cell with no level: quiet (under 0 dB) or inside a building.
pub const NO_LEVEL: u8 = 255;

/// How a square is cut: pixels per side, pixels per block and per coarse cell.
#[derive(Clone, Copy)]
pub struct Grid {
    pub pixels: usize,
    pub block: usize,
    pub coarse: usize,
}

/// The levels' evaluation at their points: the bounds left out under 0.2 dB of the known energy,
/// or the loudest bounds and a sample of the rest. Prague's square 2212/1387 (34 pixels outdoors
/// against the popup there) reads the same with 16 + 16 at the pixels as with 128 + 64 (mean
/// difference 0.34 against 0.36 dB) in a third of the time.
const FAR_RULE: Rule = Rule {
    tolerance: 0.047,
    proven: 128,
    sample: 128,
};
const MID_RULE: Rule = Rule {
    tolerance: 0.047,
    proven: 64,
    sample: 64,
};
const NEAR_RULE: Rule = Rule {
    tolerance: 0.047,
    proven: 16,
    sample: 16,
};

/// The loudest mid-level sources a block corner names, which its block's pixels evaluate exactly
/// (screening near a pixel changes them where interpolation cannot: a courtyard): at most this
/// many, each at least [`NAMED_SHARE`] of the corner's energy.
const NAMED_PER_CORNER: usize = 12;
const NAMED_SHARE: f64 = 0.02;

/// Per pixel (row-major from the north-west) its cell per layer and the total's last: twice the
/// Lden rounded, or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

fn cell(energy: f64) -> u8 {
    if energy < 1.0 {
        NO_LEVEL
    } else {
        (2.0 * level_db(energy)).round().min(254.0) as u8
    }
}

fn mix(seed: u64, value: u64) -> u64 {
    (seed ^ value)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .rotate_left(29)
}

/// Bilinear in energy between four corners (north-west, north-east, south-west, south-east).
fn bilinear(corners: [&[f64; LAYERS]; 4], fx: f64, fy: f64) -> [f64; LAYERS] {
    std::array::from_fn(|layer| {
        let north = corners[0][layer] * (1.0 - fx) + corners[1][layer] * fx;
        let south = corners[2][layer] * (1.0 - fx) + corners[3][layer] * fx;
        north * (1.0 - fy) + south * fy
    })
}

/// The aircraft's Lden energy at `point`, each box's share `weight` of its distance: every box
/// within `within_m`, fine boxes from tiles within their distance of the point, as the popup
/// reads them for a click there.
fn aircraft_energy(
    square: &Square,
    point: &Point,
    weight: &dyn Fn(f64) -> f64,
    within_m: f64,
) -> Result<f64, String> {
    let horizons = Horizons::build(
        &square.ground,
        &square.obstacles,
        point.position,
        point.altitude_m,
        point.own_footprint,
    )?;
    let receiver = AircraftReceiver {
        position: point.position,
        altitude_m: point.altitude_m,
    };
    let mut total = 0.0;
    for tile in &square.aircraft {
        let gap = gap_to_tile(&square.frame, tile.tile, point.position);
        if gap > within_m {
            continue;
        }
        let boxes = if gap <= FINE_BOXES_WITHIN_M {
            tile.fine.as_ref().or(tile.far.as_ref())
        } else {
            tile.far.as_ref()
        };
        if let Some(boxes) = boxes {
            total += tile_lden_energy(
                boxes,
                tile.tile,
                &square.frame,
                receiver,
                &horizons,
                (weight, within_m),
            );
        }
    }
    Ok(total)
}

/// Paints the square: every pixel's cells.
pub fn paint(square: &Square, grid: Grid) -> Result<Cells, String> {
    let n = grid.pixels;
    let metres = |x: f64, y: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(square.tile.x) + x / n as f64,
            y: f64::from(square.tile.y) + y / n as f64,
        })
    };
    let pixel_m = square.frame.east_m_per_unit / n as f64;
    let (block_m, coarse_m) = (pixel_m * grid.block as f64, pixel_m * grid.coarse as f64);
    let levels = Levels {
        near: [1.5 * block_m, 4.0 * block_m],
        far: [1.5 * coarse_m, 3.0 * coarse_m],
    };
    let seed = mix(u64::from(square.tile.x), u64::from(square.tile.y));
    let everything = square.candidates.len() as u32;

    let started = std::time::Instant::now();
    // The far level at the coarse corners.
    let coarse_side = n / grid.coarse + 1;
    let coarse: Vec<[f64; LAYERS]> = (0..coarse_side * coarse_side)
        .into_par_iter()
        .map_init(Work::default, |work, at| {
            let (i, j) = (at % coarse_side, at / coarse_side);
            let position = metres((i * grid.coarse) as f64, (j * grid.coarse) as f64);
            let point = Point::at(square, position)?;
            let mut far = energies(
                square,
                &point,
                &mut (0..everything),
                &|d| levels.far(d),
                &[0.0; LAYERS],
                &FAR_RULE,
                mix(seed, at as u64),
                work,
            )?;
            far[Layer::Aircraft as usize] +=
                aircraft_energy(square, &point, &|d| levels.far(d), AIRCRAFT_REACH_M)?;
            Ok(far)
        })
        .collect::<Result<_, String>>()?;

    let coarse_s = started.elapsed().as_secs_f64();
    // The aircraft's near level every second block corner: boxes change over hundreds of metres.
    let air_spacing = 2 * grid.block;
    let air_side = n / air_spacing + 1;
    let air: Vec<f64> = (0..air_side * air_side)
        .into_par_iter()
        .map(|at| {
            let (i, j) = (at % air_side, at / air_side);
            let position = metres((i * air_spacing) as f64, (j * air_spacing) as f64);
            let point = Point::at(square, position)?;
            aircraft_energy(square, &point, &|d| 1.0 - levels.far(d), levels.far[1])
        })
        .collect::<Result<_, String>>()?;
    let air_at = |i: usize, j: usize| {
        let (ai, aj) = ((i / 2).min(air_side - 2), (j / 2).min(air_side - 2));
        let (fx, fy) = ((i - 2 * ai) as f64 / 2.0, (j - 2 * aj) as f64 / 2.0);
        let value = |di: usize, dj: usize| air[(aj + dj) * air_side + ai + di];
        (value(0, 0) * (1.0 - fx) + value(1, 0) * fx) * (1.0 - fy)
            + (value(0, 1) * (1.0 - fx) + value(1, 1) * fx) * fy
    };
    // The mid level at the block corners, over the far level and the aircraft there.
    let ratio = grid.coarse / grid.block;
    let corner_side = n / grid.block + 1;
    let corners: Vec<([f64; LAYERS], Vec<u32>)> = (0..corner_side * corner_side)
        .into_par_iter()
        .map_init(
            || (Work::default(), Vec::new()),
            |(work, list), at| {
                let (i, j) = (at % corner_side, at / corner_side);
                let (ci, cj) = (
                    (i / ratio).min(coarse_side - 2),
                    (j / ratio).min(coarse_side - 2),
                );
                let (fx, fy) = (
                    (i - ci * ratio) as f64 / ratio as f64,
                    (j - cj * ratio) as f64 / ratio as f64,
                );
                let far = bilinear(
                    [
                        &coarse[cj * coarse_side + ci],
                        &coarse[cj * coarse_side + ci + 1],
                        &coarse[(cj + 1) * coarse_side + ci],
                        &coarse[(cj + 1) * coarse_side + ci + 1],
                    ],
                    fx,
                    fy,
                );
                let position = metres((i * grid.block) as f64, (j * grid.block) as f64);
                let point = Point::at(square, position)?;
                square.index.within(position, position, levels.far[1], list);
                let mid = energies(
                    square,
                    &point,
                    &mut list.iter().copied(),
                    &|d| levels.mid(d),
                    &far,
                    &MID_RULE,
                    mix(seed, (1 << 40) | at as u64),
                    work,
                )?;
                let mut rest: [f64; LAYERS] = std::array::from_fn(|l| far[l] + mid[l]);
                let mut named = std::mem::take(&mut work.evaluated);
                named.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
                let least = NAMED_SHARE * rest.iter().sum::<f64>();
                let named: Vec<u32> = named
                    .iter()
                    .take(NAMED_PER_CORNER)
                    .filter(|(energy, _)| *energy > 0.0 && *energy >= least)
                    .map(|&(_, index)| index)
                    .collect();
                rest[Layer::Aircraft as usize] += air_at(i, j);
                Ok((rest, named))
            },
        )
        .collect::<Result<_, String>>()?;

    let corners_s = started.elapsed().as_secs_f64() - coarse_s;
    // Every pixel: the near level evaluated there over the rest interpolated.
    let blocks = n / grid.block;
    let painted: Vec<Cells> = (0..blocks * blocks)
        .into_par_iter()
        .map_init(
            || (Work::default(), Vec::new()),
            |(work, list), at| {
                let (bx, by) = (at % blocks, at / blocks);
                let a = metres((bx * grid.block) as f64, (by * grid.block) as f64);
                let b = metres(
                    ((bx + 1) * grid.block) as f64,
                    ((by + 1) * grid.block) as f64,
                );
                let low = [a[0].min(b[0]), a[1].min(b[1])];
                let high = [a[0].max(b[0]), a[1].max(b[1])];
                square.index.within(low, high, levels.near[1], list);
                // The corners' named sources, evaluated exactly at every pixel, and the corners'
                // rest without their mid-level shares.
                let at = [(0, 0), (1, 0), (0, 1), (1, 1)];
                let mut named: Vec<u32> = at
                    .iter()
                    .flat_map(|&(i, j)| corners[(by + j) * corner_side + bx + i].1.iter().copied())
                    .collect();
                named.sort_unstable();
                named.dedup();
                let mut rests = [[0.0; LAYERS]; 4];
                for (k, &(i, j)) in at.iter().enumerate() {
                    rests[k] = corners[(by + j) * corner_side + bx + i].0;
                    let position = metres(
                        ((bx + i) * grid.block) as f64,
                        ((by + j) * grid.block) as f64,
                    );
                    let point = Point::at(square, position)?;
                    for &index in &named {
                        let layer = square.candidates[index as usize].layer as usize;
                        let share = energy_of(square, &point, index, &|d| levels.mid(d), work)?;
                        rests[k][layer] = (rests[k][layer] - share).max(0.0);
                    }
                }
                let mut cells = Vec::with_capacity(grid.block * grid.block);
                for py in 0..grid.block {
                    for px in 0..grid.block {
                        let (x, y) = (bx * grid.block + px, by * grid.block + py);
                        let position = metres(x as f64 + 0.5, y as f64 + 0.5);
                        if square.obstacles.enclosing_building_id(position)?.is_some() {
                            cells.push([NO_LEVEL; LAYERS + 1]);
                            continue;
                        }
                        let (fx, fy) = (
                            (px as f64 + 0.5) / grid.block as f64,
                            (py as f64 + 0.5) / grid.block as f64,
                        );
                        let mut rest =
                            bilinear([&rests[0], &rests[1], &rests[2], &rests[3]], fx, fy);
                        let (lat, lon) = square.frame.to_mercator(position).to_degrees();
                        let point = Point {
                            position,
                            altitude_m: square.ground.at(position)?.height_m
                                + popup::answer::RECEIVER_HEIGHT_M,
                            weather: square.release.weather.place(lat, lon),
                            own_footprint: 0,
                        };
                        for &index in &named {
                            let layer = square.candidates[index as usize].layer as usize;
                            rest[layer] +=
                                energy_of(square, &point, index, &|d| levels.mid(d), work)?;
                        }
                        let near = energies(
                            square,
                            &point,
                            &mut list.iter().copied(),
                            &|d| levels.near(d),
                            &rest,
                            &NEAR_RULE,
                            mix(seed, (2 << 40) | (y * n + x) as u64),
                            work,
                        )?;
                        let reflection = energy(square.obstacles.reflection_db(position, None)?);
                        let layers: [f64; LAYERS] =
                            std::array::from_fn(|l| (near[l] + rest[l]) * reflection);
                        let mut pixel = [NO_LEVEL; LAYERS + 1];
                        for (l, value) in layers.iter().enumerate() {
                            pixel[l] = cell(*value);
                        }
                        pixel[LAYERS] = cell(layers.iter().sum());
                        cells.push(pixel);
                    }
                }
                Ok(cells)
            },
        )
        .collect::<Result<_, String>>()?;

    eprintln!(
        "qm-paint: coarse {coarse_s:.1} s, corners {corners_s:.1} s, pixels {:.1} s",
        started.elapsed().as_secs_f64() - coarse_s - corners_s
    );
    // Blocks back into rows.
    let mut cells = vec![[NO_LEVEL; LAYERS + 1]; n * n];
    for (at, block) in painted.into_iter().enumerate() {
        let (bx, by) = (at % blocks, at / blocks);
        for (k, pixel) in block.into_iter().enumerate() {
            let (px, py) = (k % grid.block, k / grid.block);
            cells[(by * grid.block + py) * n + bx * grid.block + px] = pixel;
        }
    }
    Ok(cells)
}
