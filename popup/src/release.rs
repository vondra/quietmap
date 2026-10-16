//! An opened release: its year root (served only when complete) with the year's weather table in
//! it, and the reads of a ring's tiles, each whole.

use std::path::{Path, PathBuf};
use tiles::Kind;
use tiles::geo::TileId;
use tiles::square::read_tiles;

/// The weather table's file name in a year root: the long-term climatology the year is computed in
/// (the same table in every year, a reflink).
pub const WEATHER_FILE: &str = "weather";

pub struct Release {
    pub year_root: PathBuf,
    /// The year's weather table: a click reads its own place of it
    /// (`physics::weather::read_place`), the painter all of it.
    pub weather_path: PathBuf,
}

impl Release {
    /// Opens `<prepared>/<year>`; refuses a year root that is not complete in this layout.
    pub fn open(prepared: &Path, year: &str) -> Result<Self, String> {
        let year_root = prepared.join(year);
        tiles::check_year_complete(&year_root)?;
        Ok(Release {
            weather_path: year_root.join(WEATHER_FILE),
            year_root,
        })
    }
}

/// The tiles of one ring, read whole and at once: per tile, one slot per requested kind.
pub struct RingFiles {
    pub tiles: Vec<TileId>,
    pub kinds: Vec<Kind>,
    /// `files[tile * kinds.len() + kind]`; `None` is a tile with nothing of the kind.
    pub files: Vec<Option<Vec<u8>>>,
    pub bytes: u64,
    /// The tiles read that hold something.
    pub tile_count: usize,
    pub read_seconds: f64,
}

impl RingFiles {
    /// Reads every tile of `tiles` and `kinds` that is `wanted`, each whole and all at once (the
    /// time is the ring's cold read); a tile not wanted reads as empty.
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
        let requests: Vec<(TileId, Kind)> = slots
            .iter()
            .copied()
            .filter(|&(tile, kind)| wanted(tile, kind))
            .collect();
        let mut read = read_tiles(&release.year_root, &requests)?.into_iter();
        let files: Vec<Option<Vec<u8>>> = slots
            .iter()
            .map(|&(tile, kind)| {
                if wanted(tile, kind) {
                    read.next().expect("one read per wanted tile")
                } else {
                    None
                }
            })
            .collect();
        let bytes = files.iter().flatten().map(|file| file.len() as u64).sum();
        let tile_count = files.iter().filter(|file| file.is_some()).count();
        Ok(RingFiles {
            tiles,
            kinds,
            files,
            bytes,
            tile_count,
            read_seconds: started.elapsed().as_secs_f64(),
        })
    }

    pub fn file(&self, tile_index: usize, kind: Kind) -> Option<&[u8]> {
        let kind_index = self.kinds.iter().position(|&k| k == kind)?;
        self.files[tile_index * self.kinds.len() + kind_index].as_deref()
    }
}
