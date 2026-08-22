//! Read-only adapter over a loose staging tree `{layer}/{z}/{x}/{y}.bin`.
//!
//! Single-host builds still emit run-scoped loose tiles before the transcoder
//! ingests them into the working [`super::TileStore`]; parity reads both forms.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::wire_hm3;

pub struct LooseTree {
    /// The layer dir (e.g. `…/build/road`).
    root: PathBuf,
    zoom: u8,
}

impl LooseTree {
    pub fn new(layer_dir: &Path, zoom: u8) -> Self {
        LooseTree {
            root: layer_dir.to_path_buf(),
            zoom,
        }
    }

    fn tile_path(&self, x: u32, y: u32) -> PathBuf {
        self.root
            .join(self.zoom.to_string())
            .join(x.to_string())
            .join(format!("{y}.bin"))
    }

    pub fn present(&self, x: u32, y: u32) -> bool {
        self.tile_path(x, y).exists()
    }

    /// The on-disk bytes verbatim (a whole-file-Brotli HM3 image).
    pub fn get_blob(&self, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        match fs::read(self.tile_path(x, y)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read loose {x}/{y}")),
        }
    }

    pub fn get_cells(&self, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        match self.get_blob(x, y)? {
            Some(blob) => Ok(Some(wire_hm3::read_tile_bytes(&blob)?)),
            None => Ok(None),
        }
    }

    /// Walk the tree column-major (numeric order), calling `f(x, y)` for every
    /// tile file. Matches [`super::TileStore::for_each_present`] order so the
    /// two can be zip-diffed.
    pub fn for_each_present(&self, mut f: impl FnMut(u32, u32) -> Result<()>) -> Result<()> {
        let zdir = self.root.join(self.zoom.to_string());
        if !zdir.exists() {
            return Ok(());
        }
        let mut cols: Vec<u32> = fs::read_dir(&zdir)?
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        cols.sort_unstable();
        for x in cols {
            let xdir = zdir.join(x.to_string());
            let mut rows: Vec<u32> = fs::read_dir(&xdir)?
                .filter_map(|e| {
                    let name = e.ok()?.file_name();
                    name.to_str()?.strip_suffix(".bin")?.parse().ok()
                })
                .collect();
            rows.sort_unstable();
            for y in rows {
                f(x, y)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::TILE_PX;
    use crate::wire_hm3::{write_tile, NO_DATA, SOURCE_ID_RAIL};

    #[test]
    fn loose_tree_reads_and_walks_written_tiles() {
        let dir = tempfile::tempdir().unwrap();
        let layer = dir.path().join("rail");
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[42] = 88;
        for (x, y) in [(7u32, 9u32), (3, 4)] {
            let p = layer
                .join("12")
                .join(x.to_string())
                .join(format!("{y}.bin"));
            write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        }

        let tree = LooseTree::new(&layer, 12);
        assert!(tree.present(7, 9));
        assert!(!tree.present(7, 10));
        assert_eq!(tree.get_cells(3, 4).unwrap().unwrap(), cells);
        assert!(tree.get_blob(3, 4).unwrap().is_some());

        let mut seen = Vec::new();
        tree.for_each_present(|x, y| {
            seen.push((x, y));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![(3, 4), (7, 9)]);
    }
}
