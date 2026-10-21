//! HM3 tile streams and the atomic square bundles the painter writes.

use crate::exact::LAYERS;
use crate::paint::Cells;
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use tiles::sources::Layer;

pub const TILE_PX: usize = 512;
pub const HEADER: [u8; 5] = *b"HM3 \x07";
const SQUARE_HEADER_BYTES: usize = 14;
// Fixed Paeth at quality 11 meets the measured volume budget; Up and quality 10 do not.
const BROTLI_QUALITY: u8 = 11;
// This is the smallest window that retains every possible within-tile match.
const BROTLI_WINDOW_LOG2: u8 = 19;

pub fn layer_names() -> [&'static str; LAYERS + 1] {
    std::array::from_fn(|number| {
        if number < LAYERS {
            Layer::ALL[number].name()
        } else {
            "total"
        }
    })
}

pub fn square_path(out: &Path, zoom: u32, square: (u32, u32)) -> PathBuf {
    out.join(zoom.to_string())
        .join((square.0 / 8).to_string())
        .join((square.1 / 8).to_string())
        .join(format!("{}_{}.hm3", square.0, square.1))
}

fn tiles_per_side(zoom: u32) -> Result<usize, String> {
    match zoom {
        12 | 13 => Ok(1 << (zoom - 12)),
        _ => Err("the painted zoom is 12 or 13".into()),
    }
}

/// PNG's Paeth predictor, including its left/above/corner tie order (PNG specification, filter 4).
fn paeth_prediction(cells: &[u8], at: usize) -> u8 {
    let has_left_pixel = !at.is_multiple_of(TILE_PX);
    let has_above_pixel = at >= TILE_PX;
    let left = if has_left_pixel { cells[at - 1] } else { 0 };
    let above = if has_above_pixel {
        cells[at - TILE_PX]
    } else {
        0
    };
    let upper_left = if has_above_pixel && has_left_pixel {
        cells[at - TILE_PX - 1]
    } else {
        0
    };
    let estimate = i16::from(left) + i16::from(above) - i16::from(upper_left);
    let left_distance = (estimate - i16::from(left)).abs();
    let above_distance = (estimate - i16::from(above)).abs();
    let corner_distance = (estimate - i16::from(upper_left)).abs();
    if left_distance <= above_distance && left_distance <= corner_distance {
        left
    } else if above_distance <= corner_distance {
        above
    } else {
        upper_left
    }
}

