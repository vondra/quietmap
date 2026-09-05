//! Strict native z9 mmap sampling with a byte-bounded cache and caller-retained hot tiles.

use crate::catalog::{self, Coverage, Digest};
use crate::channel::Channel;
use grid::{raster::RasterWindow, square_of, Square};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub enum Interp {
    Bilinear,
    Nearest,
}

pub struct RawTile {
    pixels: Option<Mmap>,
    window: RasterWindow,
    channel: Channel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileIdentity {
    fn read(file: &File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

impl RawTile {
    fn load(
        root: &Path,
        square: Square,
        channel: Channel,
        digest: Digest,
        verified: Option<FileIdentity>,
    ) -> Result<(Self, FileIdentity), String> {
        let path = channel.path(root, square);
        let window = RasterWindow::for_square(square);
        let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let identity = FileIdentity::read(&file)?;
        if identity.length != channel.byte_len(window) as u64 {
            return Err(format!(
                "{}: wrong native window byte length",
                path.display()
            ));
        }
        let pixels = unsafe { Mmap::map(&file) }.map_err(|error| error.to_string())?;
        if verified != Some(identity) {
            #[cfg(test)]
            tests::observe_full_hash(&file);
            if catalog::content_digest(&pixels) != digest {
                return Err(format!("{}: published checksum mismatch", path.display()));
            }
        }
        if FileIdentity::read(&file)? != identity {
            return Err(format!(
                "{}: file changed during validation",
                path.display()
            ));
        }
        Ok((
            Self {
                pixels: Some(pixels),
                window,
                channel,
            },
            identity,
        ))
    }

    fn read_pixel(&self, row: u32, column: u32) -> f64 {
        let Some(pixels) = &self.pixels else {
            return f64::from(self.channel.ocean_value());
        };
        let index = row.min(self.window.rows - 1) as usize * self.window.columns as usize
            + column.min(self.window.columns - 1) as usize;
        self.channel.decode(pixels, index)
    }

    fn sample(&self, lat: f64, lon: f64, interp: Interp) -> f64 {
        if self.pixels.is_none() {
            return f64::from(self.channel.ocean_value());
        }
        let Some(position) = self.window.sample_position(lat, lon) else {
            return f64::NAN;
        };
        if matches!(interp, Interp::Nearest) {
            return self.read_pixel(position.nearest_row, position.nearest_column);
        }
        let row_value = |row| {
            let left = self.read_pixel(row, position.column);
            if position.column_fraction == 0.0 {
                left
            } else {
                left + position.column_fraction * (self.read_pixel(row, position.column + 1) - left)
            }
        };
        let top = row_value(position.row);
        if position.row_fraction == 0.0 {
            top
        } else {
            top + position.row_fraction * (row_value(position.row + 1) - top)
        }
    }
}

#[cfg(test)]
mod tests;

struct CachedTile {
    tile: Option<Arc<RawTile>>,
    touched: u64,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    tiles: HashMap<Square, CachedTile>,
    // Published files are immutable. Receipts survive mmap eviction and detect ordinary
    // writes/replacements on reload, not silent bitrot with unchanged identity metadata.
    verified: HashMap<Square, FileIdentity>,
    bytes: usize,
}

pub struct TileStore {
    root: PathBuf,
    channel: Channel,
    coverage: Result<Coverage, String>,
    ocean: Arc<RawTile>,
    cache: Mutex<Cache>,
    use_counter: AtomicU64,
    max_bytes: usize,
}

impl TileStore {
    pub fn new(root: &Path, channel: Channel, max_bytes: usize) -> Self {
        Self {
            root: root.to_path_buf(),
            channel,
            coverage: catalog::read_channel(root, channel),
            ocean: Arc::new(RawTile {
                pixels: None,
                window: RasterWindow::for_square(Square { x: 0, y: 0 }),
                channel,
            }),
            cache: Mutex::new(Cache::default()),
            use_counter: AtomicU64::new(0),
            max_bytes,
        }
    }

    pub fn has_complete_coverage(&self) -> bool {
        self.coverage
            .as_ref()
            .is_ok_and(|coverage| coverage.len() == 512 * 512)
    }

    fn get_tile(&self, square: Square) -> Option<Arc<RawTile>> {
        let coverage = self.coverage.as_ref().ok()?;
        let digest = coverage.get(&square)?;
        let Some(digest) = digest else {
            // An undeclared leftover file cannot hide behind an ocean declaration.
            return self
                .channel
                .file_is_absent(&self.root, square)
                .ok()
                .filter(|absent| *absent)
                .map(|_| Arc::clone(&self.ocean));
        };
        let touched = self.use_counter.fetch_add(1, Ordering::Relaxed);
        let verified = {
            let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = cache.tiles.get_mut(&square) {
                entry.touched = touched;
                return entry.tile.clone();
            }
            cache.verified.get(&square).copied()
        };
        // File opens/checksums stay outside the shared lock: unrelated warm visitors keep moving.
        let (tile, identity) =
            match RawTile::load(&self.root, square, self.channel, *digest, verified) {
                Ok((tile, identity)) => (Some(Arc::new(tile)), Some(identity)),
                Err(error) => {
                    eprintln!("raster-reader: REFUSED {error}");
                    (None, None)
                }
            };
        let bytes = std::mem::size_of::<CachedTile>()
            + tile
                .as_ref()
                .and_then(|tile| tile.pixels.as_ref())
                .map_or(0, |pixels| pixels.len());
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = cache.tiles.get(&square) {
            return entry.tile.clone();
        }
        if let Some(identity) = identity {
            cache.verified.insert(square, identity);
        }
        while cache.bytes.saturating_add(bytes) > self.max_bytes {
            let oldest = cache
                .tiles
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(&key, _)| key);
            let Some(oldest) = oldest else { break };
            let removed = cache.tiles.remove(&oldest).unwrap();
            cache.bytes -= removed.bytes;
        }
        if bytes <= self.max_bytes {
            cache.bytes += bytes;
            cache.tiles.insert(
                square,
                CachedTile {
                    tile: tile.clone(),
                    touched,
                    bytes,
                },
            );
        }
        tile
    }

    fn key(lat: f64, lon: f64) -> Option<Square> {
        (lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat))
            .then(|| square_of(lat, lon))
    }

    fn interpolation(&self) -> Interp {
        if self.channel == Channel::Forest {
            Interp::Nearest
        } else {
            Interp::Bilinear
        }
    }

    pub fn sample(&self, lat: f64, lon: f64) -> f64 {
        self.sample_with(lat, lon, self.interpolation())
    }

    pub fn sample_with(&self, lat: f64, lon: f64, interp: Interp) -> f64 {
        Self::key(lat, lon)
            .and_then(|square| self.get_tile(square))
            .map_or(f64::NAN, |tile| tile.sample(lat, lon, interp))
    }

    pub fn sample_cached(
        &self,
        lat: f64,
        lon: f64,
        cached_key: &mut (i32, i32),
        cached_tile: &mut Option<Arc<RawTile>>,
    ) -> f64 {
        self.sample_cached_with(lat, lon, self.interpolation(), cached_key, cached_tile)
    }

    pub fn sample_cached_with(
        &self,
        lat: f64,
        lon: f64,
        interp: Interp,
        cached_key: &mut (i32, i32),
        cached_tile: &mut Option<Arc<RawTile>>,
    ) -> f64 {
        let Some(square) = Self::key(lat, lon) else {
            return f64::NAN;
        };
        let key = (i32::from(square.y), i32::from(square.x));
        if key != *cached_key {
            *cached_key = key;
            *cached_tile = self.get_tile(square);
        }
        cached_tile
            .as_ref()
            .map_or(f64::NAN, |tile| tile.sample(lat, lon, interp))
    }

    pub fn preload_bbox(&self, lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) {
        let Some(north_west) = Self::key(lat_max, lon_min) else {
            return;
        };
        let Some(south_east) = Self::key(lat_min, lon_max) else {
            return;
        };
        let mut x = north_west.x;
        loop {
            for y in north_west.y..=south_east.y {
                self.get_tile(Square { x, y });
            }
            if x == south_east.x {
                break;
            }
            x = (x + 1) % 512;
        }
    }

    #[cfg(test)]
    pub(crate) fn cache_touch_count(&self) -> u64 {
        self.use_counter.load(Ordering::Relaxed)
    }
}
