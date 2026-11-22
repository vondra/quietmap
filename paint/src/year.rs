//! Bind painted square trees, region caches and published tile directories to one map year.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::Path;

const MARKER: &str = "year";

/// Validate a four-digit ASCII map year (`YYYY`), stripping surrounding ASCII whitespace.
pub fn parse_year(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.len() == 4 && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        Ok(trimmed.to_owned())
    } else {
        Err(format!("invalid map year {text:?}: expected YYYY"))
    }
}

/// Read and validate `dir/year`. Fails if the marker is missing or malformed.
pub fn read_year(dir: &Path) -> Result<String, String> {
    let path = dir.join(MARKER);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_year(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

/// Bind `dir` to `year`: if `dir/year` exists it must match `year`; if absent and `has_payload`
/// reports existing files the directory is refused; otherwise `dir/year` is created atomically
/// with no-clobber and parent-directory sync.
fn bind_year(
    dir: &Path,
    year: &str,
    has_payload: impl FnOnce(&Path) -> Result<bool, String>,
) -> Result<(), String> {
    let year = parse_year(year)?;
    let path = dir.join(MARKER);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let existing = parse_year(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
            if existing != year {
                return Err(format!(
                    "{}: bound to year {existing}, not {year}",
                    dir.display()
                ));
            }
            return Ok(());
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    if has_payload(dir)? {
        return Err(format!(
            "{}: contains existing files without a year marker",
            dir.display()
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    writeln!(temporary, "{year}").map_err(|e| format!("{}: {e}", dir.display()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => {
            File::open(dir)
                .and_then(|file| file.sync_all())
                .map_err(|e| format!("{}: {e}", dir.display()))?;
            Ok(())
        }
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
            let existing = read_year(dir)?;
            if existing != year {
                return Err(format!(
                    "{}: bound to year {existing}, not {year}",
                    dir.display()
                ));
            }
            Ok(())
        }
        Err(error) => Err(format!("{}: {}", path.display(), error.error)),
    }
}

fn root_entries(dir: &Path) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Bind a painted square tree (`12/`, `13/`, or `.hm3` bundles) to `year`.
pub fn bind_painted_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year, |root| {
        Ok(root_entries(root)?
            .iter()
            .any(|name| name == "12" || name == "13" || name.ends_with(".hm3")))
    })
}

/// Bind a zoom-8 region cache directory (`*.region`) to `year`.
pub fn bind_regions_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year, |root| {
        Ok(root_entries(root)?
            .iter()
            .any(|name| name.ends_with(".region")))
    })
}

/// Bind a published tiles directory (`*.pmtiles` or `current.json`, ignoring `.pack.lock`) to
/// `year`.
pub fn bind_tiles_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year, |root| {
        Ok(root_entries(root)?
            .iter()
            .any(|name| name == "current.json" || name.ends_with(".pmtiles")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_idempotently_refuses_mismatch_and_unmarked_payload() {
        let root = tempfile::tempdir().unwrap();
        let painted = root.path().join("painted");
        bind_painted_dir(&painted, "2026").unwrap();
        assert_eq!(read_year(&painted).unwrap(), "2026");
        bind_painted_dir(&painted, "2026").unwrap();
        assert!(
            bind_painted_dir(&painted, "2024")
                .unwrap_err()
                .contains("bound to year 2026, not 2024")
        );

        // Unmarked populated painted, regions and tiles dirs are all refused.
        let legacy_painted = root.path().join("legacy-painted");
        std::fs::create_dir_all(legacy_painted.join("13")).unwrap();
        assert!(
            bind_painted_dir(&legacy_painted, "2026")
                .unwrap_err()
                .contains("without a year marker")
        );

        let legacy_regions = root.path().join("legacy-regions");
        std::fs::create_dir_all(&legacy_regions).unwrap();
        std::fs::write(legacy_regions.join("0_0.region"), b"x").unwrap();
        assert!(
            bind_regions_dir(&legacy_regions, "2026")
                .unwrap_err()
                .contains("without a year marker")
        );

        let tiles = root.path().join("tiles");
        std::fs::create_dir_all(&tiles).unwrap();
        // `.pack.lock` alone does not count as existing published payload.
        std::fs::write(tiles.join(".pack.lock"), b"").unwrap();
        bind_tiles_dir(&tiles, "2024").unwrap();
        assert_eq!(read_year(&tiles).unwrap(), "2024");

        let interrupted_tiles = root.path().join("interrupted-tiles");
        std::fs::create_dir_all(&interrupted_tiles).unwrap();
        std::fs::write(interrupted_tiles.join(".pack.lock"), b"").unwrap();
        std::fs::write(interrupted_tiles.join("road.b1.pmtiles"), b"partial").unwrap();
        assert!(
            bind_tiles_dir(&interrupted_tiles, "2024")
                .unwrap_err()
                .contains("without a year marker")
        );
    }

    #[test]
    fn concurrent_first_writers_of_different_years_cannot_both_succeed() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|scope| {
            let h1 = scope.spawn(|| {
                barrier.wait();
                bind_painted_dir(&shared, "2024")
            });
            let h2 = scope.spawn(|| {
                barrier.wait();
                bind_painted_dir(&shared, "2026")
            });
            (h1.join().unwrap(), h2.join().unwrap())
        });
        assert_eq!(
            usize::from(a.is_ok()) + usize::from(b.is_ok()),
            1,
            "exactly one year may claim a fresh directory: a={a:?}, b={b:?}"
        );
        let winner = read_year(&shared).unwrap();
        assert!(winner == "2024" || winner == "2026");
    }
}
