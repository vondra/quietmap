//! z9-square → (continent, country, city) lookup, one record per prepared square.
//!
//! Every prepared square carries its own admin record beside its arrows, at
//! `<squares>/<square>/admin.bin`, so a paint task reads admin for exactly
//! the squares in its read ring and nothing world-wide has to travel with it.
//!
//! `square` is the square's Morton z-order id (see [`square_id`]): interleaved
//! x/y bits of the z9 tile, `0..262144`. The directory name is the decimal id.
//!
//! Record (13 bytes, little-endian):
//!
//! ```text
//! [u64 square_id, u8 continent, u8 iso0, u8 iso1, u16 city]
//! ```
//!
//! `square_id` repeats the directory name on purpose: the path anchors the
//! identity and the record proves the file was not copied in from another
//! square.
//!
//! Policy: admin assignment uses the z9 square **centroid** point-in-polygon
//! against the global CGAZ ADM0 boundaries, with interior sampling for
//! sea-centroid squares and hand-curated metro polygons. At ~78 km square
//! resolution (z9 at the equator):
//!
//!   - Interior squares fall cleanly inside one country.
//!   - Border squares pick up whichever polygon claims the centroid —
//!     acceptable approximation at z9 granularity.
//!   - Micro-states (Vatican, Monaco, ...) are absorbed into the
//!     surrounding country's polygon.
//!   - Metros tag only squares whose centroid falls inside the bounding
//!     polygon — suburbs outside the polygon correctly fall back to the
//!     country-level default.
//!

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// `[u64 square_id, u8 continent, u8 iso0, u8 iso1, u16 city]`.
pub const RECORD_SIZE: usize = 8 + 1 + 2 + 2;

/// The admin record's file name inside `<squares>/<square>/`.
pub const ADMIN_FILE_NAME: &str = "admin.bin";

/// Bits per z9 axis: 512 tiles per axis, so x/y are 9-bit values and the
/// Morton id fits in 18 bits.
const SQUARE_BITS_PER_AXIS: u32 = 9;

/// Largest valid z-order square id: both axes at 511 interleave to 18 one-bits.
pub const MAX_SQUARE_ID: i64 = (1 << (2 * SQUARE_BITS_PER_AXIS)) - 1;

/// Morton z-order id of a z9 square: bit `i` of `x` goes to bit `2i`, bit `i`
/// of `y` to bit `2i + 1`. The grid crate names the unit ([`grid::Square`],
/// [`grid::square_of`); the integer id is what the prepared tree is keyed by.
pub fn square_id(square: grid::Square) -> i64 {
    let mut id: i64 = 0;
    for i in 0..SQUARE_BITS_PER_AXIS {
        id |= (((u32::from(square.x) >> i) & 1) as i64) << (2 * i);
        id |= (((u32::from(square.y) >> i) & 1) as i64) << (2 * i + 1);
    }
    id
}

/// Inverse of [`square_id`]: `None` when the id is not a z9 Morton code
/// (negative or past [`MAX_SQUARE_ID`]).
pub fn square_from_id(id: i64) -> Option<grid::Square> {
    if !(0..=MAX_SQUARE_ID).contains(&id) {
        return None;
    }
    let mut x: u16 = 0;
    let mut y: u16 = 0;
    for i in 0..SQUARE_BITS_PER_AXIS {
        x |= (((id >> (2 * i)) & 1) as u16) << i;
        y |= (((id >> (2 * i + 1)) & 1) as u16) << i;
    }
    Some(grid::Square { x, y })
}

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

/// The path of one square's admin record inside a prepared squares tree.
pub fn cell_admin_path(square_directory: &Path, square: i64) -> PathBuf {
    square_directory
        .join(format!("{square}"))
        .join(ADMIN_FILE_NAME)
}

