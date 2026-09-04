//! [`RealRasters`] — lazy mmap'd 1°×1° raster tiles, the popup + extract sampler.
//!
//! Implements [`noise_compute::types::RasterSampler`] over three [`TileStore`]s
//! (DEM, forest, IMD), each an LRU cache of mmap'd 1° tiles loaded on
//! first access. This is the global-scale reader the per-point popup and the
//! aircraft extract sample directly; the pipeline crops it into a
//! [`crate::fused_grid::FusedGrid`] for L3-resident batch compute.

use crate::tile::{DType, Interp, TileStore};
use crate::RawTile;
use noise_compute::types::RasterSampler;
use std::path::Path;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

/// Real raster data from 1°×1° tiles. Implements RasterSampler.
pub struct RealRasters {
    pub dem: TileStore,
    pub forest: TileStore,
    pub imd: TileStore,
}

impl RealRasters {
    /// Create from a release root. Tiles loaded lazily on first access.
    ///
    /// `data_dir` is the dev4 prepared release year dir (`<prepared>/2026`):
    /// DEM at `rasters/dem/*.hgt`, forest at `rasters/forest/*.raw`, IMD at
    /// `rasters/imd/*.raw` (all 1°×1° 3601² node-registered, same bytes as
    /// dev1). The dev1 `dem/copernicus` (+ `dem/srtm` fallback) tree is kept
    /// as DEM fallbacks so old checkouts keep reading.
    pub fn new(data_dir: &Path) -> Self {
        // Defaults to 32 not 12: 12 is below the working-set size of
        // any realistic extract bbox (Praha alone is ~20 1°×1° DEM
        // tiles), so the LRU evict path fires repeatedly and scans
        // every tile slot each time. 32 covers most regional bboxes
        // without code changes; operators override with the env-var
        // for global runs.
        let dem_cache_tiles = env_usize("QUIETMAP_CACHE_DEM_TILES", 32);
        let forest_cache_tiles = env_usize("QUIETMAP_CACHE_FOREST_TILES", 64);
        let imd_cache_tiles = env_usize("QUIETMAP_CACHE_IMD_TILES", 128);

        // DEM: Copernicus GLO-30 primary (.hgt) from the dev4 release tree,
        // dev1 `dem/copernicus` tree then SRTM fallback (.hgt) after it.
        let dem = TileStore::new(
            data_dir.join("rasters/dem"),
            3601,
            DType::I16BE,
            Interp::Bilinear,
            0.0,
            ".hgt",
            dem_cache_tiles,
        )
        .with_alt_dir(data_dir.join("dem/copernicus"), ".hgt")
        .with_alt_dir(data_dir.join("dem/srtm"), ".hgt");

        // Forest cover: u8 (0/100%), 3601×3601 (WorldCover 30m), nearest-neighbor
        let forest = TileStore::new(
            data_dir.join("rasters/forest"),
            3601,
            DType::U8,
            Interp::Nearest,
            0.0,
            ".raw",
            forest_cache_tiles,
        );

        // IMD ground type: u8 (0-100 imperviousness), 3601×3601 (30m), bilinear.
        // Missing-tile default 100 = hard (G=0): the WorldCover converter emits
        // an IMD tile for every land tile, so a tile absent from the complete
        // set is open ocean — acoustically hard water (ISO 9613-2, audit B3).
        // Caveat: on hosts with a partial tree (e.g. only 34–59°N plus
        // synced Scandinavia) missing northern LAND tiles read hard too;
        // the production host's complete tree is the truth.
        let imd = TileStore::new(
            data_dir.join("rasters/imd"),
            3601,
            DType::U8,
            Interp::Bilinear,
            100.0,
            ".raw",
            imd_cache_tiles,
        );

        RealRasters { dem, forest, imd }
    }

