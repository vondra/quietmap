//! Fold painted cells into the same assessed-pixel loudness means with memory bounded by depth.

use crate::exact::LAYERS;
use crate::hm3::{TILE_PX, encode_tile, read};
use crate::paint::{Cells, NO_LEVEL, SILENCE, cell};
use pmtiles::TileCoord;
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) const MIN_ZOOM: u8 = 2;

struct AssessedLoudnessMeans {
    assessed_pixel_counts: Vec<u32>,
    mean_layer_loudness_sone: Vec<[f32; LAYERS + 1]>,
}

fn cell_byte_loudness_sone() -> [f32; 256] {
    std::array::from_fn(|byte| match byte as u8 {
        SILENCE | NO_LEVEL => 0.0,
        level => 2f64.powf((f64::from(level) - 80.0) / 20.0) as f32,
    })
}

fn assessed_loudness_mean(
    children: impl Iterator<Item = (u32, [f32; LAYERS + 1])>,
) -> (u32, [f32; LAYERS + 1]) {
    let (mut weight, mut sum) = (0u32, [0.0f64; LAYERS + 1]);
    for (count, loudness) in children {
        weight += count;
        for (total, value) in sum.iter_mut().zip(loudness) {
            *total += f64::from(count) * f64::from(value);
        }
    }
    // The f32 mean at each fold, and the four children's order, preserve the published cells.
    (
        weight,
        std::array::from_fn(|layer| (sum[layer] / f64::from(weight.max(1))) as f32),
    )
}

impl AssessedLoudnessMeans {
    fn empty() -> Self {
        Self {
            assessed_pixel_counts: vec![0; TILE_PX * TILE_PX],
            mean_layer_loudness_sone: vec![[0.0; LAYERS + 1]; TILE_PX * TILE_PX],
        }
    }

    fn from_painted(cells: &Cells, zoom: u8, loudness: &[f32; 256]) -> Self {
        let step = 1usize << (zoom - 12);
        let side = step * TILE_PX;
        let mut means = Self::empty();
        for row in 0..TILE_PX {
            for column in 0..TILE_PX {
                let destination = row * TILE_PX + column;
                let (weight, value) = assessed_loudness_mean((0..step * step).map(|child| {
                    let pixel =
                        cells[(step * row + child / step) * side + step * column + child % step];
                    if pixel[LAYERS] == NO_LEVEL {
                        (0, [0.0; LAYERS + 1])
                    } else {
                        (
                            1,
                            std::array::from_fn(|layer| loudness[pixel[layer] as usize]),
                        )
                    }
                }));
                means.assessed_pixel_counts[destination] = weight;
                means.mean_layer_loudness_sone[destination] = value;
            }
        }
        means
    }

    fn place_child(&mut self, x: u32, y: u32, child: &Self) {
        let half = TILE_PX / 2;
        for row in 0..half {
            for column in 0..half {
                let at = 2 * row * TILE_PX + 2 * column;
                let destination = (y as usize * half + row) * TILE_PX + x as usize * half + column;
                let (weight, value) = assessed_loudness_mean(
                    [at, at + 1, at + TILE_PX, at + TILE_PX + 1]
                        .into_iter()
                        .map(|at| {
                            (
                                child.assessed_pixel_counts[at],
                                child.mean_layer_loudness_sone[at],
                            )
                        }),
                );
                self.assessed_pixel_counts[destination] = weight;
                self.mean_layer_loudness_sone[destination] = value;
            }
        }
    }

    fn layer_cells(&self, layer: usize) -> Vec<u8> {
        self.assessed_pixel_counts
            .iter()
            .zip(&self.mean_layer_loudness_sone)
            .map(|(&weight, loudness)| {
                if weight == 0 {
                    NO_LEVEL
                } else {
                    cell(f64::from(loudness[layer]))
                }
            })
            .collect()
    }
}

struct Pyramid<'a> {
    out: &'a Path,
    painted_zoom: u8,
    covered_tile_coordinates_by_zoom: Vec<BTreeSet<(u32, u32)>>,
    loudness: [f32; 256],
}

