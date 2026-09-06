//! Strict country/city defaults from each prepared `z9/x/y/admin.bin`.

use grid::{square_from_id, square_id};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// Little-endian `[u64 Morton square_id, u8 continent, u8 iso0, u8 iso1, u16 city]`.
pub const RECORD_SIZE: usize = 8 + 1 + 2 + 2;

/// The admin record's file name beside one z9 unit's Arrow files.
pub const ADMIN_FILE_NAME: &str = "admin.bin";

/// Six major continents. Hand-coded ids must match the admin build script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Continent {
    Unknown = 0,
    Europe = 1,
    NorthAmerica = 2,
    SouthAmerica = 3,
    Asia = 4,
    Africa = 5,
    Oceania = 6,
}

impl Continent {
    pub fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Europe,
            2 => Self::NorthAmerica,
            3 => Self::SouthAmerica,
            4 => Self::Asia,
            5 => Self::Africa,
            6 => Self::Oceania,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Europe => "EU",
            Self::NorthAmerica => "NA",
            Self::SouthAmerica => "SA",
            Self::Asia => "AS",
            Self::Africa => "AF",
            Self::Oceania => "OC",
            Self::Unknown => "",
        }
    }
}

/// Admin triplet for a z9 square.
///
/// `country_iso` holds the 2-letter ISO-3166 alpha-2 code as raw bytes
/// (e.g. `*b"CZ"`, `*b"BR"`). `b"\0\0"` is the "unknown" sentinel — squares
/// whose centroid fell outside every country polygon (mostly oceanic or
/// small-island squares).
///
/// `city_id` is the metro `id`; `0` means the centroid did not fall inside
/// any metro polygon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admin {
    pub continent: Continent,
    pub country_iso: [u8; 2],
    pub city_id: u16,
}

impl Admin {
    pub const UNKNOWN: Admin = Admin {
        continent: Continent::Unknown,
        country_iso: [0, 0],
        city_id: 0,
    };

    /// Returns the ISO code as a `&str` when it is valid ASCII A-Z.
    /// `None` means the square was not classified (oceanic / missing polygon).
    pub fn country_code(&self) -> Option<&str> {
        if self.country_iso[0] == 0 && self.country_iso[1] == 0 {
            None
        } else {
            std::str::from_utf8(&self.country_iso).ok()
        }
    }
}

/// The path of one square's admin record inside a prepared year.
pub fn cell_admin_path(prepared_directory: &Path, square: i64) -> io::Result<PathBuf> {
    let square = square_from_id(square).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid z9 square id {square}"),
        )
    })?;
    Ok(prepared_directory
        .join(grid::square_name(square))
        .join(ADMIN_FILE_NAME))
}

/// Read one square's admin record. `io::ErrorKind::NotFound` means the square
/// is outside the prepared world (no square directory at all); every other
/// error is a real fault: a prepared square directory without its record, a
/// truncated record, or one copied in from another square.
pub fn read_cell_admin(prepared_directory: &Path, square: i64) -> io::Result<Admin> {
    let path = cell_admin_path(prepared_directory, square)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The DIRECTORY separates the two absences, exactly as the obstacle
            // table does: a square the Planet extract never produced has no
            // directory; a directory without the record is undelivered data.
            let directory = path.parent().expect("record path has a parent");
            return match fs::metadata(directory) {
                Err(dir_error) if dir_error.kind() == io::ErrorKind::NotFound => Err(error),
                Err(dir_error) => Err(dir_error),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "square {square} has no {} — every prepared square carries one, so this \
                         is undelivered data, not open sea",
                        path.display()
                    ),
                )),
            };
        }
        Err(error) => return Err(error),
    };
    if bytes.len() != RECORD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} holds {} bytes, expected exactly {RECORD_SIZE}",
                path.display(),
                bytes.len()
            ),
        ));
    }
    // The independently derived z9 path anchors identity; copied records fail.
    let stored = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    if stored != square as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} holds square {stored}, not {square}", path.display()),
        ));
    }
    Ok(Admin {
        continent: Continent::from_u8(bytes[8]),
        country_iso: [bytes[9], bytes[10]],
        city_id: u16::from_le_bytes([bytes[11], bytes[12]]),
    })
}

// Keep cache-initializing tests in their integration binary: setting this
// process-wide geography changes the defaults seen by unrelated lib tests.
#[derive(Default)]
struct AdminCache {
    prepared_directory: Option<PathBuf>,
    by_square: HashMap<i64, Admin>,
}

static ADMIN_CACHE: LazyLock<RwLock<AdminCache>> = LazyLock::new(RwLock::default);

const POISONED: &str = "a thread panicked while holding the admin cache";

/// Point the process at the prepared squares tree admin is read from.
/// Switching trees drops every cached record, so a re-initialised
/// source-reader can never answer from the tree it just left.
pub fn set_admin_prepared_directory(prepared_directory: &Path) {
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    if cache.prepared_directory.as_deref() == Some(prepared_directory) {
        return;
    }
    cache.prepared_directory = Some(prepared_directory.to_path_buf());
    cache.by_square.clear();
}

