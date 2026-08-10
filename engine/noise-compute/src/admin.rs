//! H3R4 → (continent, country, city) lookup.
//!
//! Loads `data/prepared/h3r4-admin.bin` (produced by
//! `scripts/build-h3-admin.ts`) into a `HashMap<u64, Admin>`. Engine callers
//! use `hex_admin(hex, &table)` to resolve the admin triplet for a single
//! H3R4 cell.
//!
//! Policy: admin assignment uses H3R4 hex **centroid** point-in-polygon
//! against Natural Earth 1:10 m country polygons and hand-curated metro
//! polygons. At ~24 km hex resolution:
//!
//!   - Interior hexes (~72 % globally) fall cleanly inside one country.
//!   - Border hexes pick up whichever polygon ray-casts first in Natural
//!     Earth's feature ordering — acceptable approximation at H3R4
//!     granularity.
//!   - Micro-states (Vatican, Monaco, ...) are absorbed into the
//!     surrounding country's polygon.
//!   - Metros tag only hexes whose centroid falls inside the bounding
//!     polygon — suburbs outside the polygon correctly fall back to the
//!     country-level default.
//!
//! See `scripts/build-h3-admin.ts` header for geopolitical caveats
//! (Natural Earth encodes a specific view of contested boundaries).
//!
//! Binary format (little-endian):
//!
//! ```text
//! bytes 0-7   : "H3ADMIN1" magic
//! bytes 8-11  : u32 count
//! records     : [u64 hex_id, u8 continent, u8 iso0, u8 iso1, u16 city] × count
//! ```

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

pub const MAGIC: &[u8; 8] = b"H3ADMIN1";
pub const RECORD_SIZE: usize = 8 + 1 + 2 + 2; // 13 bytes

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
/// whose centroid fell outside every Natural Earth country polygon (mostly
/// oceanic or small-island hexes).
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

/// Load the admin table produced by `scripts/build-h3-admin.ts`.
///
/// Returns a `HashMap<u64 h3_index, Admin>`. Typical size: ~90 k entries,
/// ~3 MB heap. Loaded once at process startup.
pub fn load_admin_table(path: &Path) -> io::Result<HashMap<u64, Admin>> {
    let bytes = fs::read(path)?;
    if bytes.len() < 12 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file too short for H3ADMIN header",
        ));
    }
    if &bytes[0..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad magic: expected {:?}, got {:?}", MAGIC, &bytes[0..8]),
        ));
    }
    let count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let expected_len = 12 + count * RECORD_SIZE;
    if bytes.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file length {} ≠ expected {} (count={}, record_size={})",
                bytes.len(),
                expected_len,
                count,
                RECORD_SIZE
            ),
        ));
    }

    let mut table = HashMap::with_capacity(count);
    let mut off = 12usize;
    for _ in 0..count {
        let hex_id = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        let continent = Continent::from_u8(bytes[off + 8]);
        let country_iso = [bytes[off + 9], bytes[off + 10]];
        let city_id = u16::from_le_bytes([bytes[off + 11], bytes[off + 12]]);
        table.insert(
            hex_id,
            Admin {
                continent,
                country_iso,
                city_id,
            },
        );
        off += RECORD_SIZE;
    }
    Ok(table)
}

/// Resolve admin for a single hex. Returns `Admin::UNKNOWN` if the hex is
/// not in the table (new hex after admin build, or the table was not loaded).
pub fn hex_admin(hex: u64, table: &HashMap<u64, Admin>) -> Admin {
    table.get(&hex).copied().unwrap_or(Admin::UNKNOWN)
}

// ─── Process-wide singleton ──────────────────────────────────────────────
//
// tile-painter, source-reader and point-debug all benefit from the
// defaults cascade. Rather than plumb the table by reference through the
// ~6-deep call chain (entry point → compute_at_point → compute_roads →
// normalize_road_segment), we keep it in a process-wide OnceLock and
// provide tiny lookup helpers. Callers MUST call `init` once at startup.
//
// OnceLock rather than Lazy so initialisation failure (missing admin.bin)
// is an explicit caller concern — tests and point-debug default to
// `Admin::UNKNOWN` and observable behaviour is unchanged.

static ADMIN_TABLE: OnceLock<HashMap<u64, Admin>> = OnceLock::new();

/// Initialise the process-wide admin table. Idempotent: subsequent calls
/// short-circuit before touching the disk and return the already-loaded
/// entry count. Recommended path: `data/prepared/h3r4-admin.bin` (built
/// by `scripts/build-h3-admin.ts`).
pub fn init_admin_table(path: &Path) -> io::Result<usize> {
    if let Some(existing) = ADMIN_TABLE.get() {
        return Ok(existing.len());
    }
    let table = load_admin_table(path)?;
    let n = table.len();
    let _ = ADMIN_TABLE.set(table);
    Ok(n)
}

