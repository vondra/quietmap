//! Lossless native-byte window publication; source coverage, not file presence, permits ocean.

use crate::catalog::{record_square, Digest};
use crate::channel::Channel;
use grid::raster::{RasterWindow, NODES_PER_DEGREE, SOURCE_TILE_SIDE};
use grid::Square;
use memmap2::Mmap;
use rusqlite::Connection;
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub type SourceKey = (i32, i32);

pub fn window_touches(window: RasterWindow, keys: &HashSet<SourceKey>) -> bool {
    let south = (window.south_node() - 1)
        .div_euclid(NODES_PER_DEGREE)
        .max(-90);
    let north = window.north_node.div_euclid(NODES_PER_DEGREE).min(89);
    let west = (window.west_node - 1).div_euclid(NODES_PER_DEGREE);
    let east = window.east_node().div_euclid(NODES_PER_DEGREE);
    (south..=north).any(|lat| {
        (west..=east).any(|lon| keys.contains(&(lat, (lon + 180).rem_euclid(360) - 180)))
    })
}

pub struct NativeSources {
    root: PathBuf,
    channel: Channel,
    expected: HashSet<SourceKey>,
    unknown: HashSet<SourceKey>,
    open: HashMap<SourceKey, Mmap>,
}

impl NativeSources {
    /// `expected` is the complete externally verified source coverage, never a scan of available outputs.
    pub fn new(
        root: &Path,
        channel: Channel,
        expected: HashSet<SourceKey>,
        unknown: HashSet<SourceKey>,
    ) -> Result<Self, String> {
        if expected.is_empty()
            || expected
                .iter()
                .chain(&unknown)
                .any(|&(lat, lon)| !(-90..90).contains(&lat) || !(-180..180).contains(&lon))
        {
            return Err("empty or invalid native source coverage".into());
        }
        if !expected.is_disjoint(&unknown) {
            return Err("source coverage overlaps unknown land".into());
        }
        let result = Self {
            root: root.into(),
            channel,
            expected,
            unknown,
            open: HashMap::new(),
        };
        // Refuse incomplete or wrong-format input trees before the first published square.
        for &key in &result.expected {
            let path = result.path(key);
            let bytes = std::fs::metadata(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .len();
            if bytes != (SOURCE_TILE_SIDE * SOURCE_TILE_SIDE * channel.bytes_per_node()) as u64 {
                return Err(format!(
                    "{}: wrong native source byte length",
                    path.display()
                ));
            }
        }
        Ok(result)
    }

    fn path(&self, (lat, lon): SourceKey) -> PathBuf {
        self.root.join(format!(
            "{}{:02}{}{:03}.{}",
            if lat < 0 { 'S' } else { 'N' },
            lat.unsigned_abs(),
            if lon < 0 { 'W' } else { 'E' },
            lon.unsigned_abs(),
            self.channel.source_extension()
        ))
    }

    /// Bind the complete native input bytes, not mtime or file presence, before publication.
    pub fn source_identity(&self, authority: &[u8]) -> Result<String, String> {
        use std::io::Read;
        let mut hash = Sha256::new();
        hash.update(authority);
        let mut keys: Vec<_> = self.expected.iter().copied().collect();
        keys.sort_unstable();
        let mut buffer = vec![0; 1024 * 1024];
        for (index, key) in keys.iter().enumerate() {
            hash.update(key.0.to_be_bytes());
            hash.update(key.1.to_be_bytes());
            let mut file = File::open(self.path(*key)).map_err(|error| error.to_string())?;
            loop {
                let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            if (index + 1) % 1000 == 0 {
                eprintln!("source identity: {}/{}", index + 1, keys.len());
            }
        }
        Ok(hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    fn source(&mut self, key: SourceKey) -> Result<Option<&Mmap>, String> {
        if !self.expected.contains(&key) {
            return Ok(None);
        }
        if !self.open.contains_key(&key) {
            let path = self.path(key);
            let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let mmap = unsafe { Mmap::map(&file) }.map_err(|error| error.to_string())?;
            if mmap.len() != SOURCE_TILE_SIDE * SOURCE_TILE_SIDE * self.channel.bytes_per_node() {
                return Err(format!("{} changed native source size", path.display()));
            }
            self.open.insert(key, mmap);
        }
        Ok(self.open.get(&key))
    }

    fn keys_for_row(window: RasterWindow, latitude_node: i32) -> Vec<(SourceKey, i32)> {
        let lat = latitude_node.div_euclid(NODES_PER_DEGREE).min(89);
        let first_lat = if latitude_node % NODES_PER_DEGREE == 0 {
            lat - 1
        } else {
            lat
        };
        let first_lon = (window.west_node - 1).div_euclid(NODES_PER_DEGREE);
        let last_lon = window.east_node().div_euclid(NODES_PER_DEGREE);
        let mut keys = Vec::new();
        for source_lat in first_lat.max(-90)..=lat {
            if latitude_node < source_lat * NODES_PER_DEGREE
                || latitude_node > (source_lat + 1) * NODES_PER_DEGREE
            {
                continue;
            }
            for unwrapped_lon in first_lon..=last_lon {
                keys.push((
                    (source_lat, (unwrapped_lon + 180).rem_euclid(360) - 180),
                    unwrapped_lon,
                ));
            }
        }
        keys
    }

    fn row(
        &mut self,
        window: RasterWindow,
        latitude_node: i32,
        output: &mut [u8],
    ) -> Result<(), String> {
        let width = self.channel.bytes_per_node();
        let ocean = self.channel.ocean_value().to_be_bytes();
        for pixel in output.chunks_exact_mut(width) {
            pixel.copy_from_slice(&ocean[2 - width..]);
        }
        let keys = Self::keys_for_row(window, latitude_node);
        self.open
            .retain(|key, _| keys.iter().any(|(needed, _)| key == needed));
        let mut written: Vec<(usize, usize)> = Vec::new();
        for ((lat, lon), unwrapped_lon) in keys {
            let source_west = unwrapped_lon * NODES_PER_DEGREE;
            let left = window.west_node.max(source_west);
            let right = window.east_node().min(source_west + NODES_PER_DEGREE);
            if left > right {
                continue;
            }
            let Some(source) = self.source((lat, lon))? else {
                continue;
            };
            let row = ((lat + 1) * NODES_PER_DEGREE - latitude_node) as usize;
            let offset = (row * SOURCE_TILE_SIDE + (left - source_west) as usize) * width;
            let begin = (left - window.west_node) as usize * width;
            let end = (right - window.west_node + 1) as usize * width;
            let bytes = &source[offset..offset + end - begin];
            for &(previous_begin, previous_end) in &written {
                let common_begin = begin.max(previous_begin);
                let common_end = end.min(previous_end);
                if common_begin < common_end
                    && output[common_begin..common_end]
                        != bytes[common_begin - begin..common_end - begin]
                {
                    return Err(format!("native source seam disagrees at latitude node {latitude_node}, source {lat}/{lon}"));
                }
            }
            if width == 1 && bytes.iter().any(|&value| value > 100) {
                return Err(format!("invalid percentage in native source {lat}/{lon}"));
            }
            output[begin..end].copy_from_slice(bytes);
            written.push((begin, end));
        }
        Ok(())
    }

    /// Streams one row at a time; only source tiles touching that row remain mapped.
    pub fn publish_square(
        &mut self,
        database: &Connection,
        root: &Path,
        square: Square,
    ) -> Result<Option<Digest>, String> {
        let window = RasterWindow::for_square(square);
        if window_touches(window, &self.unknown) {
            return Err(format!(
                "{} z9/{}/{} includes land outside verified source coverage",
                self.channel.name(),
                square.x,
                square.y
            ));
        }
        let path = self.channel.path(root, square);
        if !window_touches(window, &self.expected) {
            if !self
                .channel
                .file_is_absent(root, square)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "unexplained file in declared ocean: {}",
                    path.display()
                ));
            }
            record_square(database, self.channel, square, None)?;
            return Ok(None);
        }
        let parent = path.parent().ok_or("raster path has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut staged =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        let mut hash = Sha256::new();
        let mut row = vec![0; window.columns as usize * self.channel.bytes_per_node()];
        for index in 0..window.rows {
            self.row(window, window.north_node - index as i32, &mut row)?;
            hash.update(&row);
            staged.write_all(&row).map_err(|error| error.to_string())?;
        }
        staged.flush().map_err(|error| error.to_string())?;
        staged
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        let digest: Digest = hash.finalize().into();
        if !self
            .channel
            .file_is_absent(root, square)
            .map_err(|error| error.to_string())?
        {
            let file = File::open(&path).map_err(|error| error.to_string())?;
            let existing = unsafe { Mmap::map(&file) }.map_err(|error| error.to_string())?;
            let actual: Digest = Sha256::digest(&existing).into();
            if existing.len() != self.channel.byte_len(window) || actual != digest {
                return Err(format!(
                    "refusing to replace different published raster {}",
                    path.display()
                ));
            }
        } else {
            staged
                .persist_noclobber(&path)
                .map_err(|error| error.to_string())?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
        }
        record_square(database, self.channel, square, Some(digest))?;
        Ok(Some(digest))
    }
}
