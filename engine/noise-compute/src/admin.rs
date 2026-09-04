//! H3R4 → (continent, country, city) lookup, one record per prepared cell.
//!
//! Every prepared cell carries its own admin record beside its arrows, at
//! `prepared/{year}/h3r4/<cell>/admin.bin` (written by
//! `scripts/build-h3-admin.ts`), so a paint task reads admin for exactly the
//! cells in its read ring and nothing world-wide has to travel with it.
//!
//! Record (13 bytes, little-endian):
//!
//! ```text
//! [u64 hex_id, u8 continent, u8 iso0, u8 iso1, u16 city]
//! ```
//!
//! `hex_id` repeats the directory name on purpose: the path anchors the
//! identity and the record proves the file was not copied in from another
//! cell.
//!
//! Policy: admin assignment uses the H3R4 hex **centroid** point-in-polygon
//! against the global CGAZ ADM0 boundaries, with interior sampling for
//! sea-centroid hexes and hand-curated metro polygons. At ~24 km hex
//! resolution:
//!
//!   - Interior hexes (~72 % globally) fall cleanly inside one country.
//!   - Border hexes pick up whichever polygon claims the centroid —
//!     acceptable approximation at H3R4 granularity.
//!   - Micro-states (Vatican, Monaco, ...) are absorbed into the
//!     surrounding country's polygon.
//!   - Metros tag only hexes whose centroid falls inside the bounding
//!     polygon — suburbs outside the polygon correctly fall back to the
//!     country-level default.
//!
//! See `scripts/build-h3-admin.ts` header for geopolitical caveats (CGAZ
//! encodes a specific view of contested boundaries).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// `[u64 hex_id, u8 continent, u8 iso0, u8 iso1, u16 city]`.
pub const RECORD_SIZE: usize = 8 + 1 + 2 + 2;

/// The admin record's file name inside `prepared/{year}/h3r4/<cell>/`.
pub const ADMIN_FILE_NAME: &str = "admin.bin";

/// Six major continents. Hand-coded ids must match
/// `scripts/build-h3-admin.ts::CONTINENT_ID`.
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

/// Admin triplet for an H3R4 cell.
///
/// `country_iso` holds the 2-letter ISO-3166 alpha-2 code as raw bytes
/// (e.g. `*b"CZ"`, `*b"BR"`). `b"\0\0"` is the "unknown" sentinel — hexes
/// whose centroid fell outside every country polygon (mostly oceanic or
/// small-island hexes).
///
/// `city_id` is the `id` field from `scripts/h3-admin-metros.json`; `0`
/// means the centroid did not fall inside any metro polygon.
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
    /// `None` means the hex was not classified (oceanic / missing polygon).
    pub fn country_code(&self) -> Option<&str> {
        if self.country_iso[0] == 0 && self.country_iso[1] == 0 {
            None
        } else {
            std::str::from_utf8(&self.country_iso).ok()
        }
    }
}

/// The path of one cell's admin record inside an h3r4 tree.
pub fn cell_admin_path(h3r4_directory: &Path, cell: u64) -> PathBuf {
    h3r4_directory
        .join(format!("{cell:015x}"))
        .join(ADMIN_FILE_NAME)
}

/// Read one cell's admin record. `io::ErrorKind::NotFound` means the cell is
/// outside the prepared world (no cell directory at all); every other error is
/// a real fault: a prepared cell directory without its record, a truncated
/// record, or one copied in from another cell.
pub fn read_cell_admin(h3r4_directory: &Path, cell: u64) -> io::Result<Admin> {
    let path = cell_admin_path(h3r4_directory, cell);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The DIRECTORY separates the two absences, exactly as the obstacle
            // table does: a cell the Planet extract never produced has no
            // directory; a directory without the record is undelivered data.
            let directory = path.parent().expect("record path has a parent");
            return match fs::metadata(directory) {
                Err(dir_error) if dir_error.kind() == io::ErrorKind::NotFound => Err(error),
                Err(dir_error) => Err(dir_error),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "cell {cell:015x} has no {} — every prepared cell carries one, so this \
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
    let stored = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    if stored != cell {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} holds cell {stored:015x}, not {cell:015x}",
                path.display()
            ),
        ));
    }
    Ok(Admin {
        continent: Continent::from_u8(bytes[8]),
        country_iso: [bytes[9], bytes[10]],
        city_id: u16::from_le_bytes([bytes[11], bytes[12]]),
    })
}

// ─── Process-wide per-cell cache ─────────────────────────────────────────
//
// tile-painter, source-reader and point-debug all benefit from the defaults
// cascade. Rather than plumb admin by reference through the ~6-deep call
// chain (entry point → compute_at_point → compute_roads →
// normalize_road_segment), the process records its h3r4 tree once at startup
// and each cell's record is read on first use and then kept.
//
// A process that never records a tree — lib tests, point-debug — resolves
// `Admin::UNKNOWN` everywhere and keeps the WORLD defaults arm, so lib tests
// may assume no admin is ever visible to them. That assumption is why the
// wiring test lives in its own integration binary
// (`tests/admin_process_directory.rs`): filling this cache inside the lib test
// binary flipped `none_channel_is_receiver_path_bit_identical` between the
// WORLD and the country arm depending on when the fill landed.

