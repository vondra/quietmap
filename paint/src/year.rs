//! Bind painted square trees and region caches to one map year (`dir/year`).

use std::io::{ErrorKind, Write};
use std::path::Path;

const MARKER: &str = "year";

/// Validate a four-digit ASCII map year (`YYYY`).
pub fn parse(text: &str) -> Result<String, String> {
    let year = text.trim();
    if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) {
        Ok(year.to_owned())
    } else {
        Err(format!("invalid map year {text:?}: expected YYYY"))
    }
}

/// Read and validate `dir/year`.
pub fn read(dir: &Path) -> Result<String, String> {
    let path = dir.join(MARKER);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

fn check_existing(dir: &Path, expected: &str) -> Result<bool, String> {
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(raw) => {
            let found = parse(&raw).map_err(|e| format!("{}: {e}", dir.join(MARKER).display()))?;
            if found != expected {
                return Err(format!(
                    "{}: bound to year {found}, not {expected}",
                    dir.display()
                ));
            }
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", dir.join(MARKER).display())),
    }
}

fn has_payload(dir: &Path) -> Result<bool, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    for entry in entries {
        let name = entry
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .file_name();
        let name = name.to_string_lossy();
        if name.parse::<u8>().is_ok_and(|zoom| zoom <= 15)
            || name.ends_with(".hm3")
            || name.ends_with(".region")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Bind `dir` to `year`, creating `dir/year` atomically on a fresh directory and refusing an
/// existing tree of another year or an unmarked populated tree.
pub fn bind(dir: &Path, year: &str) -> Result<(), String> {
    let year = parse(year)?;
    if check_existing(dir, &year)? {
        return Ok(());
    }
    if has_payload(dir)? {
        if check_existing(dir, &year)? {
            return Ok(());
        }
        return Err(format!(
            "{}: contains existing files without a year marker",
            dir.display()
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    writeln!(temporary, "{year}").map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let path = dir.join(MARKER);
    match temporary.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
            let found = read(dir)?;
            if found != year {
                return Err(format!(
                    "{}: bound to year {found}, not {year}",
                    dir.display()
                ));
            }
            Ok(())
        }
        Err(error) => Err(format!("{}: {}", path.display(), error.error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_once_and_refuses_mismatch_or_unmarked_payload() {
        let root = tempfile::tempdir().unwrap();
        let painted = root.path().join("painted");
        bind(&painted, "2026").unwrap();
        assert_eq!(read(&painted).unwrap(), "2026");
        bind(&painted, "2026").unwrap();
        assert!(
            bind(&painted, "2024")
                .unwrap_err()
                .contains("bound to year 2026, not 2024")
        );

        let unmarked = root.path().join("unmarked");
        std::fs::create_dir_all(unmarked.join("13")).unwrap();
        assert!(
            bind(&unmarked, "2026")
                .unwrap_err()
                .contains("without a year marker")
        );
    }
}
