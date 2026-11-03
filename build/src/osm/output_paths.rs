//! Keep disposable scratch separate from new outputs, including topology files beside the output.

use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

fn new_path(path: &Path) -> Result<PathBuf> {
    ensure!(!path.try_exists()?, "path must be new: {}", path.display());
    let name = path
        .file_name()
        .context("output needs a final directory name")?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .with_context(|| format!("output parent {} must exist", parent.display()))?
        .join(name))
}

pub fn validate(output: &Path, scratch: &Path) -> Result<()> {
    let output = new_path(output)?;
    let scratch = new_path(scratch)?;
    ensure!(
        !output.starts_with(&scratch) && !scratch.starts_with(&output),
        "OSM output and scratch must be disjoint"
    );
    for family in ["railway-ways", "train-routes"] {
        let path = super::transport::year_sibling_path(&output, family)?;
        ensure!(
            !path.try_exists()? && !path.with_extension("arrow.tmp").try_exists()?,
            "topology output already exists: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_alias_and_existing_topology_are_rejected_before_writes() {
        let root = std::env::temp_dir().join(format!("qm-osm-paths-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let output = root.join("output");
        let scratch = root.join("scratch");
        assert!(validate(&output, &scratch).is_ok());
        std::os::unix::fs::symlink(&root, root.join("alias")).unwrap();
        assert!(validate(&output, &root.join("alias/output")).is_err());
        assert!(validate(&output, &output.join("nested")).is_err());
        let global = super::super::transport::year_sibling_path(&output, "train-routes").unwrap();
        std::fs::write(&global, b"existing").unwrap();
        assert!(validate(&output, &scratch).is_err());
        assert_eq!(std::fs::read(global).unwrap(), b"existing");
        assert!(!output.exists() && !scratch.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
