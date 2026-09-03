//! Where one H3 R4 cell's obstacle table lives, and what its absence means.
//!
//! Emptiness is PER CELL. The promotion materializes
//! `<h3r4>/<cell>/obstacles.arrow` for EVERY prepared cell — merged Overture
//! footprints where the sweep found any, a 0-row table where the finished sweep
//! found none (`scripts/obstacles/enrich-obstacle-heights.py`). That makes the
//! answers a loader can get distinguishable from the cell alone, with no
//! world-wide file to read and no contention with a running ingest:
//!
//! * file with rows → these are the buildings;
//! * file with no rows → there are no buildings here (an answer, not a gap);
//! * no cell directory → the cell is outside the prepared world, so it
//!   contributes nothing, exactly as it holds no `roads.arrow` and no
//!   `railways.arrow` either;
//! * a cell directory WITHOUT the file → error. The cell belongs to the world
//!   and its buildings were not delivered; painting it anyway would publish a
//!   quiet map of a loud place, and nothing downstream could tell the
//!   difference.
//!
//! Both loaders call this — the popup's `source-reader::obstacle_store` and the
//! pipeline's `tile-painter::source_loader_obstacle` — so the rule cannot drift
//! between the map and the popup.

use std::path::{Path, PathBuf};

use h3o::CellIndex;

/// The one obstacle file a prepared cell carries.
pub const CELL_OBSTACLE_FILENAME: &str = "obstacles.arrow";

/// `Ok(Some(path))`: read this file. `Ok(None)`: the cell is outside the
/// prepared world and contributes nothing. `Err`: the cell is in the world and
/// its obstacle table is missing or unreadable.
pub fn locate_cell_obstacles(h3r4_dir: &Path, cell: CellIndex) -> Result<Option<PathBuf>, String> {
    let dir = h3r4_dir.join(cell.to_string());
    let path = dir.join(CELL_OBSTACLE_FILENAME);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(Some(path)),
        Ok(_) => Err(format!("{} is not a file", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The DIRECTORY separates the two absences, and it is the same
            // signal every other layer already reads: a cell the Planet extract
            // never produced has no directory at all.
            match std::fs::metadata(&dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("stat {}: {e}", dir.display())),
                Ok(_) => Err(format!(
                    "cell {cell} has no {CELL_OBSTACLE_FILENAME} ({}) — every prepared cell \
                     carries one, empty where the Overture sweep found no footprint, so this \
                     is undelivered data, not an empty place",
                    path.display()
                )),
            }
        }
        Err(e) => Err(format!("stat {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_at(lat: f64, lon: f64) -> CellIndex {
        h3o::LatLng::new(lat, lon)
            .unwrap()
            .to_cell(h3o::Resolution::Four)
    }

    /// The three answers, and the one that must never be silently taken for
    /// "no buildings here".
    #[test]
    fn absence_of_the_file_and_absence_of_the_cell_are_different_answers() {
        let root = std::env::temp_dir().join(format!("qm-obstacle-cell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let h3r4 = root.as_path();
        let present = cell_at(50.08, 14.43);
        let unpainted = cell_at(-23.55, -46.63);
        let outside = cell_at(0.0, -30.0);

        std::fs::create_dir_all(h3r4.join(present.to_string())).unwrap();
        std::fs::write(
            h3r4.join(present.to_string()).join(CELL_OBSTACLE_FILENAME),
            b"arrow-bytes",
        )
        .unwrap();
        std::fs::create_dir_all(h3r4.join(unpainted.to_string())).unwrap();

        assert_eq!(
            locate_cell_obstacles(h3r4, present).unwrap(),
            Some(h3r4.join(present.to_string()).join(CELL_OBSTACLE_FILENAME))
        );
        assert!(
            locate_cell_obstacles(h3r4, unpainted).is_err(),
            "a prepared cell without its obstacle table is undelivered data"
        );
        assert_eq!(
            locate_cell_obstacles(h3r4, outside).unwrap(),
            None,
            "a cell the extract never produced is outside the world, not an error"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
