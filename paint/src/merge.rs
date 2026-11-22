//! Merge completed square bundles by atomic byte copies, without decoding or replacing data.

use crate::hm3::{PaintedSquare, square_path, squares};
use crate::year::{bind_painted_dir, read_year};
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

fn verify_identical(source_path: &Path, destination_path: &Path) -> Result<(), String> {
    // A retry or concurrent merge skips identical copies and never replaces a conflicting square.
    if std::fs::read(source_path).map_err(|e| e.to_string())?
        != std::fs::read(destination_path).map_err(|e| e.to_string())?
    {
        return Err(format!(
            "{}: differs from the worker's square",
            destination_path.display()
        ));
    }
    Ok(())
}

pub fn merge(source: &Path, destination: &Path, zoom: u32) -> Result<(), String> {
    let year = read_year(source)?;
    bind_painted_dir(destination, &year)?;
    for square in squares(source, zoom)? {
        PaintedSquare::open(source, zoom, square)?;
        let source_path = square_path(source, zoom, square);
        let destination_path = square_path(destination, zoom, square);
        if destination_path.try_exists().map_err(|e| e.to_string())? {
            verify_identical(&source_path, &destination_path)?;
            continue;
        }
        let directory = destination_path.parent().expect("a square's directory");
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        let mut input = File::open(&source_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut temporary).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o644))
                .map_err(|e| e.to_string())?;
        }
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        // An interrupted copy never acquires a completed square's name.
        match temporary.persist_noclobber(&destination_path) {
            Ok(_) => {
                File::open(directory)
                    .and_then(|file| file.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                verify_identical(&source_path, &destination_path)?;
            }
            Err(error) => {
                return Err(format!("{}: {}", destination_path.display(), error.error));
            }
        }
    }
    Ok(())
}
