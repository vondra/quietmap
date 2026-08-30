//! Proof that a shard-less H3 R4 cell is INGESTED-EMPTY rather than
//! never-staged — the fact that lets the vector-building loaders keep
//! vector mode for rings containing such cells instead of silently
//! falling back to the building raster (the all-or-raster policy,
//! gg review 2026-07-28).
//!
//! WHY a manifest and not the shards: the world ingest
//! (`scripts/obstacles/ingest-overture-obstacles.py`) writes a shard only
//! for cells that received ≥1 footprint — an empty cell leaves NO trace,
//! so "no shards" is ambiguous between "covered and empty" (the common
//! case: rural cells inside a fully ingested degree tile) and "not yet
//! staged". The driving state `ingest-world-incremental.sh` keeps,
//! `.ingested-tiles` (one `N50E014`-form name per ingested 1-degree
//! Overture tile), is exactly the missing evidence: a cell whose every
//! overlapped degree tile is listed has been provably swept and contributed
//! zero footprints. Our building raster derives from the SAME Overture
//! release, so the raster fallback for such a cell would add nothing —
//! vector mode with no index for it is acoustically identical and reads no
//! raster at all.
//!
//! Coverage is CONSERVATIVE by construction: the tile set comes from the
//! cell boundary's bounding box (may include tiles the hexagon never
//! touches), polar pentagons are refused outright (their vertex bbox spans
//! only part of the longitudes that belong to the cell), and ANY unlisted
//! tile — including ocean tiles that carry no Overture parquet at all, so
//! coast-hugging cells often stay unproven — keeps the old raster fallback.
//! A manifest that is absent or unreadable (e.g. a Vast worker that staged
//! only the h3r4 tree) means coverage UNKNOWN: the loaders keep today's
//! behavior there.
//!
//! OPERATIONAL INVARIANT (unverified at load time, /gg both-reviewer
//! finding): the raster-building pipeline and this ingest must stay on the
//! SAME Overture release. A building-raster restage from a NEWER release
//! MUST delete `.ingested-tiles` (and re-run the ingest) in the same
//! change — otherwise a cell empty in the old release and built-up in the
//! new one would read manifest-proven-empty while the raster channel has
//! buildings. Re-ingesting a tile keeps this safe by removing its manifest
//! line before unlinking shards (ingest-overture-obstacles.py reconcile
//! step); full release-id pinning of the manifest against the raster
//! provenance stamp is the recommended follow-up.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use h3o::CellIndex;

/// Candidate paths for `.ingested-tiles` given a region's `h3r4` directory.
/// First existing file wins (`load_for_h3r4`).
///
/// Sahara halo of `843e191ffffffff`: three neighbours have no shard (desert,
/// ingest wrote nothing). The proof is this 120 kB list, not the 309 GB
/// staging tree. he84 keeps it at `data/source/enrichment/…`; home boxes
/// read `/mnt/synology/enrichment/…` next to `2026/h3r4`.
pub fn ingested_tiles_paths(h3r4_dir: &Path) -> Vec<PathBuf> {
    const REL: &str = "enrichment/global/overture-obstacles/.ingested-tiles";
    const SOURCE_REL: &str = "source/enrichment/global/overture-obstacles/.ingested-tiles";
    let mut paths = Vec::new();
    let mut push = |p: PathBuf| {
        if !paths.iter().any(|existing| existing == &p) {
            paths.push(p);
        }
    };
    if let Some(root) = h3r4_dir.ancestors().nth(3) {
        push(root.join(REL));
        push(root.join(SOURCE_REL));
    }
    if let Some(root) = h3r4_dir.ancestors().nth(2) {
        push(root.join(REL));
    }
    paths
}

/// Load the ingest manifest for a painter/popup `h3r4` root, or `None` if
/// none of the candidate paths exist (coverage unknown).
pub fn load_for_h3r4(h3r4_dir: &Path) -> Option<&'static IngestManifest> {
    ingested_tiles_paths(h3r4_dir)
        .into_iter()
        .find_map(|path| IngestManifest::load_cached(&path))
}