    /// Pre-load all tiles covering a bounding box. Call before rayon par_iter to avoid file opens;
    /// hot sequential walks still use each [`TileStore`](crate::tile::TileStore)'s cached sampler
    /// to avoid per-sample cache-slot locking and LRU updates.
    pub fn preload_bbox(&self, lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) {
        self.dem.preload_bbox(lat_min, lat_max, lon_min, lon_max);
        self.forest.preload_bbox(lat_min, lat_max, lon_min, lon_max);
        self.imd.preload_bbox(lat_min, lat_max, lon_min, lon_max);
    }

    /// Pre-load only DEM tiles covering a bounding box. NPD aircraft heatmap paths need receiver altitude
    /// and terrain AGL gates, but do not consume forest or IMD rasters.
    pub fn preload_dem_bbox(&self, lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) {
        self.dem.preload_bbox(lat_min, lat_max, lon_min, lon_max);
    }

    /// Check if any real raster data is available.
    pub fn has_data(&self) -> bool {
        // Quick check: try sampling a known CZ point
        let elev = self.dem.sample(49.195, 16.608);
        elev != 0.0
    }
}

impl RealRasters {
    /// Nearest-neighbor DEM lookup — for aircraft-extract Stage 1 +
    /// Stage 2A only. ~3-4× cheaper per lookup than `elevation()`
    /// because it skips the 4-pixel bilinear blend. Acoustically safe
    /// for the AGL-gate path: gates have 15-30 m slack everywhere
    /// except the phase seed at 7 620 m, GROUND_STALE_MAX_AGL_M, and
    /// RAW_GROUND_FLAG_MAX_AGL_M.
    ///
    /// Deliberately NOT on the `RasterSampler` trait — the trait is
    /// the popup contract and stays bilinear for terrain profile
    /// continuity.
    pub fn elevation_nearest(&self, lat: f64, lon: f64) -> f64 {
        self.dem.sample_with(lat, lon, Interp::Nearest)
    }

    /// Caller-threaded NN DEM lookup. Pairs with
    /// [`elevation_nearest`]: when a caller (Stage 1 per-flight loop,
    /// Stage 2A per-sub-segment) sweeps many points likely to fall in
    /// the same 1° tile, threading a stack `(cached_key, cached_tile)`
    /// pair across calls skips the per-tile mutex AND the global
    /// `use_counter` atomic on cache hits.
    ///
    /// Initialize both with `(i32::MIN, i32::MIN)` and `None`; the
    /// method updates them in place.
    pub fn elevation_nearest_cached(
        &self,
        lat: f64,
        lon: f64,
        cached_key: &mut (i32, i32),
        cached_tile: &mut Option<std::sync::Arc<RawTile>>,
    ) -> f64 {
        self.dem
            .sample_cached_with(lat, lon, Interp::Nearest, cached_key, cached_tile)
    }
}

