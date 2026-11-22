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

/// Read `dir/year` if present and verify it matches `expected`; returns `Ok(false)` only on ENOENT.
fn verify_existing(dir: &Path, expected: &str) -> Result<bool, String> {
    let path = dir.join(MARKER);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let existing = parse_year(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
            if existing != expected {
                return Err(format!(
                    "{}: bound to year {existing}, not {expected}",
                    dir.display()
                ));
            }
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

#[cfg(unix)]
fn make_world_readable(file: &File, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_world_readable(_file: &File, _path: &Path) -> Result<(), String> {
    Ok(())
}

fn is_payload_entry(name: &str) -> bool {
    name.parse::<u8>().is_ok_and(|zoom| zoom <= 15)
        || name.ends_with(".hm3")
        || name.ends_with(".region")
        || name == "current.json"
        || name.ends_with(".pmtiles")
}

/// Bind `dir` to `year`: if `dir/year` exists it must match `year`; if absent and the directory
/// already holds any painted, region or published payload it is refused (re-checking `dir/year`
/// once so a concurrent same-year first writer racing with payload creation succeeds); otherwise
/// `dir/year` is created atomically with no-clobber, readable permissions and parent-dir sync.
fn bind_year(dir: &Path, year: &str) -> Result<(), String> {
    let year = parse_year(year)?;
    let path = dir.join(MARKER);
    if verify_existing(dir, &year)? {
        return Ok(());
    }
    if root_entries(dir)?.iter().any(|name| is_payload_entry(name)) {
        if verify_existing(dir, &year)? {
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
    make_world_readable(temporary.as_file(), dir)?;
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

/// Bind a painted square tree to `year`.
pub fn bind_painted_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year)
}

/// Bind a zoom-8 region cache directory to `year`.
pub fn bind_regions_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year)
}

/// Bind a published tiles directory (ignoring `.pack.lock`) to `year`.
pub fn bind_tiles_dir(dir: &Path, year: &str) -> Result<(), String> {
    bind_year(dir, year)
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

        // Cross-role check: a region or tiles command cannot stamp an unmarked legacy painted tree.
        assert!(
            bind_regions_dir(&legacy_painted, "2024")
                .unwrap_err()
                .contains("without a year marker")
        );
        assert!(
            bind_tiles_dir(&legacy_painted, "2024")
                .unwrap_err()
                .contains("without a year marker")
        );
        assert!(
            bind_painted_dir(&interrupted_tiles, "2024")
                .unwrap_err()
                .contains("without a year marker")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tiles.join(MARKER))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o644);
        }
    }

    #[test]
    fn concurrent_same_year_first_writers_both_succeed_even_with_payload() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared-same");
        std::fs::create_dir_all(&shared).unwrap();
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|scope| {
            let h1 = scope.spawn(|| {
                barrier.wait();
                bind_painted_dir(&shared, "2026")?;
                std::fs::create_dir_all(shared.join("13")).map_err(|e| e.to_string())
            });
            let h2 = scope.spawn(|| {
                barrier.wait();
                bind_painted_dir(&shared, "2026")
            });
            (h1.join().unwrap(), h2.join().unwrap())
        });
        assert!(a.is_ok(), "worker 1 failed: {a:?}");
        assert!(b.is_ok(), "worker 2 failed: {b:?}");
        assert_eq!(read_year(&shared).unwrap(), "2026");
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