impl Pyramid<'_> {
    fn visit(
        &self,
        zoom: u8,
        x: u32,
        y: u32,
        write: &mut impl FnMut(TileCoord, &[Vec<u8>]) -> Result<(), String>,
    ) -> Result<AssessedLoudnessMeans, String> {
        let means = if zoom == 12 {
            AssessedLoudnessMeans::from_painted(
                &read(self.out, u32::from(self.painted_zoom), (x, y))?,
                self.painted_zoom,
                &self.loudness,
            )
        } else {
            let mut means = AssessedLoudnessMeans::empty();
            for (qx, qy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (child_x, child_y) = (2 * x + qx, 2 * y + qy);
                if self.covered_tile_coordinates_by_zoom[zoom as usize + 1]
                    .contains(&(child_x, child_y))
                {
                    let child = self.visit(zoom + 1, child_x, child_y, write)?;
                    means.place_child(qx, qy, &child);
                }
            }
            means
        };
        if zoom < self.painted_zoom {
            let compressed: Vec<Vec<u8>> = (0..=LAYERS)
                .into_par_iter()
                .map(|layer| encode_tile(&means.layer_cells(layer), layer))
                .collect::<Result<_, _>>()?;
            write(
                TileCoord::new(zoom, x, y).map_err(|e| e.to_string())?,
                &compressed,
            )?;
        }
        Ok(means)
    }
}

pub(crate) fn write(
    out: &Path,
    zoom: u8,
    squares: &BTreeSet<(u32, u32)>,
    mut write: impl FnMut(TileCoord, &[Vec<u8>]) -> Result<(), String>,
) -> Result<(), String> {
    let mut covered_tile_coordinates_by_zoom = vec![BTreeSet::new(); 13];
    covered_tile_coordinates_by_zoom[12] = squares.clone();
    for level in (MIN_ZOOM as usize..12).rev() {
        covered_tile_coordinates_by_zoom[level] = covered_tile_coordinates_by_zoom[level + 1]
            .iter()
            .map(|&(x, y)| (x / 2, y / 2))
            .collect();
    }
    let pyramid = Pyramid {
        out,
        painted_zoom: zoom,
        covered_tile_coordinates_by_zoom,
        loudness: cell_byte_loudness_sone(),
    };
    for &(x, y) in &pyramid.covered_tile_coordinates_by_zoom[MIN_ZOOM as usize] {
        pyramid.visit(MIN_ZOOM, x, y, &mut write)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents_average_the_painted_pixels_under_them() {
        let (road, railway, total) = (0, 1, LAYERS);
        let mut pixels = vec![[NO_LEVEL; LAYERS + 1]; TILE_PX * TILE_PX];
        pixels[0] = [SILENCE; LAYERS + 1];
        pixels[0][road] = cell(16.0);
        pixels[0][total] = cell(16.0);
        for at in [2, 3, TILE_PX + 2, TILE_PX + 3] {
            pixels[at] = [SILENCE; LAYERS + 1];
            pixels[at][railway] = cell(2.0);
            pixels[at][total] = cell(2.0);
        }
        let painted = AssessedLoudnessMeans::from_painted(&pixels, 12, &cell_byte_loudness_sone());
        let mut first = AssessedLoudnessMeans::empty();
        first.place_child(0, 0, &painted);
        assert_eq!(
            (
                first.assessed_pixel_counts[0],
                first.assessed_pixel_counts[1]
            ),
            (1, 4)
        );
        let mut second = AssessedLoudnessMeans::empty();
        second.place_child(0, 0, &first);
        assert_eq!(second.assessed_pixel_counts[0], 5);
        assert_eq!(second.layer_cells(total)[0], cell((16.0 + 4.0 * 2.0) / 5.0));
        assert_eq!(second.layer_cells(road)[0], cell(16.0 / 5.0));
        assert_eq!(second.layer_cells(2)[0], SILENCE);
        assert_eq!(second.layer_cells(total)[1], NO_LEVEL);
    }
}
