//! Publish immutable, deduplicated PMTiles archives by copying the painter's compressed streams.

use crate::exact::LAYERS;
use crate::hm3::{HEADER, Metric, PaintedSquare, layer_names, squares, squares_within};
use crate::pyramid::{self, MIN_ZOOM};
use crate::region::REGION_ZOOM;
use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

struct ParentTile {
    coordinate: TileCoord,
    ranges: [(u64, usize); LAYERS + 1],
}

/// The map's archives and manifest from the painted squares in `out`: Nden from every square,
/// Lden (`lden-` archives) from the squares that hold it; with `regions`, each zoom-8 region's
/// parts kept there (`pack_region`) are reused while their bundles are the same, else folded and
/// kept.
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
    for metric in [Metric::Nden, Metric::Lden] {
        for layer in layer_names() {
            let path = tiles_dir.join(archive_name(metric, layer, build));
            if path.try_exists().map_err(|e| e.to_string())? {
                return Err(format!("{}: builds are immutable", path.display()));
            }
        }
    }
    let squares = squares(out, u32::from(zoom))?;
    if squares.is_empty() {
        return Err("no complete painted squares".into());
    }
    let lden_squares = squares_with_lden(out, zoom, &squares)?;
    let temporary = tempfile::tempdir_in(tiles_dir).map_err(|e| e.to_string())?;
    let mut layers = serde_json::Map::new();
    for (metric, squares) in [(Metric::Nden, &squares), (Metric::Lden, &lden_squares)] {
        if !squares.is_empty() {
            let archive = Archive {
                out,
                zoom,
                metric,
                build,
                directory: temporary.path(),
            };
            archive.write(squares, regions, &mut layers)?;
        }
    }
    // Every archive is complete before any immutable name appears; a failed pack leaves the
    // previous manifest in place. A hard link installs without replacing an existing archive.
    for entry in layers.values() {
        let name = entry["file"].as_str().expect("an archive's file name");
        std::fs::hard_link(temporary.path().join(name), tiles_dir.join(name))
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
        "qm-paint: packed {} squares, {} with Lden, at zoom {zoom}",
        squares.len(),
        lden_squares.len()
    );
    Ok(())
}

fn archive_name(metric: Metric, layer: &str, build: &str) -> String {
    format!("{}.{build}.pmtiles", metric.archive_layer(layer))
}

/// The squares of `squares` that hold Lden, read from their bundles' headers.
fn squares_with_lden(
    out: &Path,
    zoom: u8,
    squares: &BTreeSet<(u32, u32)>,
) -> Result<BTreeSet<(u32, u32)>, String> {
    let held: Vec<Option<(u32, u32)>> = squares
        .par_iter()
        .map(|&square| {
            Ok(PaintedSquare::open(out, u32::from(zoom), square)?
                .has_lden()
                .then_some(square))
        })
        .collect::<Result<_, String>>()?;
    Ok(held.into_iter().flatten().collect())
}

/// One metric's seven archives of a build, written into `directory` before they are installed.
struct Archive<'a> {
    out: &'a Path,
    zoom: u8,
    metric: Metric,
    build: &'a str,
    directory: &'a Path,
}

impl Archive<'_> {
    /// Each layer's archive over `squares` (which all hold the metric), named in `layers`.
    fn write(
        &self,
        squares: &BTreeSet<(u32, u32)>,
        regions: Option<&Path>,
        layers: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let (out, zoom, metric, build) = (self.out, self.zoom, self.metric, self.build);
        let mut parents_file = tempfile::tempfile_in(self.directory).map_err(|e| e.to_string())?;
        let mut parents = Vec::new();
        pyramid::write(out, zoom, metric, squares, regions, |coordinate, tiles| {
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
        for &(sx, sy) in squares {
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
        for (number, layer) in layer_names().iter().enumerate() {
            let name = archive_name(metric, layer, build);
            let path = self.directory.join(&name);
            let file = File::create_new(&path).map_err(|e| e.to_string())?;
            let archive_layer = metric.archive_layer(layer);
            let mut writer = PmTilesWriter::new(TileType::Unknown)
                .tile_compression(Compression::Brotli)
                .internal_compression(Compression::Gzip)
                .min_zoom(MIN_ZOOM)
                .max_zoom(zoom)
                .bounds(-180.0, -85.051_13, 180.0, 85.051_13)
                .metadata(&serde_json::json!({ "name": archive_layer, "build": build }).to_string())
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
                let bytes = stored.compressed_tile(
                    metric,
                    number,
                    (x % per_side) as usize,
                    (y % per_side) as usize,
                )?;
                writer
                    .add_raw_tile(coordinate, &bytes)
                    .map_err(|e| e.to_string())?;
            }
            writer.finalize().map_err(|e| e.to_string())?;
            File::open(&path)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
            layers.insert(
                archive_layer,
                serde_json::json!({ "file": name, "build": build }),
            );
        }
        Ok(())
    }
}

/// Folds and keeps the zoom-8 region `(x, y)` of the painted squares in `out` for the pack: each
/// metric's tiles from zoom 12 to 8 and its means. The squares it held.
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
        let lden = squares_with_lden(out, zoom, &squares)?;
        pyramid::keep_region(out, zoom, Metric::Nden, squares, regions, (x, y))?;
        if !lden.is_empty() {
            pyramid::keep_region(out, zoom, Metric::Lden, lden, regions, (x, y))?;
        }
    }
    Ok(count)
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