/// Resolve admin for a given H3R4 cell id (u64). Returns `Admin::UNKNOWN`
/// when the table is not initialised or the cell is oceanic.
pub fn admin_for_hex(hex: u64) -> Admin {
    ADMIN_TABLE
        .get()
        .map(|t| t.get(&hex).copied().unwrap_or(Admin::UNKNOWN))
        .unwrap_or(Admin::UNKNOWN)
}

/// Resolve admin for a lat/lng by projecting to the enclosing H3R4 cell.
/// Callers in source-reader (popup) have lat/lng from the HTTP request.
pub fn admin_for_latlng(lat: f64, lng: f64) -> Admin {
    use h3o::{LatLng, Resolution};
    let Ok(ll) = LatLng::new(lat, lng) else {
        return Admin::UNKNOWN;
    };
    let cell = u64::from(ll.to_cell(Resolution::Four));
    admin_for_hex(cell)
}

/// Whether the process-wide table has been initialised. Useful in
/// `normalize_road` to debug the "every default is WORLD" symptom.
pub fn is_initialised() -> bool {
    ADMIN_TABLE.get().is_some()
}

/// Resolve the admin.bin path for an h3r4 input directory.
///
/// Per CLAUDE.md, admin.bin is year-independent (like rasters/DEM) and lives
/// at `data/prepared/h3r4-admin.bin`. Callers pass the year-scoped h3r4 dir
/// (e.g. `data/prepared/2026/h3r4`); we walk up two levels to land on
/// `data/prepared`. Falls back to the immediate parent for non-year-scoped
/// layouts (legacy / tests). Returns the first existing path; if none
/// exists, returns the year-independent location (caller logs the
/// soft-fail).
pub fn default_admin_path(h3r4_dir: &Path) -> std::path::PathBuf {
    let year_independent = h3r4_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("h3r4-admin.bin"));
    let year_scoped = h3r4_dir.parent().map(|p| p.join("h3r4-admin.bin"));
    if let Some(p) = &year_independent {
        if p.exists() {
            return p.clone();
        }
    }
    if let Some(p) = &year_scoped {
        if p.exists() {
            return p.clone();
        }
    }
    year_independent
        .or(year_scoped)
        .unwrap_or_else(|| Path::new("data/prepared/h3r4-admin.bin").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn load_admin_table_reads_real_file() {
        // Best-effort test: the file only exists after `npm run build:h3-admin`
        // has been run at least once. Skip silently otherwise so clean-checkout
        // CI does not fail.
        let path = Path::new("../../data/prepared/h3r4-admin.bin");
        if !path.exists() {
            eprintln!(
                "skipping: {} not found — run `npm run build:h3-admin`",
                path.display()
            );
            return;
        }
        let table = load_admin_table(path).expect("failed to load admin table");
        assert!(!table.is_empty(), "table should have entries");
        let known = table
            .values()
            .filter(|a| a.country_code().is_some())
            .count();
        assert!(known > 0, "at least some hexes must classify to a country");
    }

    #[test]
    fn dobris_hex_lands_in_czechia() {
        // 49.78 N, 14.17 E — a known Dobříš hex used for benchmarks.
        // H3 res-4 id "841e309ffffffff" is 15 hex chars → u64 with a leading 0.
        let path = Path::new("../../data/prepared/h3r4-admin.bin");
        if !path.exists() {
            return;
        }
        let table = load_admin_table(path).unwrap();
        let dobris: u64 = 0x0841_e309_ffff_ffff;
        let a = hex_admin(dobris, &table);
        assert_eq!(a.country_code(), Some("CZ"), "Dobris should land in CZ");
        assert_eq!(a.continent, Continent::Europe);
    }

    // The OnceLock singleton wiring test lives in tests/admin_singleton.rs —
    // an integration binary, i.e. its OWN process. In here it would initialise
    // the process-wide ADMIN_TABLE for every concurrently-running lib test,
    // and any test computing with an admin-dependent default (the road/rail
    // kernels' receiver fallback) would flip arms depending on WHEN this
    // test's init landed — a real observed flake: the rail none-channel
    // bit-identity test lost the race between its two compute calls on
    // data-carrying boxes (CI never saw it — the init test is data-gated and
    // skips there). Lib tests may assume the table is NEVER initialised.

    #[test]
    fn prague_hex_tagged_as_metro() {
        // Prague city-centre H3R4 cell — should be tagged city_id=31
        // (Prague entry in scripts/h3-admin-metros.json).
        let path = Path::new("../../data/prepared/h3r4-admin.bin");
        if !path.exists() {
            return;
        }
        let table = load_admin_table(path).unwrap();

        // Walk table and confirm at least one CZ hex has city_id == 31.
        let prague_tagged = table
            .values()
            .any(|a| a.country_code() == Some("CZ") && a.city_id == 31);
        assert!(
            prague_tagged,
            "expected at least one CZ hex tagged as city_id=31 (Prague)"
        );
    }

    #[test]
    fn hex_admin_returns_unknown_for_missing_hex() {
        let table = HashMap::new();
        assert_eq!(hex_admin(0xdead_beef_cafe, &table), Admin::UNKNOWN);
    }
}