#[derive(Default)]
struct AdminCache {
    h3r4_directory: Option<PathBuf>,
    by_cell: HashMap<u64, Admin>,
}

static ADMIN_CACHE: LazyLock<RwLock<AdminCache>> = LazyLock::new(RwLock::default);

const POISONED: &str = "a thread panicked while holding the admin cache";

/// Point the process at the h3r4 tree admin is read from. Switching trees
/// drops every cached record, so a re-initialised source-reader can never
/// answer from the tree it just left.
pub fn set_admin_h3r4_directory(h3r4_directory: &Path) {
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    if cache.h3r4_directory.as_deref() == Some(h3r4_directory) {
        return;
    }
    cache.h3r4_directory = Some(h3r4_directory.to_path_buf());
    cache.by_cell.clear();
}

/// Resolve admin for a given H3R4 cell id (u64). `Admin::UNKNOWN` when no
/// h3r4 tree was recorded or the cell is outside the prepared world.
pub fn admin_for_hex(cell: u64) -> Admin {
    {
        let cache = ADMIN_CACHE.read().expect(POISONED);
        if let Some(admin) = cache.by_cell.get(&cell) {
            return *admin;
        }
        if cache.h3r4_directory.is_none() {
            return Admin::UNKNOWN;
        }
    }
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    let Some(h3r4_directory) = cache.h3r4_directory.clone() else {
        return Admin::UNKNOWN;
    };
    let admin = match read_cell_admin(&h3r4_directory, cell) {
        Ok(admin) => admin,
        // A cell outside the prepared world has no directory at all: that is
        // the ocean answer, not a fault. A record that exists but does not
        // read IS a fault — painting WORLD defaults over a real country
        // because a file is torn must never pass silently.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Admin::UNKNOWN,
        Err(error) => panic!("{error}"),
    };
    cache.by_cell.insert(cell, admin);
    admin
}

/// Resolve admin for a lat/lng by projecting to the enclosing H3R4 cell.
/// Callers in source-reader (popup) have lat/lng from the HTTP request.
pub fn admin_for_latlng(lat: f64, lng: f64) -> Admin {
    use h3o::{LatLng, Resolution};
    let Ok(ll) = LatLng::new(lat, lng) else {
        return Admin::UNKNOWN;
    };
    admin_for_hex(u64::from(ll.to_cell(Resolution::Four)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prepared cell directory without its record is undelivered data, never
    /// the ocean answer: the reader must fail, not fall back to WORLD defaults.
    #[test]
    fn missing_record_in_an_existing_cell_directory_is_a_fault() {
        let root = std::env::temp_dir().join(format!("admin-missing-{}", std::process::id()));
        let cell: u64 = 0x841e309ffffffff;
        fs::create_dir_all(root.join(format!("{cell:015x}"))).unwrap();
        let error = read_cell_admin(&root, cell).unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
        let outside: u64 = 0x841e30bffffffff;
        assert_eq!(
            read_cell_admin(&root, outside).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Write one cell's admin record into an h3r4 tree.
    fn write_cell_admin(h3r4_directory: &Path, cell: u64, iso: &[u8; 2], city: u16) {
        let path = cell_admin_path(h3r4_directory, cell);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut record = Vec::with_capacity(RECORD_SIZE);
        record.extend_from_slice(&cell.to_le_bytes());
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
        let dobris: u64 = 0x0841_e309_ffff_ffff;
        write_cell_admin(tree.path(), dobris, b"CZ", 31);
        let admin = read_cell_admin(tree.path(), dobris).unwrap();
        assert_eq!(admin.country_code(), Some("CZ"));
        assert_eq!(admin.continent, Continent::Europe);
        assert_eq!(admin.city_id, 31);
    }

    #[test]
    fn read_cell_admin_rejects_a_record_from_another_cell() {
        let tree = tempfile::tempdir().unwrap();
        let dobris: u64 = 0x0841_e309_ffff_ffff;
        let neighbour: u64 = 0x0841_e30b_ffff_ffff;
        write_cell_admin(tree.path(), dobris, b"CZ", 0);
        fs::create_dir_all(tree.path().join(format!("{neighbour:015x}"))).unwrap();
        fs::copy(
            cell_admin_path(tree.path(), dobris),
            cell_admin_path(tree.path(), neighbour),
        )
        .unwrap();
        let error = read_cell_admin(tree.path(), neighbour).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_cell_admin_reports_an_absent_cell_as_not_found() {
        let tree = tempfile::tempdir().unwrap();
        let error = read_cell_admin(tree.path(), 0x0841_e309_ffff_ffff).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    // The process-wide wiring test lives in tests/admin_process_directory.rs —
    // its own process, so the lib tests keep their "no admin is visible"
    // assumption (see the cache header above).
}
