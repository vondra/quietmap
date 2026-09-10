//! Lossless native-byte window publication; source coverage, not file presence, permits a 0-byte ocean file.

use crate::channel::Channel;
use grid::raster::{RasterWindow, NODES_PER_DEGREE, SOURCE_TILE_SIDE};
use grid::Square;
use memmap2::Mmap;
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

    /// Publishes the square's file and returns its byte length: 0 for a square outside
    /// verified source coverage (the reader's ocean marker), the window length otherwise.
    /// Re-running accepts an identical published file and refuses a different one.
    pub fn publish_square(&mut self, root: &Path, square: Square) -> Result<u64, String> {
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
        let parent = path.parent().ok_or("raster path has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let absent = self
            .channel
            .file_is_absent(root, square)
            .map_err(|error| error.to_string())?;
        if !window_touches(window, &self.expected) {
            // A 0-byte file needs no staging or fsync: after a crash it is either
            // present or missing, and missing is an error, never silent ocean.
            if absent {
                File::create_new(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            } else if std::fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .len()
                != 0
            {
                return Err(format!(
                    "unexplained file in declared ocean: {}",
                    path.display()
                ));
            }
            return Ok(0);
        }
        // Streams one row at a time; only source tiles touching that row remain mapped.
        let mut staged =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        let mut row = vec![0; window.columns as usize * self.channel.bytes_per_node()];
        for index in 0..window.rows {
            self.row(window, window.north_node - index as i32, &mut row)?;
            staged.write_all(&row).map_err(|error| error.to_string())?;
        }
        staged.flush().map_err(|error| error.to_string())?;
        let length = self.channel.byte_len(window) as u64;
        if absent {
            staged
                .as_file()
                .sync_all()
                .map_err(|error| error.to_string())?;
            staged
                .persist_noclobber(&path)
                .map_err(|error| error.to_string())?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
        } else if std::fs::read(&path).map_err(|error| error.to_string())?
            != std::fs::read(staged.path()).map_err(|error| error.to_string())?
        {
            return Err(format!(
                "refusing to replace different published raster {}",
                path.display()
            ));
        }
        Ok(length)
    }
}
