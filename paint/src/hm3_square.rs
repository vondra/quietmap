//! Atomic square bundles: existing Nden-only v7 and paired Nden/Lden v8, with explicit verification.

use crate::exact::LAYERS;
use crate::hm3::{
    Metric, TILE_PX, decode_metric_tile, encode_metric_tile, square_path, tiles_per_side,
};
use crate::levels::PaintedCells;
use crate::paint::{Cells, NO_LEVEL, SILENCE};
use rayon::prelude::*;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::Path;

pub(crate) const SQUARE_HEADER_BYTES: usize = 14;
const NDEN_ONLY_SQUARE_VERSION: u8 = 7;
const SQUARE_VERSION: u8 = 8;

/// A complete square: HM3S, version, zoom, square x/y (u32 little-endian), then (offset, length)
/// u32 pairs in metric, layer, tile-row, tile-column order. Version 7 has only Nden; version 8
/// adds Lden. Offsets start at the file's beginning; identical streams share a range.
/// Nden streams are the bytes served from PMTiles. Old completed squares remain readable.
pub struct PaintedSquare {
    file: std::fs::File,
    entries: Vec<(u32, u32)>,
    per_side: usize,
    has_lden: bool,
}

impl PaintedSquare {
    pub fn open(out: &Path, zoom: u32, square: (u32, u32)) -> Result<Self, String> {
        let per_side = tiles_per_side(zoom)?;
        let path = square_path(out, zoom, square);
        let read = || -> Result<Self, String> {
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut header = [0; SQUARE_HEADER_BYTES];
            file.read_exact_at(&mut header, 0)
                .map_err(|e| e.to_string())?;
            if header[..4] != *b"HM3S"
                || !matches!(header[4], NDEN_ONLY_SQUARE_VERSION | SQUARE_VERSION)
                || header[5] as u32 != zoom
                || header[6..10] != square.0.to_le_bytes()
                || header[10..14] != square.1.to_le_bytes()
            {
                return Err("not this square's HM3 bundle".into());
            }
            let has_lden = header[4] == SQUARE_VERSION;
            let directory_bytes =
                (LAYERS + 1) * per_side * per_side * 8 * if has_lden { 2 } else { 1 };
            let mut directory = vec![0; directory_bytes];
            file.read_exact_at(&mut directory, SQUARE_HEADER_BYTES as u64)
                .map_err(|e| e.to_string())?;
            let file_bytes = file.metadata().map_err(|e| e.to_string())?.len();
            let entries: Vec<(u32, u32)> = directory
                .chunks_exact(8)
                .map(|entry| {
                    (
                        u32::from_le_bytes(entry[..4].try_into().expect("offset")),
                        u32::from_le_bytes(entry[4..].try_into().expect("length")),
                    )
                })
                .collect();
            if entries.iter().any(|&(offset, length)| {
                (offset as usize) < SQUARE_HEADER_BYTES + directory_bytes
                    || length == 0
                    || length as usize > TILE_PX * TILE_PX + 1024
                    || u64::from(offset) + u64::from(length) > file_bytes
            }) {
                return Err("HM3 bundle has an invalid tile range".into());
            }
            if has_lden {
                let ranges: std::collections::BTreeSet<_> = entries.iter().copied().collect();
                let mut end = (SQUARE_HEADER_BYTES + directory_bytes) as u64;
                for &(offset, length) in &ranges {
                    if u64::from(offset) != end {
                        return Err("HM3 bundle has gaps or overlapping streams".into());
                    }
                    end += u64::from(length);
                }
                if end != file_bytes {
                    return Err("HM3 bundle has trailing bytes".into());
                }
            }
            Ok(Self {
                file,
                entries,
                per_side,
                has_lden,
            })
        };
        read().map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn compressed_tile(&self, layer: usize, tx: usize, ty: usize) -> Result<Vec<u8>, String> {
        if layer > LAYERS || tx >= self.per_side || ty >= self.per_side {
            return Err("tile outside its HM3 square".into());
        }
        self.metric_tile(Metric::Nden, layer, tx, ty)
    }

    fn metric_tile(
        &self,
        metric: Metric,
        layer: usize,
        tx: usize,
        ty: usize,
    ) -> Result<Vec<u8>, String> {
        if metric == Metric::Lden && !self.has_lden {
            return Err("square has no Lden".into());
        }
        let channel = layer
            + if metric == Metric::Lden {
                LAYERS + 1
            } else {
                0
            };
        let (offset, length) = self.entries[(channel * self.per_side + ty) * self.per_side + tx];
        let mut bytes = vec![0; length as usize];
        self.file
            .read_exact_at(&mut bytes, u64::from(offset))
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

fn tile_layer_cells(cells: &Cells, per_side: usize, index: usize) -> impl Iterator<Item = u8> + '_ {
    let layer = index / (per_side * per_side);
    let tx = index % per_side;
    let ty = index / per_side % per_side;
    (0..TILE_PX).flat_map(move |row| {
        let start = (ty * TILE_PX + row) * per_side * TILE_PX + tx * TILE_PX;
        cells[start..start + TILE_PX]
            .iter()
            .map(move |pixel| pixel[layer])
    })
}

pub fn write(
    out: &Path,
    zoom: u32,
    square: (u32, u32),
    cells: &PaintedCells,
) -> Result<(), String> {
    let per_side = tiles_per_side(zoom)?;
    let side = per_side * TILE_PX;
    if cells.nden.len() != side * side
        || cells.lden.len() != side * side
        || square.0 >= 4096
        || square.1 >= 4096
    {
        return Err("not a zoom-12 square's cells".into());
    }
    let per_metric = (LAYERS + 1) * per_side * per_side;
    let count = 2 * per_metric;
    let tile_cells = |index| {
        let pixels = if index < per_metric {
            &cells.nden
        } else {
            &cells.lden
        };
        tile_layer_cells(pixels, per_side, index % per_metric)
    };
    // Equal cells in one layer have equal served bytes; find them before entropy coding.
    let first_identical_tile: Vec<usize> = (0..count)
        .map(|index| {
            (index / (per_side * per_side) * per_side * per_side..index)
                .find(|&previous| tile_cells(previous).eq(tile_cells(index)))
                .unwrap_or(index)
        })
        .collect();
    let compressed: Vec<Option<Vec<u8>>> = (0..count)
        .into_par_iter()
        .map(|index| {
            if first_identical_tile[index] == index {
                let tile: Vec<u8> = tile_cells(index).collect();
                let metric = if index < per_metric {
                    Metric::Nden
                } else {
                    Metric::Lden
                };
                encode_metric_tile(&tile, (index % per_metric) / (per_side * per_side), metric)
                    .map(Some)
            } else {
                Ok(None)
            }
        })
        .collect::<Result<_, _>>()?;
    let mut bytes = Vec::from(&b"HM3S"[..]);
    bytes.extend([SQUARE_VERSION, zoom as u8]);
    bytes.extend(square.0.to_le_bytes());
    bytes.extend(square.1.to_le_bytes());
    bytes.resize(SQUARE_HEADER_BYTES + count * 8, 0);
    let mut offsets = vec![0; count];
    for (index, first) in first_identical_tile.into_iter().enumerate() {
        let tile = compressed[first]
            .as_ref()
            .expect("an encoded distinct tile");
        let offset = if first == index {
            let offset = bytes.len() as u32;
            bytes.extend_from_slice(tile);
            offsets[index] = offset;
            offset
        } else {
            offsets[first]
        };
        let entry = &mut bytes[SQUARE_HEADER_BYTES + index * 8..][..8];
        entry[..4].copy_from_slice(&offset.to_le_bytes());
        entry[4..].copy_from_slice(&(tile.len() as u32).to_le_bytes());
    }
    let path = square_path(out, zoom, square);
    let directory = path.parent().expect("a square's directory");
    std::fs::create_dir_all(directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    temporary.write_all(&bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist_noclobber(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    sync_square_directory(directory)
}

fn sync_square_directory(directory: &Path) -> Result<(), String> {
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("{}: {error}", directory.display()))
}

pub fn read_nden(out: &Path, zoom: u32, square: (u32, u32)) -> Result<Cells, String> {
    PaintedSquare::open(out, zoom, square)?.read_cells(Metric::Nden)
}

impl PaintedSquare {
    pub fn read_lden(&self) -> Result<Cells, String> {
        self.read_cells(Metric::Lden)
    }

    fn read_cells(&self, metric: Metric) -> Result<Cells, String> {
        let side = self.per_side * TILE_PX;
        let mut cells = vec![[NO_LEVEL; LAYERS + 1]; side * side];
        for layer in 0..=LAYERS {
            for ty in 0..self.per_side {
                for tx in 0..self.per_side {
                    let tile = decode_metric_tile(
                        &self.metric_tile(metric, layer, tx, ty)?,
                        layer,
                        metric,
                    )?;
                    for (row, values) in tile.chunks_exact(TILE_PX).enumerate() {
                        let start = (ty * TILE_PX + row) * side + tx * TILE_PX;
                        for (pixel, &byte) in cells[start..start + TILE_PX].iter_mut().zip(values) {
                            pixel[layer] = byte;
                        }
                    }
                }
            }
        }
        Ok(cells)
    }
}

/// Decode every stored quantity; return whether Lden is present, never invent it for old squares.
pub fn verify(out: &Path, zoom: u32, square: (u32, u32)) -> Result<bool, String> {
    let stored = PaintedSquare::open(out, zoom, square)?;
    let nden = stored.read_cells(Metric::Nden)?;
    if stored.has_lden {
        let lden = stored.read_cells(Metric::Lden)?;
        for (at, (n, l)) in nden.iter().zip(&lden).enumerate() {
            if n.iter()
                .chain(l)
                .any(|&value| (value == NO_LEVEL) != (n[LAYERS] == NO_LEVEL))
            {
                return Err(format!("pixel {at}: Nden and Lden assessment differs"));
            }
            let level = |byte| if byte == SILENCE { -1 } else { i16::from(byte) };
            if l[LAYERS] != NO_LEVEL
                && l[..LAYERS]
                    .iter()
                    .any(|&v| v == NO_LEVEL || level(v) > level(l[LAYERS]))
            {
                return Err(format!("pixel {at}: Lden total is below a layer"));
            }
        }
    }
    Ok(stored.has_lden)
}

pub fn painted(out: &Path, zoom: u32, square: (u32, u32)) -> Result<bool, String> {
    let path = square_path(out, zoom, square);
    match path.metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(metadata) if metadata.is_file() => {
            PaintedSquare::open(out, zoom, square)?;
            // A previous run can fail after installing the filename; finish its directory sync.
            sync_square_directory(path.parent().expect("a square's directory"))?;
            Ok(true)
        }
        Ok(_) => Err(format!("{}: not a square bundle file", path.display())),
    }
}
