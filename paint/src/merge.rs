//! Merge completed square bundles by atomic byte copies, without decoding or replacing data.

use crate::hm3::{PaintedSquare, square_path, squares};
use std::fs::File;
use std::path::Path;

pub fn merge(source: &Path, destination: &Path, zoom: u32) -> Result<(), String> {
    for square in squares(source, zoom)? {
        PaintedSquare::open(source, zoom, square)?;
        let source_path = square_path(source, zoom, square);
        let destination_path = square_path(destination, zoom, square);
        if destination_path.try_exists().map_err(|e| e.to_string())? {
            // A retry skips identical completed copies and never replaces a conflicting square.
            if std::fs::read(&source_path).map_err(|e| e.to_string())?
                != std::fs::read(&destination_path).map_err(|e| e.to_string())?
            {
                return Err(format!(
                    "{}: differs from the worker's square",
                    destination_path.display()
                ));
            }
            continue;
        }
        let directory = destination_path.parent().expect("a square's directory");
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        let mut input = File::open(&source_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut temporary).map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        // An interrupted copy never acquires a completed square's name.
        temporary
            .persist_noclobber(&destination_path)
            .map_err(|e| format!("{}: {e}", destination_path.display()))?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
