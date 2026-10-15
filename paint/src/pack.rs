//! Publish immutable, deduplicated PMTiles archives by copying the painter's compressed streams.

use crate::exact::LAYERS;
use crate::hm3::{HEADER, PaintedSquare, layer_names, squares, squares_within};
use crate::pyramid::{self, MIN_ZOOM};
use crate::region::REGION_ZOOM;
use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

struct ParentTile {
    coordinate: TileCoord,
    ranges: [(u64, usize); LAYERS + 1],
}

/// The map's archives and manifest from the painted squares in `out`; with `regions`, each zoom-8
/// region's part kept there (`pack_region`) is reused while its bundles are the same, else folded
/// and kept.
pub fn pack(
    out: &Path,
    zoom: u8,
    tiles_dir: &Path,
    build: &str,
    regions: Option<&Path>,
) -> Result<(), String> {
    if !matches!(zoom, 12 | 13)
        || !build
            .strip_prefix('b')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("pack requires zoom 12 or 13 and build b<N>".into());
    }
    std::fs::create_dir_all(tiles_dir).map_err(|e| e.to_string())?;
    // Keep the lock inode: removing it could let two publishers lock different files.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(tiles_dir.join(".pack.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    for layer in layer_names() {
        let path = tiles_dir.join(format!("{layer}.{build}.pmtiles"));
        if path.try_exists().map_err(|e| e.to_string())? {
            return Err(format!("{}: builds are immutable", path.display()));
        }
    }
    let squares = squares(out, u32::from(zoom))?;
    if squares.is_empty() {
        return Err("no complete painted squares".into());
    }
    let temporary = tempfile::tempdir_in(tiles_dir).map_err(|e| e.to_string())?;
    let mut parents_file = tempfile::tempfile_in(temporary.path()).map_err(|e| e.to_string())?;
    let mut parents = Vec::new();
    pyramid::write(out, zoom, &squares, regions, |coordinate, tiles| {
        let mut ranges = [(0, 0); LAYERS + 1];
        for (layer, bytes) in tiles.iter().enumerate() {
            let offset = parents_file.stream_position().map_err(|e| e.to_string())?;
            parents_file.write_all(bytes).map_err(|e| e.to_string())?;
            ranges[layer] = (offset, bytes.len());
        }
        parents.push(ParentTile { coordinate, ranges });
        Ok(())
    })?;
    parents.sort_unstable_by_key(|tile| TileId::from(tile.coordinate).value());
    let per_side = 1u32 << (zoom - 12);
    let mut painted = Vec::with_capacity(squares.len() * (per_side * per_side) as usize);
    for &(sx, sy) in &squares {
        for ty in 0..per_side {
            for tx in 0..per_side {
                painted.push(
                    TileCoord::new(zoom, sx * per_side + tx, sy * per_side + ty)
                        .map_err(|e| e.to_string())?,
                );
            }
        }
    }
    painted.sort_unstable_by_key(|&coordinate| TileId::from(coordinate).value());
    let mut layers = serde_json::Map::new();
    for (number, layer) in layer_names().iter().enumerate() {
        let name = format!("{layer}.{build}.pmtiles");
        let path = temporary.path().join(&name);
        let file = File::create_new(&path).map_err(|e| e.to_string())?;
        let mut writer = PmTilesWriter::new(TileType::Unknown)
            .tile_compression(Compression::Brotli)
            .internal_compression(Compression::Gzip)
            .min_zoom(MIN_ZOOM)
            .max_zoom(zoom)
            .bounds(-180.0, -85.051_13, 180.0, 85.051_13)
            .metadata(&serde_json::json!({ "name": layer, "build": build }).to_string())
            .create(file)
            .map_err(|e| e.to_string())?;
        for tile in &parents {
            let (offset, length) = tile.ranges[number];
            let mut bytes = vec![0; length];
            parents_file
                .read_exact_at(&mut bytes, offset)
                .map_err(|e| e.to_string())?;
            writer
                .add_raw_tile(tile.coordinate, &bytes)
                .map_err(|e| e.to_string())?;
        }
        // A square's children are consecutive in Hilbert order, so one handle reads them all.
        let mut opened: Option<((u32, u32), PaintedSquare)> = None;
        for &coordinate in &painted {
            let (x, y) = (coordinate.x(), coordinate.y());
            let square = (x / per_side, y / per_side);
            if opened.as_ref().map(|(at, _)| *at) != Some(square) {
                opened = Some((square, PaintedSquare::open(out, u32::from(zoom), square)?));
            }
            let stored = &opened.as_ref().expect("opened square").1;
            let bytes =
                stored.compressed_tile(number, (x % per_side) as usize, (y % per_side) as usize)?;
            writer
                .add_raw_tile(coordinate, &bytes)
                .map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        File::open(&path)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        layers.insert(
            layer.to_string(),
            serde_json::json!({ "file": name, "build": build }),
        );
    }
    // Every archive is complete before any immutable name appears; a failed pack leaves the
    // previous manifest in place. A hard link installs without replacing an existing archive.
    for layer in layer_names() {
        let name = format!("{layer}.{build}.pmtiles");
        std::fs::hard_link(temporary.path().join(&name), tiles_dir.join(&name))
            .map_err(|e| e.to_string())?;
    }
    File::open(tiles_dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    let manifest = serde_json::json!({ "build": build, "zoom": zoom, "hm3_version": HEADER[4], "layers": layers });
    let path = temporary.path().join("current.json");
    std::fs::write(&path, manifest.to_string()).map_err(|e| e.to_string())?;
    File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&path, tiles_dir.join("current.json")).map_err(|e| e.to_string())?;
    File::open(tiles_dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    eprintln!(
        "qm-paint: packed {} squares, {} tiles a layer at zoom {zoom}",
        squares.len(),
        painted.len()
    );
    Ok(())
}

/// Folds and keeps the zoom-8 region `(x, y)` of the painted squares in `out` for the pack: its
/// tiles from zoom 12 to 8 and its means. The squares it held.
pub fn pack_region(
    out: &Path,
    zoom: u8,
    regions: &Path,
    (x, y): (u32, u32),
) -> Result<usize, String> {
    let span = 1 << (12 - REGION_ZOOM);
    let squares = squares_within(
        out,
        u32::from(zoom),
        x * span..(x + 1) * span,
        y * span..(y + 1) * span,
    )?;
    let count = squares.len();
    if count > 0 {
        pyramid::keep_region(out, zoom, squares, regions, (x, y))?;
    }
    Ok(count)
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
