//! HM3 tile streams and the atomic square bundles the painter writes.

use crate::exact::LAYERS;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use tiles::sources::Layer;

pub const TILE_PX: usize = 512;
pub const HEADER: [u8; 5] = *b"HM3 \x07";

pub use crate::hm3_square::{PaintedSquare, painted, read_nden, verify, write};

/// The two quantities a square holds: Nden in half-phon cells, Lden in half-decibel cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Nden,
    Lden,
}

impl Metric {
    fn header(self) -> [u8; 5] {
        match self {
            Self::Nden => HEADER,
            Self::Lden => *b"HMD \x01",
        }
    }

    /// A layer's archive name, `lden-road` beside `road`: one manifest names both quantities.
    pub fn archive_layer(self, layer: &str) -> String {
        match self {
            Self::Nden => layer.to_string(),
            Self::Lden => format!("lden-{layer}"),
        }
    }
}
// Fixed Paeth at quality 11 meets the measured volume budget; Up and quality 10 do not.
const NDEN_BROTLI_QUALITY: u8 = 11;
// Lden uses quality 9: the same cells for much less of the painter's processor, which bounds the run.
const LDEN_BROTLI_QUALITY: u8 = 9;
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

pub(crate) fn tiles_per_side(zoom: u32) -> Result<usize, String> {
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
    encode_metric_tile(cells, layer, Metric::Nden)
}

pub(crate) fn encode_metric_tile(
    cells: &[u8],
    layer: usize,
    metric: Metric,
) -> Result<Vec<u8>, String> {
    if cells.len() != TILE_PX * TILE_PX || layer > LAYERS {
        return Err("not one layer's 512 by 512 cells".into());
    }
    let mut predicted = Vec::with_capacity(HEADER.len() + 1 + cells.len());
    predicted.extend_from_slice(&metric.header());
    predicted.push(layer as u8);
    for (at, &cell) in cells.iter().enumerate() {
        predicted.push(cell.wrapping_sub(paeth_prediction(cells, at)));
    }
    // Google's C encoder: at quality 11 it takes half to three quarters of the Rust port's time on
    // painted tiles, its streams within 0.1 % of the port's bytes (2026-10-21).
    let encoder = (brotlic::BrotliEncoderOptions::new())
        .quality(
            brotlic::Quality::new(match metric {
                Metric::Nden => NDEN_BROTLI_QUALITY,
                Metric::Lden => LDEN_BROTLI_QUALITY,
            })
            .map_err(|e| e.to_string())?,
        )
        .window_size(brotlic::WindowSize::new(BROTLI_WINDOW_LOG2).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;
    let mut encoded = brotlic::CompressorWriter::with_encoder(encoder, Vec::new());
    encoded.write_all(&predicted).map_err(|e| e.to_string())?;
    encoded.into_inner().map_err(|e| e.to_string())
}

pub fn decode_tile(encoded: &[u8], layer: usize) -> Result<Vec<u8>, String> {
    decode_metric_tile(encoded, layer, Metric::Nden)
}

pub(crate) fn decode_metric_tile(
    encoded: &[u8],
    layer: usize,
    metric: Metric,
) -> Result<Vec<u8>, String> {
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
        || bytes[..HEADER.len()] != metric.header()
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
