//! The painted tiles published for the map: per layer and the total one PMTiles archive
//! (`<layer>.<build>.pmtiles`, Brotli tiles, as the server ships them) holding the painted zoom
//! and every zoom down to 2, and `current.json` naming the build and zoom. Only squares painted
//! whole are packed. A parent cell holds each layer's energy mean over the painted pixels under it
//! that have a total level (a building's, without one, are left out; a layer without a level there
//! is silent), carried unrounded from zoom to zoom, and the total is the sum of its layers, so
//! switching a silent layer off changes nothing.

use crate::hm3::{HEADER, TILE_PX, layer_names, painted, tile_path};
use crate::levels::LAYERS;
use crate::paint::NO_LEVEL;
use pmtiles::{Compression, PmTilesWriter, TileCoord, TileType};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The lowest zoom published.
const MIN_ZOOM: u8 = 2;
/// Brotli's quality for the tiles: 9 of 11 packs a Prague tile within a few percent of 11 in a
/// fraction of the time.
const BROTLI_QUALITY: u32 = 9;

type Tiles = BTreeMap<(u32, u32), Vec<u8>>;

/// A parent tile's cells: how many painted pixels with a total level each stands for, and per
/// layer their mean energy.
struct Means {
    weight: Vec<u32>,
    energy: Vec<[f32; LAYERS]>,
}

type Level = BTreeMap<(u32, u32), Means>;

/// The cells of layer `number`'s tiles `wanted` at `zoom`, each tile's header checked.
fn read(
    out: &Path,
    number: usize,
    zoom: u8,
    wanted: &BTreeSet<(u32, u32)>,
) -> Result<Tiles, String> {
    let name = layer_names()[number];
    wanted
        .par_iter()
        .map(|&(x, y)| {
            let path = tile_path(out, name, u32::from(zoom), x, y);
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if bytes.len() != HEADER.len() + 1 + TILE_PX * TILE_PX
                || bytes[..HEADER.len()] != HEADER
                || usize::from(bytes[HEADER.len()]) != number
            {
                return Err(format!("{}: not this layer's HM3 tile", path.display()));
            }
            Ok(((x, y), bytes[HEADER.len() + 1..].to_vec()))
        })
        .collect()
}

/// The cell of a parent tile child `(x, y)` fills at `(row, column)` of its quarter, and the
/// child's four cells under it.
fn children_of(x: u32, y: u32, row: usize, column: usize) -> (usize, [usize; 4]) {
    let (qx, qy) = (
        (x % 2) as usize * TILE_PX / 2,
        (y % 2) as usize * TILE_PX / 2,
    );
    let at = 2 * row * TILE_PX + 2 * column;
    (
        (qy + row) * TILE_PX + qx + column,
        [at, at + 1, at + TILE_PX, at + TILE_PX + 1],
    )
}

/// The parents of children given by `child(tile, cell) -> (weight, energies)`.
fn parents(
    tiles: &BTreeSet<(u32, u32)>,
    child: &(dyn Fn((u32, u32), usize) -> (u32, [f32; LAYERS]) + Sync),
) -> Level {
    let mut parents = Level::new();
    for &(x, y) in tiles {
        let parent = parents.entry((x / 2, y / 2)).or_insert_with(|| Means {
            weight: vec![0; TILE_PX * TILE_PX],
            energy: vec![[0.0; LAYERS]; TILE_PX * TILE_PX],
        });
        for row in 0..TILE_PX / 2 {
            for column in 0..TILE_PX / 2 {
                let (to, four) = children_of(x, y, row, column);
                let (mut weight, mut sum) = (0u32, [0.0f64; LAYERS]);
                for cell in four {
                    let (w, energy) = child((x, y), cell);
                    weight += w;
                    for (total, value) in sum.iter_mut().zip(energy) {
                        *total += f64::from(w) * f64::from(value);
                    }
                }
                parent.weight[to] = weight;
                parent.energy[to] =
                    std::array::from_fn(|l| (sum[l] / f64::from(weight.max(1))) as f32);
            }
        }
    }
    parents
}

