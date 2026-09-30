//! Which archive parts hold a provider-day. The primary provider's download keeps the publisher's
//! choice in `catalog.sqlite`: one preferred export per day (table `assets`), replaced by the one
//! intact alternative where the preferred export failed its TAR check (table `archive_checks`); an
//! MLAT-only export is no complete day. The secondary provider has one `<year>/<day>/` directory.

use super::archive::{checked_archive_parts, directory_archive_parts};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

/// The checked parts of the primary provider's export of `day`; `None` when the day has no
/// complete export (missing, never zero).
pub fn primary_day_parts(root: &Path, day: &str) -> Result<Option<Vec<PathBuf>>, String> {
    let catalog = root.join("catalog.sqlite");
    let database = Connection::open_with_flags(&catalog, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("{}: {error}", catalog.display()))?;
    let failed = |error: rusqlite::Error| format!("{}: {error}", catalog.display());
    let mut statement = database
        .prepare("SELECT name, size, tag FROM assets WHERE day = ?1 ORDER BY name")
        .map_err(failed)?;
    let assets: Vec<(String, u64, String)> = statement
        .query_map([day], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(failed)?
        .collect::<Result<_, _>>()
        .map_err(failed)?;
    let Some(preferred) = assets.first().map(|asset| asset.2.clone()) else {
        return Err(format!("{day} lies outside the catalog's window"));
    };
    if assets.iter().any(|asset| asset.2 != preferred) {
        return Err(format!("{day}: more than one preferred export"));
    }
    if preferred.contains("mlatonly") {
        return Ok(None);
    }
    let check_error = |tag: &str| -> Result<Option<String>, String> {
        database
            .query_row(
                "SELECT error FROM archive_checks WHERE tag = ?1",
                [tag],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)
    };
    let tag = match check_error(&preferred)? {
        Some(error) if !error.is_empty() => {
            let prefix = format!("v{}-planes-readsb-", day.replace('-', "."));
            let mut statement = database
                .prepare("SELECT tag FROM archive_checks WHERE error = '' ORDER BY tag")
                .map_err(failed)?;
            let intact: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .map_err(failed)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(failed)?
                .into_iter()
                .filter(|tag| tag.starts_with(&prefix) && !tag.contains("mlatonly"))
                .collect();
            match intact.as_slice() {
                [alternative] => alternative.clone(),
                _ => {
                    return Err(format!(
                        "{day}: preferred export {preferred} failed ({error}) and {} intact \
                         alternatives exist, not one",
                        intact.len()
                    ));
                }
            }
        }
        _ => preferred.clone(),
    };
    let directory = root.join(&day[..4]).join(day);
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut parts = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name
            .strip_prefix(tag.as_str())
            .is_some_and(|rest| rest.starts_with(".tar"))
        {
            parts.push(path);
        }
    }
    if tag == preferred {
        for (name, size, _) in &assets {
            let path = directory.join(name);
            let found = path.metadata().map(|m| m.len()).ok();
            if found != Some(*size) {
                return Err(format!(
                    "{}: {found:?} bytes, catalog {size}",
                    path.display()
                ));
            }
        }
        if parts.len() != assets.len() {
            return Err(format!(
                "{day}: {} parts on disk, catalog {}",
                parts.len(),
                assets.len()
            ));
        }
    }
    checked_archive_parts(parts).map(Some)
}

/// The checked parts of the secondary provider's `<root>/<year>/<day>/`; `None` without that
/// directory.
pub fn secondary_day_parts(root: &Path, day: &str) -> Result<Option<Vec<PathBuf>>, String> {
    let directory = root.join(&day[..4]).join(day);
    if !directory.is_dir() {
        return Ok(None);
    }
    directory_archive_parts(&directory).map(Some)
}
