//! Writing a release: its tile files (`tiles::write_tile`), and the completion marker written
//! last.

use std::path::Path;
use tiles::COMPLETION_MARKER;
pub use tiles::write_tile;

/// Marks a year root complete: from now on an absent tile file means "empty".
pub fn mark_complete(year_root: &Path, note: &str) -> Result<(), String> {
    let path = year_root.join(COMPLETION_MARKER);
    std::fs::write(&path, note).map_err(|error| format!("{}: {error}", path.display()))
}