/// The parsed `.ingested-tiles` manifest.
pub struct IngestManifest {
    tiles: HashSet<String>,
}

/// One manifest per distinct path for the process lifetime (the file changes
/// only when a new world ingest runs — months). Leaked deliberately: the
/// distinct-path count is one in production and a handful under test.
fn cache() -> &'static Mutex<std::collections::HashMap<PathBuf, &'static IngestManifest>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, &'static IngestManifest>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

impl IngestManifest {
    /// Parse the manifest at `path`. `None` ⇒ absent or unreadable ⇒
    /// coverage unknown (callers keep the raster fallback for shard-less
    /// cells); malformed lines are skipped loudly on stderr, never guessed
    /// around — a half-parsed manifest must not claim coverage it lost.
    pub fn load_cached(path: &Path) -> Option<&'static IngestManifest> {
        if !path.is_file() {
            return None;
        }
        let mut cache = cache().lock().expect("ingest-manifest cache poisoned");
        if let Some(held) = cache.get(path) {
            return Some(held);
        }
        let text = std::fs::read_to_string(path).ok()?;
        let mut tiles = HashSet::new();
        for line in text.lines() {
            let name = line.trim();
            if is_valid_tile_name(name) {
                tiles.insert(name.to_owned());
            } else if !name.is_empty() {
                eprintln!(
                    "obstacle ingest manifest {}: unrecognized line {name:?} — skipped",
                    path.display()
                );
            }
        }
        let leaked: &'static IngestManifest = Box::leak(Box::new(IngestManifest { tiles }));
        cache.insert(path.to_owned(), leaked);
        Some(leaked)
    }

    /// Is every 1-degree tile overlapped by `cell`'s bounding box listed as
    /// ingested? Conservative in both directions — see the module doc —
    /// EXCEPT it must never be asked about polar cells (below).
    pub fn covers_cell(&self, cell: CellIndex) -> bool {
        let (lat_min, lat_max, lon_min, lon_max) = cell_boundary_bbox(cell);
        // Polar pentagons: the vertex bbox spans only PART of the longitudes
        // that belong to the cell (the pole itself touches every meridian),
        // so tile enumeration over it is NOT conservative — a manifest
        // listing just the vertex-span tiles would wrongly prove coverage
        // (/gg Codex finding 2 vs Kimi's fail-safe read; Codex is right).
        // Guard by latitude: beyond ±89° the only R4 cells are the polar
        // pentagons; refuse coverage there (raster fallback — ice/ocean,
        // zero buildings, no cost).
        if lat_max > 89.0 || lat_min < -89.0 {
            return false;
        }
        let lat_s = lat_min.floor() as i32;
        let lat_n = lat_max.floor() as i32;
        let lon_w = lon_min.floor() as i32;
        let lon_e = lon_max.floor() as i32;
        for lat in lat_s..=lat_n {
            for lon in lon_w..=lon_e {
                if !self.tiles.contains(&degree_tile_name(lat, lon)) {
                    return false;
                }
            }
        }
        true
    }
}

/// `N50E014`-form name of the 1-degree tile with SW corner `(lat, lon)`.
fn degree_tile_name(lat_floor: i32, lon_floor: i32) -> String {
    let ns = if lat_floor >= 0 { 'N' } else { 'S' };
    let ew = if lon_floor >= 0 { 'E' } else { 'W' };
    format!("{ns}{:02}{ew}{:03}", lat_floor.abs(), lon_floor.abs())
}

fn is_valid_tile_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 7
        && matches!(bytes[0], b'N' | b'S')
        && bytes[1..3].iter().all(u8::is_ascii_digit)
        && matches!(bytes[3], b'E' | b'W')
        && bytes[4..7].iter().all(u8::is_ascii_digit)
}