/// A cell's byte of an energy: twice its level, or none under 0 dB.
fn encode(energy: f64) -> u8 {
    if energy < 1.0 {
        NO_LEVEL
    } else {
        (20.0 * energy.log10()).round().min(254.0) as u8
    }
}

/// Layer `number`'s (the total past the layers) cells of a parent tile.
fn cells(means: &Means, number: usize) -> Vec<u8> {
    means
        .weight
        .iter()
        .zip(&means.energy)
        .map(|(&weight, energy)| {
            if weight == 0 {
                NO_LEVEL
            } else if number < LAYERS {
                encode(f64::from(energy[number]))
            } else {
                encode(energy.iter().map(|&e| f64::from(e)).sum())
            }
        })
        .collect()
}

/// The squares painted whole (their total's last tile written) and their tiles at `zoom`.
fn whole(out: &Path, zoom: u8) -> Result<(usize, BTreeSet<(u32, u32)>), String> {
    let per_side = 1u32 << (zoom - 12);
    let root = out.join("total").join(zoom.to_string());
    let listed = |path: &Path| {
        std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, String>>()
    };
    let mut squares = BTreeSet::new();
    for column in listed(&root)? {
        let x: Option<u32> = column.file_name().and_then(|n| n.to_str()?.parse().ok());
        for file in listed(&column)? {
            let y: Option<u32> = file
                .file_name()
                .and_then(|n| n.to_str()?.strip_suffix(".hm3")?.parse().ok());
            if let (Some(x), Some(y)) = (x, y) {
                let square = (x / per_side, y / per_side);
                if painted(out, u32::from(zoom), square) {
                    squares.insert(square);
                }
            }
        }
    }
    let tiles = squares
        .iter()
        .flat_map(|&(sx, sy)| {
            (0..per_side * per_side)
                .map(move |k| (sx * per_side + k % per_side, sy * per_side + k / per_side))
        })
        .collect();
    Ok((squares.len(), tiles))
}

