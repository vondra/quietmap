//! Writing tile files: whole files, renamed into place, and the completion marker written last.

use std::path::Path;
use tiles::geo::TileId;
use tiles::{COMPLETION_MARKER, Kind, tile_path};

/// Writes one tile file atomically (a temporary name renamed into place).
pub fn write_tile(year_root: &Path, tile: TileId, kind: Kind, bytes: &[u8]) -> Result<(), String> {
    let path = tile_path(year_root, tile, kind);
    let directory = path.parent().expect("tile paths have a directory");
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let temporary = path.with_extension(format!("{}.partial", kind.name()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Marks a year root complete: from now on an absent tile file means "empty".
pub fn mark_complete(year_root: &Path, note: &str) -> Result<(), String> {
    let path = year_root.join(COMPLETION_MARKER);
    std::fs::write(&path, note).map_err(|error| format!("{}: {error}", path.display()))
}