/// Lat/lon bounding box of the cell's boundary ring.
fn cell_boundary_bbox(cell: CellIndex) -> (f64, f64, f64, f64) {
    let mut lat_min = f64::MAX;
    let mut lat_max = f64::MIN;
    let mut lon_min = f64::MAX;
    let mut lon_max = f64::MIN;
    for ll in cell.boundary().iter() {
        let (lat, lon) = (ll.lat_radians().to_degrees(), ll.lng_radians().to_degrees());
        lat_min = lat_min.min(lat);
        lat_max = lat_max.max(lat);
        lon_min = lon_min.min(lon);
        lon_max = lon_max.max(lon);
    }
    (lat_min, lat_max, lon_min, lon_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_at(lat: f64, lon: f64) -> CellIndex {
        h3o::LatLng::new(lat, lon)
            .unwrap()
            .to_cell(h3o::Resolution::Four)
    }

    #[test]
    fn tile_name_forms() {
        assert_eq!(degree_tile_name(50, 14), "N50E014");
        assert_eq!(degree_tile_name(0, 13), "N00E013");
        assert_eq!(degree_tile_name(-1, 6), "S01E006");
        assert_eq!(degree_tile_name(-23, -46), "S23W046");
        assert!(is_valid_tile_name("N50E014"));
        assert!(!is_valid_tile_name("N50E14"));
        assert!(!is_valid_tile_name(""));
    }

    #[test]
    fn cover_requires_every_overlapped_tile() {
        // Praha cell: spans N49E014 + N50E013/14 + N49E013-ish bbox tiles.
        let cell = cell_at(50.08, 14.44);
        let with_all = IngestManifest {
            tiles: bbox_tiles(cell).into_iter().collect(),
        };
        assert!(with_all.covers_cell(cell));
        let mut minus_one = with_all.tiles.clone();
        let victim = with_all.tiles.iter().next().unwrap().clone();
        minus_one.remove(&victim);
        assert!(!IngestManifest { tiles: minus_one }.covers_cell(cell));
    }

    #[test]
    fn polar_and_dateline_cells_refuse_coverage() {
        // Dateline straddle: bbox lon range ≈ global → some tile always
        // missing → false. (Fixture manifest holding EVERY tile would be
        // needed to flip it; assert the conservative default instead.)
        let dateline = cell_at(49.5, 179.999);
        let sparse = IngestManifest {
            tiles: [degree_tile_name(49, 179), degree_tile_name(49, -180)]
                .into_iter()
                .collect(),
        };
        assert!(!sparse.covers_cell(dateline));
        // Polar pentagon: even a manifest holding the vertex-span tiles
        // must refuse (the pole belongs to every meridian).
        let north = cell_at(89.99, 0.0);
        let mut greedy = HashSet::new();
        for lon in -180..180 {
            for lat in 84..90 {
                greedy.insert(degree_tile_name(lat, lon));
            }
        }
        assert!(!IngestManifest { tiles: greedy }.covers_cell(north));
    }

    #[test]
    fn absent_manifest_is_none() {
        assert!(IngestManifest::load_cached(Path::new("/nonexistent/.ingested-tiles")).is_none());
    }

    #[test]
    fn h3r4_path_walk_covers_he84_and_nas_layouts() {
        let he84 = ingested_tiles_paths(Path::new("/quietmap/data/prepared/2026/h3r4"));
        assert!(he84.iter().any(|p| p
            == Path::new(
                "/quietmap/data/source/enrichment/global/overture-obstacles/.ingested-tiles"
            )));
        let nas = ingested_tiles_paths(Path::new("/mnt/synology/2026/h3r4"));
        assert!(nas.iter().any(|p| p
            == Path::new("/mnt/synology/enrichment/global/overture-obstacles/.ingested-tiles")));
    }

    fn bbox_tiles(cell: CellIndex) -> Vec<String> {
        let (lat_min, lat_max, lon_min, lon_max) = cell_boundary_bbox(cell);
        let mut out = Vec::new();
        for lat in lat_min.floor() as i32..=lat_max.floor() as i32 {
            for lon in lon_w(lon_min)..=(lon_e(lon_max)) {
                out.push(degree_tile_name(lat, lon));
            }
        }
        fn lon_w(lon_min: f64) -> i32 {
            lon_min.floor() as i32
        }
        fn lon_e(lon_max: f64) -> i32 {
            lon_max.floor() as i32
        }
        out
    }
}
