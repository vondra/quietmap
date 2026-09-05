//! [`RealRasters`] — lazy mmap'd native-lattice z9 windows for popup and aircraft sampling.
//!
//! Implements [`noise_compute::types::RasterSampler`] over three [`TileStore`]s
//! (DEM, forest, IMD), each a byte-bounded LRU cache of mmap'd windows loaded on
//! first access. This is the global-scale reader the per-point popup and the
//! aircraft extract sample directly; the pipeline crops it into a
//! [`crate::fused_grid::FusedGrid`] for L3-resident batch compute.

use crate::channel::Channel;
use crate::tile::{Interp, TileStore};
use crate::RawTile;
use noise_compute::types::RasterSampler;
use std::path::Path;

/// Real raster data from native-lattice z9 windows. Implements RasterSampler.
pub struct RealRasters {
    pub dem: TileStore,
    pub forest: TileStore,
    pub imd: TileStore,
}

impl RealRasters {
    /// Open channel publication coverage; mmap data only on first access.
    /// Missing channels stay unavailable, independently, until they are consumed.
    pub fn new(data_dir: &Path) -> Self {
        // Byte bounds accommodate two 91 MB polar DEM windows without allowing
        // a global flight sweep to retain every visited mmap (768 MiB total).
        let cache_bytes = 256 * 1024 * 1024;
        Self {
            dem: TileStore::new(data_dir, Channel::Dem, cache_bytes),
            forest: TileStore::new(data_dir, Channel::Forest, cache_bytes),
            imd: TileStore::new(data_dir, Channel::Imd, cache_bytes),
        }
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

    /// A complete DEM channel can serve worldwide aircraft preprocessing independently.
    pub fn has_data(&self) -> bool {
        self.dem.has_complete_coverage()
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
    /// the same z9 window, threading a stack `(cached_key, cached_tile)`
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
        // Only catalog-declared ocean is 100; unknown or corrupt data remains NaN.
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
        // stays warm while consecutive samples fall in the same z9 window.
        let mut dem_key = (i32::MIN, i32::MIN);
        let mut dem_tile = None;
        let mut for_key = (i32::MIN, i32::MIN);
        let mut for_tile = None;
        let mut imd_key = (i32::MIN, i32::MIN);
        let mut imd_tile = None;

        for &t in &out.t {
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = grid::geo::interpolate_longitude_short_arc(src_lon, rcv_lon, t);
            let elev = self
                .dem
                .sample_cached(lat, lon, &mut dem_key, &mut dem_tile);
            let fr = self
                .forest
                .sample_cached(lat, lon, &mut for_key, &mut for_tile);
            let imd = self
                .imd
                .sample_cached(lat, lon, &mut imd_key, &mut imd_tile);
            // PathProfile's byte channels cannot carry NaN. Preserve an invalid
            // consumed channel in its floating plane for the operation guard.
            out.elevation_m.push(if fr.is_finite() && imd.is_finite() {
                elev as f32
            } else {
                f32::NAN
            });
            out.forest_u8.push(fr as u8);
            out.imd_u8.push(imd as u8);
        }

        // The bilateral adaptive cadence (dense near endpoints, coarse mid-path)
        // IS the sampling strategy. The P3 mid-path peak augmentation that
        // re-scanned every coarse gap at 30 m was removed: it re-walked the very
        // terrain the cadence deliberately coarsens, undercutting the cadence's
        // purpose, for a refinement the cadence already largely captures.
        out.step_m_med = noise_compute::propagation::path_profile::median_step_m(&out.t, dist_m);
    }
}
