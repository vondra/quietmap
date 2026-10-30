//! A zoom-8 region's part of the pack kept between the painting and the archives, one a metric:
//! its compressed tiles from zoom 12 to 8, its zoom-8 means and the bundles they came from.
//! `qm-paint pack-region` packs a region once its squares are painted; the world's pack then folds
//! only zooms 7 to 2 and redoes a region whose bundles changed since.

use crate::exact::LAYERS;
use crate::hm3::{Metric, square_path};
use crate::pyramid::AssessedMeans;
use pmtiles::TileCoord;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A region is a tile of this zoom: 16 x 16 squares of zoom 12.
pub(crate) const REGION_ZOOM: u8 = 8;
const MAGIC: &[u8; 5] = b"HM3R\x01";
/// The means compress fast: they are stored once and read once.
const MEANS_BROTLI_QUALITY: u32 = 1;

/// A square's bundle as the region saw it: a different length or time means a repainted square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bundle {
    pub square: (u32, u32),
    pub bytes: u64,
    pub modified_ns: u128,
}

pub(crate) struct Region {
    pub tiles: Vec<(TileCoord, Vec<Vec<u8>>)>,
    pub means: AssessedMeans,
}

/// The bundles of `squares` (zoom 12) in the painted tree `out`.
pub(crate) fn bundles(
    out: &Path,
    zoom: u32,
    squares: impl Iterator<Item = (u32, u32)>,
) -> Result<Vec<Bundle>, String> {
    squares
        .map(|square| {
            let path = square_path(out, zoom, square);
            let metadata = path
                .metadata()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let modified = metadata.modified().map_err(|e| e.to_string())?;
            Ok(Bundle {
                square,
                bytes: metadata.len(),
                modified_ns: modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos(),
            })
        })
        .collect()
}

/// `12_34.region` for Nden, `12_34.lden.region` for Lden: one directory, distinct names.
fn path(directory: &Path, x: u32, y: u32, metric: Metric) -> PathBuf {
    directory.join(match metric {
        Metric::Nden => format!("{x}_{y}.region"),
        Metric::Lden => format!("{x}_{y}.lden.region"),
    })
}

/// Writes the region atomically: a pack that reads it sees a whole region or none.
pub(crate) fn store(
    directory: &Path,
    (x, y): (u32, u32),
    metric: Metric,
    tiles: &[(TileCoord, Vec<Vec<u8>>)],
    means: &AssessedMeans,
    bundles: &[Bundle],
) -> Result<(), String> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend((bundles.len() as u32).to_le_bytes());
    for bundle in bundles {
        bytes.extend(bundle.square.0.to_le_bytes());
        bytes.extend(bundle.square.1.to_le_bytes());
        bytes.extend(bundle.bytes.to_le_bytes());
        bytes.extend(bundle.modified_ns.to_le_bytes());
    }
    bytes.extend((tiles.len() as u32).to_le_bytes());
    for (coordinate, layers) in tiles {
        bytes.push(coordinate.z());
        bytes.extend(coordinate.x().to_le_bytes());
        bytes.extend(coordinate.y().to_le_bytes());
        for layer in layers {
            bytes.extend((layer.len() as u32).to_le_bytes());
            bytes.extend(layer);
        }
    }
    let mut compressed = Vec::new();
    {
        let mut encoder =
            brotli::CompressorWriter::new(&mut compressed, 1 << 16, MEANS_BROTLI_QUALITY, 22);
        encoder
            .write_all(&means.to_bytes())
            .map_err(|e| e.to_string())?;
    }
    bytes.extend(compressed);
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path(directory, x, y, metric))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The stored region if it was packed from exactly `bundles`, else `None`.
pub(crate) fn load(
    directory: &Path,
    (x, y): (u32, u32),
    metric: Metric,
    bundles: &[Bundle],
) -> Result<Option<Region>, String> {
    let bytes = match std::fs::read(path(directory, x, y, metric)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !bytes.starts_with(MAGIC) {
        return Err(format!(
            "{}: not a region file",
            path(directory, x, y, metric).display()
        ));
    }
    let mut reader = Reader {
        bytes: &bytes,
        at: MAGIC.len(),
    };
    let count = reader.word(4)? as usize;
    let mut stored = Vec::with_capacity(count);
    for _ in 0..count {
        stored.push(Bundle {
            square: (reader.word(4)? as u32, reader.word(4)? as u32),
            bytes: reader.word(8)? as u64,
            modified_ns: reader.word(16)?,
        });
    }
    if stored != bundles {
        return Ok(None);
    }
    let tile_count = reader.word(4)? as usize;
    let mut tiles = Vec::with_capacity(tile_count);
    for _ in 0..tile_count {
        let (z, tx, ty) = (
            reader.word(1)? as u8,
            reader.word(4)? as u32,
            reader.word(4)? as u32,
        );
        let coordinate = TileCoord::new(z, tx, ty).map_err(|e| e.to_string())?;
        let mut layers = Vec::with_capacity(LAYERS + 1);
        for _ in 0..=LAYERS {
            let length = reader.word(4)? as usize;
            layers.push(reader.take(length)?.to_vec());
        }
        tiles.push((coordinate, layers));
    }
    let mut means = Vec::new();
    brotli::Decompressor::new(&bytes[reader.at..], 1 << 16)
        .read_to_end(&mut means)
        .map_err(|e| e.to_string())?;
    Ok(Some(Region {
        tiles,
        means: AssessedMeans::from_bytes(&means)?,
    }))
}

/// Little-endian fields read in order.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let taken = (self.bytes.get(self.at..self.at + length)).ok_or("a region file cut short")?;
        self.at += length;
        Ok(taken)
    }

    fn word(&mut self, length: usize) -> Result<u128, String> {
        Ok((self.take(length)?.iter().rev()).fold(0, |value, &byte| value << 8 | u128::from(byte)))
    }
}
