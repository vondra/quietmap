//! An opened release: its year root (served only when complete), the global weather table beside
//! it, and the whole-file reads of a ring's tiles.

use std::path::{Path, PathBuf};
use tiles::geo::TileId;
use tiles::read::read_all;
use tiles::{COMPLETION_MARKER, Kind, tile_path};

/// The weather table's file name beside the year roots.
pub const WEATHER_FILE: &str = "weather";

pub struct Release {
    pub year_root: PathBuf,
    /// The global weather table: a click reads its own place of it
    /// (`physics::weather::read_place`), the painter all of it.
    pub weather_path: PathBuf,
}

impl Release {
    /// Opens `<prepared>/<year>`; refuses a year root without the completion marker.
    pub fn open(prepared: &Path, year: &str) -> Result<Self, String> {
        let year_root = prepared.join(year);
        if !year_root.join(COMPLETION_MARKER).exists() {
            return Err(format!("{} is not a complete release", year_root.display()));
        }
        Ok(Release {
            year_root,
            weather_path: prepared.join(WEATHER_FILE),
        })
    }
}

/// The files of one ring, read whole and at once: per tile, one slot per requested kind.
pub struct RingFiles {
    pub tiles: Vec<TileId>,
    pub kinds: Vec<Kind>,
    /// `files[tile * kinds.len() + kind]`; `None` is an absent file.
    pub files: Vec<Option<Vec<u8>>>,
    pub bytes: u64,
    pub file_count: usize,
    pub read_seconds: f64,
}

impl RingFiles {
    /// Reads every file of `tiles` and `kinds` that is `wanted` whole and at once (the time is the
    /// ring's cold read); a file not wanted reads as absent.
    pub fn read(
        release: &Release,
        tiles: Vec<TileId>,
        kinds: Vec<Kind>,
        wanted: impl Fn(TileId, Kind) -> bool,
    ) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let slots: Vec<(TileId, Kind)> = tiles
            .iter()
            .flat_map(|&tile| kinds.iter().map(move |&kind| (tile, kind)))
            .collect();
        let paths: Vec<PathBuf> = slots
            .iter()
            .filter(|&&(tile, kind)| wanted(tile, kind))
            .map(|&(tile, kind)| tile_path(&release.year_root, tile, kind))
            .collect();
        let mut read = read_all(&paths)?.into_iter();
        let files: Vec<Option<Vec<u8>>> = slots
            .iter()
            .map(|&(tile, kind)| {
                if wanted(tile, kind) {
                    read.next().expect("one read per wanted file")
                } else {
                    None
                }
            })
            .collect();
        let bytes = files.iter().flatten().map(|file| file.len() as u64).sum();
        let file_count = files.iter().filter(|file| file.is_some()).count();
        Ok(RingFiles {
            tiles,
            kinds,
            files,
            bytes,
            file_count,
            read_seconds: started.elapsed().as_secs_f64(),
        })
    }

    pub fn file(&self, tile_index: usize, kind: Kind) -> Option<&[u8]> {
        let kind_index = self.kinds.iter().position(|&k| k == kind)?;
        self.files[tile_index * self.kinds.len() + kind_index].as_deref()
    }
}
