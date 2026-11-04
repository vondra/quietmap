//! File identities, immutable manifests and publication for the resumable year runner.
use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufReader, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub fn digest(path: &Path) -> Result<String> {
    let mut input = BufReader::new(fs::File::open(path)?);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn location(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(location(parent)?.join(path.file_name().context("directory needs a name")?))
        }
        Err(error) => Err(error.into()),
    }
}

pub fn stamp(path: &Path) -> Result<serde_json::Value> {
    let metadata = fs::metadata(path).with_context(|| format!("inventory {}", path.display()))?;
    Ok(json!([
        path,
        metadata.len(),
        metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string()
    ]))
}

pub fn source_identity(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    inventory(root, &mut files)?;
    // A world has terabytes of immutable publisher data: stat them, hash the small recipes.
    for file in &mut files {
        let path = PathBuf::from(file[0].as_str().context("input path")?);
        file.as_array_mut()
            .unwrap()
            .push(json!(path.canonicalize()?));
        if path.parent() == Some(root) && fs::metadata(&path)?.len() <= 1_048_576 {
            file.as_array_mut().unwrap().push(json!(digest(&path)?));
        }
    }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&files)?)))
}

pub fn library_identity(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    inventory(root, &mut files)?;
    for file in &mut files {
        let path = PathBuf::from(file[0].as_str().context("library path")?);
        file.as_array_mut().unwrap().push(json!(digest(&path)?));
    }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&files)?)))
}

pub fn link(target: &Path, alias: &Path) -> Result<()> {
    if let Ok(existing) = fs::read_link(alias) {
        ensure!(existing == target, "conflicting alias {}", alias.display());
    } else {
        ensure!(
            !alias.exists(),
            "input alias already exists: {}",
            alias.display()
        );
        std::os::unix::fs::symlink(target, alias)?;
    }
    Ok(())
}

pub fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn publish(source: &Path, target: &Path) -> Result<()> {
    if source.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            publish(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if !target.exists() {
        fs::create_dir_all(target.parent().context("published output has no parent")?)?;
        let temporary = target.with_extension("publishing");
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        if let Err(error) = fs::hard_link(source, &temporary) {
            if error.raw_os_error() != Some(libc::EXDEV) {
                return Err(error.into());
            }
            fs::copy(source, &temporary)?;
        }
        fs::rename(temporary, target)?;
    } else {
        let a = fs::metadata(source)?;
        let b = fs::metadata(target)?;
        if (a.dev(), a.ino()) == (b.dev(), b.ino()) {
            return Ok(());
        }
        ensure!(
            digest(source)? == digest(target)?,
            "published output changed: {}",
            target.display()
        );
    }
    Ok(())
}

pub fn pin_manifest(root: &Path, source: &Path, year: &str) -> Result<PathBuf> {
    let target = root.join("year.json");
    if !target.exists() {
        let tables = source
            .parent()
            .context("manifest directory")?
            .join("tables")
            .join(year);
        if tables.is_dir() {
            let copies = root.join("tables").join(year);
            fs::create_dir_all(&copies)?;
            for entry in fs::read_dir(tables)? {
                let entry = entry?;
                if entry.path().is_file() {
                    fs::copy(entry.path(), copies.join(entry.file_name()))?;
                }
            }
        }
        let temporary = root.join("year.json.tmp");
        fs::copy(source, &temporary)?;
        fs::rename(temporary, &target)?;
    }
    ensure!(
        digest(source)? == digest(&target)?,
        "pinned manifest changed"
    );
    Ok(target)
}

pub fn owned_inventory(root: &Path, name: &str, files: &mut Vec<serde_json::Value>) -> Result<()> {
    let names: &[&str] = match name {
        "terrain" => &["terrain", "terrain.json"],
        "terrain-fine" => &["terrain-fine"],
        "obstacles" => &["obstacles"],
        "sources" => &["sources"],
        "aircraft-boxes" => &["aircraft", "aircraft-far"],
        "aircraft-events" => &["aircraft-events"],
        "complete" => &["complete", "year.json"],
        _ => return inventory(root, files),
    };
    fn visit(path: &Path, names: &[&str], files: &mut Vec<serde_json::Value>) -> Result<()> {
        if path.is_dir() {
            let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                visit(&entry.path(), names, files)?;
            }
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| names.contains(&name))
        {
            files.push(stamp(path)?);
        }
        Ok(())
    }
    visit(root, names, files)
}

pub fn inventory(path: &Path, files: &mut Vec<serde_json::Value>) -> Result<()> {
    if path.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            inventory(&entry.path(), files)?;
        }
    } else {
        files.push(stamp(path)?);
    }
    Ok(())
}