/// Resolve admin for a given z9 z-order square id ([`square_id`]).
/// `Admin::UNKNOWN` when no squares tree was recorded or the square is
/// outside the prepared world.
pub fn admin_for_square(square: i64) -> Admin {
    {
        let cache = ADMIN_CACHE.read().expect(POISONED);
        if let Some(admin) = cache.by_square.get(&square) {
            return *admin;
        }
        if cache.prepared_directory.is_none() {
            return Admin::UNKNOWN;
        }
    }
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    let Some(prepared_directory) = cache.prepared_directory.clone() else {
        return Admin::UNKNOWN;
    };
    let admin = match read_cell_admin(&prepared_directory, square) {
        Ok(admin) => admin,
        // A square outside the prepared world has no directory at all: that is
        // the ocean answer, not a fault. A record that exists but does not
        // read IS a fault — painting WORLD defaults over a real country
        // because a file is torn must never pass silently.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Admin::UNKNOWN,
        Err(error) => panic!("{error}"),
    };
    cache.by_square.insert(square, admin);
    admin
}

/// Resolve admin for a lat/lng by projecting to the enclosing z9 square.
/// Callers in source-reader (popup) have lat/lng from the HTTP request.
pub fn admin_for_latlng(lat: f64, lng: f64) -> Admin {
    admin_for_square(square_id(grid::square_of(lat, lng)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prepared square directory without its record is undelivered data,
    /// never the ocean answer: the reader must fail, not fall back to WORLD
    /// defaults.
    #[test]
    fn missing_record_in_an_existing_square_directory_is_a_fault() {
        let tree = tempfile::tempdir().unwrap();
        let root = tree.path();
        let square = square_id(grid::Square { x: 276, y: 173 });
        fs::create_dir_all(root.join("z9/276/173")).unwrap();
        let old_directory = root.join("admin").join(square.to_string());
        fs::create_dir_all(&old_directory).unwrap();
        fs::write(old_directory.join(ADMIN_FILE_NAME), [0; RECORD_SIZE]).unwrap();
        let error = read_cell_admin(root, square).unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    /// Write one square's admin record into a squares tree.
    fn write_cell_admin(prepared_directory: &Path, square: i64, iso: &[u8; 2], city: u16) {
        let path = cell_admin_path(prepared_directory, square).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut record = Vec::with_capacity(RECORD_SIZE);
        record.extend_from_slice(&(square as u64).to_le_bytes());
        record.push(Continent::Europe as u8);
        record.extend_from_slice(iso);
        record.extend_from_slice(&city.to_le_bytes());
        fs::write(path, record).unwrap();
    }

    #[test]
    fn continent_roundtrip() {
        for c in [
            Continent::Unknown,
            Continent::Europe,
            Continent::NorthAmerica,
            Continent::SouthAmerica,
            Continent::Asia,
            Continent::Africa,
            Continent::Oceania,
        ] {
            assert_eq!(Continent::from_u8(c as u8), c);
        }
    }

    #[test]
    fn admin_unknown_sentinel() {
        assert_eq!(Admin::UNKNOWN.country_code(), None);
        assert_eq!(Admin::UNKNOWN.continent, Continent::Unknown);
        assert_eq!(Admin::UNKNOWN.city_id, 0);
    }

    #[test]
    fn country_code_roundtrips_ascii() {
        let a = Admin {
            continent: Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        assert_eq!(a.country_code(), Some("CZ"));
    }

    #[test]
    fn read_cell_admin_returns_the_written_record() {
        let tree = tempfile::tempdir().unwrap();
        let dobris = square_id(grid::square_of(49.78, 14.17));
        write_cell_admin(tree.path(), dobris, b"CZ", 31);
        let admin = read_cell_admin(tree.path(), dobris).unwrap();
        assert_eq!(admin.country_code(), Some("CZ"));
        assert_eq!(admin.continent, Continent::Europe);
        assert_eq!(admin.city_id, 31);
    }

    #[test]
    fn read_cell_admin_rejects_a_record_from_another_square() {
        let tree = tempfile::tempdir().unwrap();
        let dobris = square_id(grid::square_of(49.78, 14.17));
        let neighbour = square_id(grid::Square { x: 0, y: 0 });
        write_cell_admin(tree.path(), dobris, b"CZ", 0);
        fs::create_dir_all(tree.path().join("z9/0/0")).unwrap();
        fs::copy(
            cell_admin_path(tree.path(), dobris).unwrap(),
            cell_admin_path(tree.path(), neighbour).unwrap(),
        )
        .unwrap();
        let error = read_cell_admin(tree.path(), neighbour).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_cell_admin_reports_an_absent_square_as_not_found() {
        let tree = tempfile::tempdir().unwrap();
        let error =
            read_cell_admin(tree.path(), square_id(grid::square_of(49.78, 14.17))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn read_cell_admin_rejects_invalid_ids_and_truncated_records() {
        let tree = tempfile::tempdir().unwrap();
        for square in [-1, grid::MAX_SQUARE_ID + 1] {
            assert_eq!(
                read_cell_admin(tree.path(), square).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let square = square_id(grid::Square { x: 276, y: 173 });
        write_cell_admin(tree.path(), square, b"CZ", 0);
        fs::write(
            cell_admin_path(tree.path(), square).unwrap(),
            [0; RECORD_SIZE - 1],
        )
        .unwrap();
        assert_eq!(
            read_cell_admin(tree.path(), square).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    // The process-wide wiring test lives in tests/admin_process_directory.rs —
    // its own process, so the lib tests keep their "no admin is visible"
    // assumption (see the cache header above).
}