impl RasterSampler for RealRasters {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.dem.sample(lat, lon)
    }

    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        // IMD 0=natural(soft), 100=impervious(hard)
        // G: 0=hard, 1=soft → G = 1.0 - IMD/100
        // Missing-tile default is 100 (ocean → hard water, G=0), so no special
        // case needed — land tiles always exist in the converted set.
        // WHY no conditional: IMD=0 means fully soft ground (forest, meadow) → G=1.0.
        // Old code returned 0.5 for IMD=0, halving ground attenuation in rural areas.
        let imd = self.imd.sample(lat, lon);
        (1.0 - imd / 100.0).clamp(0.0, 1.0)
    }

    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut noise_compute::propagation::PathProfile,
    ) {
        out.clear();
        out.dist_m = dist_m;
        out.src_lat = src_lat;
        out.src_lon = src_lon;
        out.rcv_lat = rcv_lat;
        out.rcv_lon = rcv_lon;

        noise_compute::propagation::path_profile::fill_t_values(dist_m, &mut out.t);

        let n = out.t.len();
        out.elevation_m.reserve(n);
        out.forest_u8.reserve(n);
        out.imd_u8.reserve(n);

        // Per-raster tile caches — each warms on the first sample in a tile,
        // stays warm while consecutive samples fall in the same 1° tile.
        let mut dem_key = (i32::MIN, i32::MIN);
        let mut dem_tile = None;
        let mut for_key = (i32::MIN, i32::MIN);
        let mut for_tile = None;
        let mut imd_key = (i32::MIN, i32::MIN);
        let mut imd_tile = None;

        for &t in &out.t {
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = src_lon + t * (rcv_lon - src_lon);
            let elev = self
                .dem
                .sample_cached(lat, lon, &mut dem_key, &mut dem_tile);
            let fr = self
                .forest
                .sample_cached(lat, lon, &mut for_key, &mut for_tile);
            let imd = self
                .imd
                .sample_cached(lat, lon, &mut imd_key, &mut imd_tile);
            out.elevation_m.push(elev as f32);
            out.forest_u8.push(fr.clamp(0.0, 255.0) as u8);
            out.imd_u8.push(imd.clamp(0.0, 255.0) as u8);
        }

        // The bilateral adaptive cadence (dense near endpoints, coarse mid-path)
        // IS the sampling strategy. The P3 mid-path peak augmentation that
        // re-scanned every coarse gap at 30 m was removed: it re-walked the very
        // terrain the cadence deliberately coarsens, undercutting the cadence's
        // purpose, for a refinement the cadence already largely captures.
        out.step_m_med = noise_compute::propagation::path_profile::median_step_m(&out.t, dist_m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::types::RasterSampler;
    use std::path::PathBuf;

    fn test_root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qm-rasters-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Uniform N00E000 DEM tile accepted by a 3601 store via the legacy
    /// 1201² SRTM exception (2.9 MB, not 26 MB).
    fn write_dem_tile(dir: &Path, value: i16) {
        std::fs::create_dir_all(dir).unwrap();
        let bytes: Vec<u8> = value.to_be_bytes().repeat(1201 * 1201);
        std::fs::write(dir.join("N00E000.hgt"), bytes).unwrap();
    }

    fn write_u8_tile(dir: &Path, value: u8) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("N00E000.raw"), vec![value; 3601 * 3601]).unwrap();
    }

    /// The dev4 release tree (`rasters/dem`, `rasters/forest`, `rasters/imd`
    /// under `<prepared>/2026`) is read as the primary layout.
    #[test]
    fn dev4_rasters_layout_is_primary() {
        let root = test_root("dev4");
        write_dem_tile(&root.join("rasters/dem"), 42);
        write_u8_tile(&root.join("rasters/forest"), 0);
        write_u8_tile(&root.join("rasters/imd"), 0);
        let r = RealRasters::new(&root);
        assert_eq!(r.elevation(0.5, 0.5), 42.0);
        assert_eq!(r.forest.sample(0.5, 0.5), 0.0);
        assert_eq!(r.ground_g(0.5, 0.5), 1.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The dev1 `dem/copernicus` tree still serves DEM when the dev4
    /// `rasters/dem` dir has no tile; absent forest/IMD read as defaults.
    #[test]
    fn dev1_dem_copernicus_layout_is_fallback() {
        let root = test_root("dev1");
        write_dem_tile(&root.join("dem/copernicus"), 42);
        let r = RealRasters::new(&root);
        assert_eq!(r.elevation(0.5, 0.5), 42.0);
        assert_eq!(r.ground_g(0.5, 0.5), 0.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A release root with no tiles at all reads as empty ocean (elev 0,
    /// hard ground), never panics.
    #[test]
    fn missing_tiles_read_as_empty_ocean() {
        let root = test_root("empty");
        std::fs::create_dir_all(&root).unwrap();
        let r = RealRasters::new(&root);
        assert_eq!(r.elevation(0.5, 0.5), 0.0);
        assert_eq!(r.ground_g(0.5, 0.5), 0.0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