/// Read one square's admin record. `io::ErrorKind::NotFound` means the square
/// is outside the prepared world (no square directory at all); every other
/// error is a real fault: a prepared square directory without its record, a
/// truncated record, or one copied in from another square.
pub fn read_cell_admin(square_directory: &Path, square: i64) -> io::Result<Admin> {
    let path = cell_admin_path(square_directory, square);
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

// ─── Process-wide per-square cache ─────────────────────────────────────
///
/// tile-painter, source-reader and point-debug all benefit from the defaults
/// cascade. Rather than plumb admin by reference through the ~6-deep call
/// chain (entry point → compute_at_point → compute_roads →
/// normalize_road_segment), the process records its squares tree once at
/// startup and each square's record is read on first use and then kept.
///
/// A process that never records a tree — lib tests, point-debug — resolves
/// `Admin::UNKNOWN` everywhere and keeps the WORLD defaults arm, so lib tests
/// may assume no admin is ever visible to them. That assumption is why the
/// wiring test lives in its own integration binary
/// (`tests/admin_process_directory.rs`): filling this cache inside the lib test
/// binary flipped `none_channel_is_receiver_path_bit_identical` between the
/// WORLD and the country arm depending on when the fill landed.

#[derive(Default)]
struct AdminCache {
    square_directory: Option<PathBuf>,
    by_square: HashMap<i64, Admin>,
}

static ADMIN_CACHE: LazyLock<RwLock<AdminCache>> = LazyLock::new(RwLock::default);

const POISONED: &str = "a thread panicked while holding the admin cache";

/// Point the process at the prepared squares tree admin is read from.
/// Switching trees drops every cached record, so a re-initialised
/// source-reader can never answer from the tree it just left.
pub fn set_admin_square_directory(square_directory: &Path) {
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    if cache.square_directory.as_deref() == Some(square_directory) {
        return;
    }
    cache.square_directory = Some(square_directory.to_path_buf());
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
        if cache.square_directory.is_none() {
            return Admin::UNKNOWN;
        }
    }
    let mut cache = ADMIN_CACHE.write().expect(POISONED);
    let Some(square_directory) = cache.square_directory.clone() else {
        return Admin::UNKNOWN;
    };
    let admin = match read_cell_admin(&square_directory, square) {
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
        let root = std::env::temp_dir().join(format!("admin-missing-{}", std::process::id()));
        let square = square_id(grid::Square { x: 276, y: 173 });
        fs::create_dir_all(root.join(format!("{square}"))).unwrap();
        let error = read_cell_admin(&root, square).unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
        let outside = square_id(grid::Square { x: 0, y: 0 });
        assert_eq!(
            read_cell_admin(&root, outside).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Write one square's admin record into a squares tree.
    fn write_cell_admin(square_directory: &Path, square: i64, iso: &[u8; 2], city: u16) {
        let path = cell_admin_path(square_directory, square);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut record = Vec::with_capacity(RECORD_SIZE);
        record.extend_from_slice(&(square as u64).to_le_bytes());
        record.push(Continent::Europe as u8);
        record.extend_from_slice(iso);
        record.extend_from_slice(&city.to_le_bytes());
        fs::write(path, record).unwrap();
    }

    #[test]
    fn square_id_roundtrips_through_morton_bits() {
        for (x, y) in [(0, 0), (511, 511), (276, 173), (1, 0), (0, 1)] {
            let square = grid::Square { x, y };
            assert_eq!(square_from_id(square_id(square)), Some(square));
        }
        assert_eq!(square_from_id(-1), None);
        assert_eq!(square_from_id(MAX_SQUARE_ID + 1), None);
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
        fs::create_dir_all(tree.path().join(format!("{neighbour}"))).unwrap();
        fs::copy(
            cell_admin_path(tree.path(), dobris),
            cell_admin_path(tree.path(), neighbour),
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

    // The process-wide wiring test lives in tests/admin_process_directory.rs —
    // its own process, so the lib tests keep their "no admin is visible"
    // assumption (see the cache header above).
}
