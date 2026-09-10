//! Durable creation for directories that anchor generation and owner databases.
use anyhow::{Context, Result};
use std::{fs::File, path::Path};

pub fn create_dir_all(path: &Path) -> Result<()> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    std::fs::create_dir_all(path)?;
    for directory in path
        .ancestors()
        .filter(|directory| !directory.as_os_str().is_empty())
    {
        sync_directory(directory)?;
    }
    if path.is_relative()
        && !path
            .ancestors()
            .any(|directory| directory == Path::new("."))
    {
        sync_directory(Path::new("."))?;
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    File::open(path)
        .with_context(|| format!("open directory {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}
