//! Remove stale derived aircraft files only from canonical, in-scope z9 units.

use crate::{scope::ScopeBbox, spatial::square_directories};
use anyhow::{Context, Result};
use std::path::Path;

pub fn wipe_stale_arrows_for_scope(
    root: &Path,
    filename: &str,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    anyhow::ensure!(
        matches!(
            filename,
            "airborne.arrow" | "cruise.arrow" | "airport_traffic.arrow" | "airport_summary.arrow"
        ),
        "only a derived aircraft basename may be removed: {filename:?}"
    );
    let mut removed = 0;
    for (id, directory) in square_directories(root)? {
        if scope.is_some_and(|scope| !scope.contains_square(id)) {
            continue;
        }
        let path = directory.join(filename);
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{square_id, square_path};

    #[test]
    fn stale_cleanup_preserves_other_layers_and_out_of_scope_data() {
        let root = tempfile::tempdir().unwrap();
        let inside = root
            .path()
            .join(square_path(square_id(50.1, 14.26).unwrap()));
        let outside = root
            .path()
            .join(square_path(square_id(27.93, -15.39).unwrap()));
        for directory in [&inside, &outside] {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("airborne.arrow"), b"old").unwrap();
            std::fs::write(directory.join("airport_lines.arrow"), b"source").unwrap();
        }
        let scope = ScopeBbox::parse("48.65,12,51.55,16.9").unwrap();
        assert_eq!(
            wipe_stale_arrows_for_scope(root.path(), "airborne.arrow", Some(&scope)).unwrap(),
            1
        );
        assert!(!inside.join("airborne.arrow").exists());
        assert!(outside.join("airborne.arrow").exists());
        assert!(inside.join("airport_lines.arrow").exists());
        assert!(wipe_stale_arrows_for_scope(root.path(), "airport_lines.arrow", None).is_err());
        assert_eq!(
            wipe_stale_arrows_for_scope(root.path(), "airborne.arrow", Some(&scope)).unwrap(),
            0
        );
    }
}