/// Writes every layer's archive into `tiles_dir` and the manifest naming them.
pub fn pack(out: &Path, zoom: u8, tiles_dir: &Path, build: &str) -> Result<(), String> {
    std::fs::create_dir_all(tiles_dir).map_err(|e| format!("{}: {e}", tiles_dir.display()))?;
    let (squares, wanted) = whole(out, zoom)?;
    // Every layer's painted cells, then the parents unrounded, zoom by zoom.
    let mut painted: Vec<Tiles> = (0..=LAYERS)
        .map(|number| read(out, number, zoom, &wanted))
        .collect::<Result<_, String>>()?;
    let energy: [f32; 256] = std::array::from_fn(|cell| match cell as u8 {
        NO_LEVEL => 0.0,
        level => 10f64.powf(f64::from(level) / 20.0) as f32,
    });
    let mut levels: Vec<Level> = vec![parents(&wanted, &|tile, cell| {
        if painted[LAYERS][&tile][cell] == NO_LEVEL {
            return (0, [0.0; LAYERS]);
        }
        (
            1,
            std::array::from_fn(|l| energy[usize::from(painted[l][&tile][cell])]),
        )
    })];
    for _ in MIN_ZOOM + 1..zoom {
        let below = levels.last().expect("the first parents");
        let tiles: BTreeSet<(u32, u32)> = below.keys().copied().collect();
        let next = parents(&tiles, &|tile, cell| {
            let means = &below[&tile];
            (means.weight[cell], means.energy[cell])
        });
        levels.push(next);
    }
    let mut layers = serde_json::Map::new();
    for (number, layer) in layer_names().iter().enumerate() {
        // The painted zoom as painted, the parents encoded from their means.
        let mut tiles: Vec<Tiles> = vec![std::mem::take(&mut painted[number])];
        for level in &levels {
            tiles.push(
                level
                    .iter()
                    .map(|(&at, means)| (at, cells(means, number)))
                    .collect(),
            );
        }
        let levels = tiles;
        let name = format!("{layer}.{build}.pmtiles");
        let path = tiles_dir.join(&name);
        let file =
            std::fs::File::create_new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut writer = PmTilesWriter::new(TileType::Unknown)
            .tile_compression(Compression::Brotli)
            .internal_compression(Compression::Gzip)
            .min_zoom(MIN_ZOOM)
            .max_zoom(zoom)
            .bounds(-180.0, -85.051_13, 180.0, 85.051_13)
            .metadata(&serde_json::json!({ "name": layer, "build": build }).to_string())
            .create(std::io::BufWriter::new(file))
            .map_err(|e| e.to_string())?;
        // Tiles in the order of their ids: by zoom, then along the Hilbert curve.
        let mut all: Vec<(u64, u8, u32, u32)> = Vec::new();
        for (index, level) in levels.iter().enumerate() {
            let z = zoom - index as u8;
            for &(x, y) in level.keys() {
                let coord = TileCoord::new(z, x, y).map_err(|e| e.to_string())?;
                all.push((pmtiles::TileId::from(coord).value(), z, x, y));
            }
        }
        all.sort_unstable();
        // Compressed in parallel, written in order.
        let compressed: Vec<Vec<u8>> = all
            .par_iter()
            .map(|&(_, z, x, y)| {
                let mut tile = Vec::with_capacity(HEADER.len() + 1 + TILE_PX * TILE_PX);
                tile.extend_from_slice(&HEADER);
                tile.push(number as u8);
                tile.extend_from_slice(&levels[usize::from(zoom - z)][&(x, y)]);
                let mut out = Vec::new();
                let mut encoder =
                    brotli::CompressorWriter::new(&mut out, 1 << 16, BROTLI_QUALITY, 22);
                encoder.write_all(&tile).expect("compressing in memory");
                drop(encoder);
                out
            })
            .collect();
        for ((_, z, x, y), bytes) in all.into_iter().zip(compressed) {
            writer
                .add_raw_tile(TileCoord::new(z, x, y).map_err(|e| e.to_string())?, &bytes)
                .map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        layers.insert(
            layer.to_string(),
            serde_json::json!({ "file": name, "build": build }),
        );
    }
    let manifest = serde_json::json!({ "build": build, "zoom": zoom, "layers": layers });
    let path = tiles_dir.join("current.json");
    let temporary = tiles_dir.join("current.json.part");
    std::fs::write(&temporary, manifest.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!(
        "qm-paint: packed {squares} squares, {} tiles a layer at zoom {zoom}",
        wanted.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One outdoor pixel at 60 dB and four at 40 dB two zooms below read their energy mean, not
    /// the mean of their parents; a layer silent under a parent stays out of its total; a
    /// building's pixel counts nowhere.
    #[test]
    fn parents_average_the_painted_pixels_under_them() {
        let energy = |db: f64| 10f64.powf(db / 10.0) as f32;
        // Tile (4, 4): pixel 0 60 dB of road, pixel 1 inside a building; pixels 2, 3 and the two
        // below them 40 dB of railway (the next parent cell).
        let pixel = |tile: (u32, u32), cell: usize| -> (u32, [f32; LAYERS]) {
            let mut layers = [0.0; LAYERS];
            match (tile, cell) {
                ((4, 4), 0) => layers[0] = energy(60.0),
                ((4, 4), c) if [2, 3, TILE_PX + 2, TILE_PX + 3].contains(&c) => {
                    layers[1] = energy(40.0)
                }
                _ => return (0, layers),
            }
            (1, layers)
        };
        let tiles: BTreeSet<(u32, u32)> = [(4, 4)].into();
        let first = parents(&tiles, &pixel);
        let parent = &first[&(2, 2)];
        assert_eq!((parent.weight[0], parent.weight[1]), (1, 4));
        let second = parents(&[(2, 2)].into(), &|tile, cell| {
            (first[&tile].weight[cell], first[&tile].energy[cell])
        });
        let top = &second[&(1, 1)];
        assert_eq!(top.weight[0], 5);
        let mean = (10f64.powf(6.0) + 4.0 * 10f64.powf(4.0)) / 5.0;
        assert_eq!(cells(top, LAYERS)[0], encode(mean));
        assert_eq!(cells(top, 0)[0], encode(10f64.powf(6.0) / 5.0));
        assert_eq!(cells(top, 2)[0], NO_LEVEL);
    }
}