/// One served Brotli stream: HM3 version 7, layer byte, then fixed PNG Paeth residuals modulo 256.
pub fn encode_tile(cells: &[u8], layer: usize) -> Result<Vec<u8>, String> {
    if cells.len() != TILE_PX * TILE_PX || layer > LAYERS {
        return Err("not one layer's 512 by 512 cells".into());
    }
    let mut predicted = Vec::with_capacity(HEADER.len() + 1 + cells.len());
    predicted.extend_from_slice(&HEADER);
    predicted.push(layer as u8);
    for (at, &cell) in cells.iter().enumerate() {
        predicted.push(cell.wrapping_sub(paeth_prediction(cells, at)));
    }
    // Google's C encoder: at quality 11 it takes half to three quarters of the Rust port's time on
    // painted tiles, its streams within 0.1 % of the port's bytes (2026-10-21).
    let encoder = (brotlic::BrotliEncoderOptions::new())
        .quality(brotlic::Quality::new(BROTLI_QUALITY).map_err(|e| e.to_string())?)
        .window_size(brotlic::WindowSize::new(BROTLI_WINDOW_LOG2).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;
    let mut encoded = brotlic::CompressorWriter::with_encoder(encoder, Vec::new());
    encoded.write_all(&predicted).map_err(|e| e.to_string())?;
    encoded.into_inner().map_err(|e| e.to_string())
}

pub fn decode_tile(encoded: &[u8], layer: usize) -> Result<Vec<u8>, String> {
    if layer > LAYERS {
        return Err("unknown HM3 layer".into());
    }
    let expected = HEADER.len() + 1 + TILE_PX * TILE_PX;
    let mut bytes = vec![0; expected + 1];
    let (mut available_in, mut input_offset) = (encoded.len(), 0);
    let (mut available_out, mut output_offset, mut written) = (bytes.len(), 0, 0);
    let mut state = brotli::BrotliState::new(
        brotli::HeapAlloc::default(),
        brotli::HeapAlloc::default(),
        brotli::HeapAlloc::default(),
    );
    let result = brotli::BrotliDecompressStream(
        &mut available_in,
        &mut input_offset,
        encoded,
        &mut available_out,
        &mut output_offset,
        &mut bytes,
        &mut written,
        &mut state,
    );
    // Buffered readers can consume trailing garbage; the streaming API proves the exact range.
    if !matches!(result, brotli::BrotliResult::ResultSuccess)
        || input_offset != encoded.len()
        || output_offset != expected
        || bytes[..HEADER.len()] != HEADER
        || bytes[HEADER.len()] as usize != layer
    {
        return Err("not this layer's HM3 tile".into());
    }
    bytes.truncate(expected);
    bytes.drain(..HEADER.len() + 1);
    for at in 0..bytes.len() {
        bytes[at] = bytes[at].wrapping_add(paeth_prediction(&bytes, at));
    }
    Ok(bytes)
}

/// A complete square: HM3S, version, zoom, square x/y (u32 little-endian), then (offset, length)
/// u32 pairs in layer, tile-row, tile-column order. Offsets start at the file's beginning;
/// identical streams share a range. Every stream is already the bytes served from PMTiles.
pub struct PaintedSquare {
    file: std::fs::File,
    entries: Vec<(u32, u32)>,
    per_side: usize,
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
                || header[4] != HEADER[4]
                || header[5] as u32 != zoom
                || header[6..10] != square.0.to_le_bytes()
                || header[10..14] != square.1.to_le_bytes()
            {
                return Err("not this square's HM3 bundle".into());
            }
            let directory_bytes = (LAYERS + 1) * per_side * per_side * 8;
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
            Ok(Self {
                file,
                entries,
                per_side,
            })
        };
        read().map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn compressed_tile(&self, layer: usize, tx: usize, ty: usize) -> Result<Vec<u8>, String> {
        if layer > LAYERS || tx >= self.per_side || ty >= self.per_side {
            return Err("tile outside its HM3 square".into());
        }
        let (offset, length) = self.entries[(layer * self.per_side + ty) * self.per_side + tx];
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

pub fn write(out: &Path, zoom: u32, square: (u32, u32), cells: &Cells) -> Result<(), String> {
    let per_side = tiles_per_side(zoom)?;
    let side = per_side * TILE_PX;
    if cells.len() != side * side || square.0 >= 4096 || square.1 >= 4096 {
        return Err("not a zoom-12 square's cells".into());
    }
    let count = (LAYERS + 1) * per_side * per_side;
    // Equal cells in one layer have equal served bytes; find them before entropy coding.
    let first_identical_tile: Vec<usize> = (0..count)
        .map(|index| {
            (index / (per_side * per_side) * per_side * per_side..index)
                .find(|&previous| {
                    tile_layer_cells(cells, per_side, previous)
                        .eq(tile_layer_cells(cells, per_side, index))
                })
                .unwrap_or(index)
        })
        .collect();
    let compressed: Vec<Option<Vec<u8>>> = (0..count)
        .into_par_iter()
        .map(|index| {
            if first_identical_tile[index] == index {
                let tile: Vec<u8> = tile_layer_cells(cells, per_side, index).collect();
                encode_tile(&tile, index / (per_side * per_side)).map(Some)
            } else {
                Ok(None)
            }
        })
        .collect::<Result<_, _>>()?;
    let mut bytes = Vec::from(&b"HM3S"[..]);
    bytes.extend([HEADER[4], zoom as u8]);
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

pub fn read(out: &Path, zoom: u32, square: (u32, u32)) -> Result<Cells, String> {
    let stored = PaintedSquare::open(out, zoom, square)?;
    let side = stored.per_side * TILE_PX;
    let mut cells = vec![[crate::paint::NO_LEVEL; LAYERS + 1]; side * side];
    for layer in 0..=LAYERS {
        for ty in 0..stored.per_side {
            for tx in 0..stored.per_side {
                let tile = decode_tile(&stored.compressed_tile(layer, tx, ty)?, layer)?;
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

fn listed(path: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .map(|entry| entry.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect()
}

/// The squares of one bundle directory's files.
fn squares_of(
    out: &Path,
    zoom: u32,
    directory: &Path,
    squares: &mut BTreeSet<(u32, u32)>,
) -> Result<(), String> {
    for path in listed(directory)? {
        if let Some((x, y)) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".hm3"))
            .and_then(|name| name.split_once('_'))
            .and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?)))
        {
            if x >= 4096 || y >= 4096 || path != square_path(out, zoom, (x, y)) {
                return Err(format!("{}: not a canonical HM3 square", path.display()));
            }
            squares.insert((x, y));
        }
    }
    Ok(())
}

pub fn squares(out: &Path, zoom: u32) -> Result<BTreeSet<(u32, u32)>, String> {
    tiles_per_side(zoom)?;
    let mut squares = BTreeSet::new();
    for column in listed(&out.join(zoom.to_string()))? {
        for row in listed(&column)? {
            squares_of(out, zoom, &row, &mut squares)?;
        }
    }
    Ok(squares)
}

/// The painted squares with x in `columns` and y in `rows`, reading only their directories.
pub fn squares_within(
    out: &Path,
    zoom: u32,
    columns: std::ops::Range<u32>,
    rows: std::ops::Range<u32>,
) -> Result<BTreeSet<(u32, u32)>, String> {
    tiles_per_side(zoom)?;
    let mut squares = BTreeSet::new();
    for column in columns.start / 8..columns.end.div_ceil(8) {
        for row in rows.start / 8..rows.end.div_ceil(8) {
            let directory = out
                .join(zoom.to_string())
                .join(column.to_string())
                .join(row.to_string());
            if directory.is_dir() {
                squares_of(out, zoom, &directory, &mut squares)?;
            }
        }
    }
    squares.retain(|(x, y)| columns.contains(x) && rows.contains(y));
    Ok(squares)
}

#[cfg(test)]
#[path = "hm3_tests.rs"]
mod tests;
