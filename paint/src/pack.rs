//! The painted tiles published for the map: per layer and the total one PMTiles archive
//! (`<layer>.<build>.pmtiles`, Brotli tiles, as the server ships them) holding the painted zoom
//! and every zoom down to 2, a parent cell the energy mean of its children with a level (cells
//! without one left out, none if all four are), and `current.json` naming the build and zoom.

use crate::hm3::{TILE_PX, layer_names};
use crate::paint::NO_LEVEL;
use pmtiles::{Compression, PmTilesWriter, TileCoord, TileType};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// The lowest zoom published.
const MIN_ZOOM: u8 = 2;
/// Brotli's quality for the tiles: 9 of 11 packs a Prague tile within a few percent of 11 in a
/// fraction of the time.
const BROTLI_QUALITY: u32 = 9;
const HEADER_BYTES: usize = 6;

type Tiles = BTreeMap<(u32, u32), Vec<u8>>;

/// Every painted tile of one layer at `zoom`: its cells by (x, y).
fn painted(out: &Path, layer: &str, zoom: u8) -> Result<Tiles, String> {
    let mut tiles = Tiles::new();
    let root = out.join(layer).join(zoom.to_string());
    let read_dir =
        |path: &Path| std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()));
    for column in read_dir(&root)? {
        let column = column.map_err(|e| e.to_string())?.path();
        let x: u32 = column
            .file_name()
            .and_then(|name| name.to_str()?.parse().ok())
            .ok_or_else(|| format!("{}: not a column", column.display()))?;
        for file in read_dir(&column)? {
            let file = file.map_err(|e| e.to_string())?.path();
            let Some(y) = file
                .file_name()
                .and_then(|name| name.to_str()?.strip_suffix(".hm3")?.parse::<u32>().ok())
            else {
                continue;
            };
            let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            if bytes.len() != HEADER_BYTES + TILE_PX * TILE_PX || &bytes[..4] != b"HM3 " {
                return Err(format!("{}: not an HM3 tile", file.display()));
            }
            tiles.insert((x, y), bytes[HEADER_BYTES..].to_vec());
        }
    }
    Ok(tiles)
}

/// The parents of `children`: each parent cell the energy mean of its four children's cells with
/// a level.
fn parents(children: &Tiles) -> Tiles {
    let energy: [f64; 256] = std::array::from_fn(|cell| 10f64.powf(cell as f64 / 20.0));
    let mut parents = Tiles::new();
    for (&(x, y), cells) in children {
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
                let four = [at, at + 1, at + TILE_PX, at + TILE_PX + 1].map(|i| cells[i]);
                let level: Vec<f64> = four
                    .iter()
                    .filter(|&&cell| cell != NO_LEVEL)
                    .map(|&cell| energy[usize::from(cell)])
                    .collect();
                parent[(qy + row) * TILE_PX + qx + column] = if level.is_empty() {
                    NO_LEVEL
                } else {
                    let mean = level.iter().sum::<f64>() / level.len() as f64;
                    (20.0 * mean.log10()).round().clamp(0.0, 254.0) as u8
                };
            }
        }
    }
    parents
}

/// Writes every layer's archive into `tiles_dir` and the manifest naming them.
pub fn pack(out: &Path, zoom: u8, tiles_dir: &Path, build: &str) -> Result<(), String> {
    std::fs::create_dir_all(tiles_dir).map_err(|e| format!("{}: {e}", tiles_dir.display()))?;
    let mut layers = serde_json::Map::new();
    for (number, layer) in layer_names().iter().enumerate() {
        let mut levels = vec![painted(out, layer, zoom)?];
        for _ in MIN_ZOOM..zoom {
            let next = parents(levels.last().expect("the painted zoom"));
            levels.push(next);
        }
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
                let mut tile = Vec::with_capacity(HEADER_BYTES + TILE_PX * TILE_PX);
                tile.extend_from_slice(b"HM3 \x03");
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
    std::fs::rename(&temporary, &path).map_err(|e| format!("{}: {e}", path.display()))
}
