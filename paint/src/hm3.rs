//! The map's heatmap tiles: per layer and the total, 512 x 512 cells of twice the Lden (255 none)
//! behind the header `HM3 `, version 3 and the layer's number, as the map decodes them
//! (`frontend/src/lib/hm3-decoder.ts`). A painted square of zoom-12 is `2^(zoom - 12)` tiles a side.

use crate::exact::LAYERS;
use crate::paint::Cells;
use std::path::{Path, PathBuf};
use tiles::sources::Layer;

pub const TILE_PX: usize = 512;
pub const HEADER: [u8; 5] = *b"HM3 \x03";

/// The tile trees' names: the layers, then the total.
pub fn layer_names() -> [&'static str; LAYERS + 1] {
    std::array::from_fn(|l| {
        if l < LAYERS {
            Layer::ALL[l].name()
        } else {
            "total"
        }
    })
}

pub fn tile_path(out: &Path, layer: &str, zoom: u32, x: u32, y: u32) -> PathBuf {
    out.join(layer)
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{y}.hm3"))
}

/// Writes the square's tiles, each layer's before the total's, every file under a temporary name
/// renamed into place, so a square whose total's last tile exists is painted.
pub fn write(out: &Path, zoom: u32, square: (u32, u32), cells: &Cells) -> Result<(), String> {
    let per_side = 1usize << (zoom - 12);
    let side = per_side * TILE_PX;
    assert_eq!(cells.len(), side * side, "a square's cells");
    for (layer, name) in layer_names().iter().enumerate() {
        for ty in 0..per_side {
            for tx in 0..per_side {
                let mut bytes = Vec::with_capacity(HEADER.len() + 1 + TILE_PX * TILE_PX);
                bytes.extend_from_slice(&HEADER);
                bytes.push(layer as u8);
                for row in 0..TILE_PX {
                    let start = (ty * TILE_PX + row) * side + tx * TILE_PX;
                    bytes.extend(
                        cells[start..start + TILE_PX]
                            .iter()
                            .map(|pixel| pixel[layer]),
                    );
                }
                let (x, y) = (
                    square.0 * per_side as u32 + tx as u32,
                    square.1 * per_side as u32 + ty as u32,
                );
                let path = tile_path(out, name, zoom, x, y);
                let directory = path.parent().expect("a tile's directory");
                std::fs::create_dir_all(directory)
                    .map_err(|e| format!("{}: {e}", directory.display()))?;
                let temporary = path.with_extension("hm3.part");
                std::fs::write(&temporary, &bytes)
                    .map_err(|e| format!("{}: {e}", temporary.display()))?;
                std::fs::rename(&temporary, &path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    Ok(())
}

/// Whether the square's tiles are all written.
pub fn painted(out: &Path, zoom: u32, square: (u32, u32)) -> bool {
    let per_side = 1u32 << (zoom - 12);
    tile_path(
        out,
        "total",
        zoom,
        square.0 * per_side + per_side - 1,
        square.1 * per_side + per_side - 1,
    )
    .exists()
}
