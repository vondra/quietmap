//! The painted tiles published for the map: per layer and the total one PMTiles archive
//! (`<layer>.<build>.pmtiles`, Brotli tiles, as the server ships them) holding the painted zoom
//! and every zoom down to 2, and `current.json` naming the build and zoom. Only squares painted
//! whole are packed. A parent cell is the energy mean of its children that have a total level
//! (a building's cells, without one, are left out), a layer's cell without a level counting as
//! silence there, so the layers of a parent sum to its total as they do below.

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

/// The parents of `children` (one layer's cells) over the total's children `totals`.
fn parents(children: &Tiles, totals: &Tiles) -> Tiles {
    let energy: [f64; 256] = std::array::from_fn(|cell| match cell as u8 {
        NO_LEVEL => 0.0,
        level => 10f64.powf(f64::from(level) / 20.0),
    });
    let mut parents = Tiles::new();
    for (&(x, y), cells) in children {
        let total = &totals[&(x, y)];
        let parent = parents
            .entry((x / 2, y / 2))
            .or_insert_with(|| vec![NO_LEVEL; TILE_PX * TILE_PX]);
        let (qx, qy) = (
            (x % 2) as usize * TILE_PX / 2,
            (y % 2) as usize * TILE_PX / 2,
        );
        for row in 0..TILE_PX / 2 {
            for column in 0..TILE_PX / 2 {
                let at = 2 * row * TILE_PX + 2 * column;
                let (mut sum, mut count) = (0.0, 0u32);
                for child in [at, at + 1, at + TILE_PX, at + TILE_PX + 1] {
                    if total[child] != NO_LEVEL {
                        sum += energy[usize::from(cells[child])];
                        count += 1;
                    }
                }
                let mean = sum / f64::from(count.max(1));
                parent[(qy + row) * TILE_PX + qx + column] = if mean < 1.0 {
                    NO_LEVEL
                } else {
                    (20.0 * mean.log10()).round().min(254.0) as u8
                };
            }
        }
    }
    parents
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
    // The total's pyramid first: every layer's parents count the cells it has a level in.
    let mut totals = vec![read(out, LAYERS, zoom, &wanted)?];
    for _ in MIN_ZOOM..zoom {
        let last = totals.last().expect("the painted zoom");
        let next = parents(last, last);
        totals.push(next);
    }
    let mut layers = serde_json::Map::new();
    for (number, layer) in layer_names().iter().enumerate() {
        let levels = if number == LAYERS {
            std::mem::take(&mut totals)
        } else {
            let mut levels = vec![read(out, number, zoom, &wanted)?];
            for below in 0..usize::from(zoom - MIN_ZOOM) {
                let next = parents(&levels[below], &totals[below]);
                levels.push(next);
            }
            levels
        };
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

    /// A parent cell is the energy mean of its children with a total level, a layer's cell
    /// without a level silent there; four children without a total level leave it without one;
    /// a child tile fills its own quarter.
    #[test]
    fn a_parent_averages_its_childrens_energy() {
        let (mut layer, mut total) = (Tiles::new(), Tiles::new());
        let mut cells = vec![NO_LEVEL; TILE_PX * TILE_PX];
        let mut sums = vec![NO_LEVEL; TILE_PX * TILE_PX];
        // 60 dB and a silent cell under one parent cell, both outdoors; two cells indoors.
        cells[0] = 120;
        sums[0] = 120;
        sums[1] = 100;
        layer.insert((5, 7), cells);
        total.insert((5, 7), sums);
        let parent = &parents(&layer, &total)[&(2, 3)];
        let quarter = (TILE_PX / 2) * TILE_PX + TILE_PX / 2;
        let mean: f64 = 10f64.powf(6.0) / 2.0;
        assert_eq!(parent[quarter], (20.0 * mean.log10()).round() as u8);
        assert_eq!(parent[quarter + 1], NO_LEVEL);
        assert_eq!(parent[0], NO_LEVEL);
    }
}
