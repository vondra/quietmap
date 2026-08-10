//! Compute configuration types — `ComputeConfig` toggles (terrain/screening/
//! vegetation/top-N) and the `RasterSampler` trait popup + pipeline implement.
/// Computation toggles.
#[derive(Debug, Clone)]
pub struct ComputeConfig {
    pub terrain: bool,    // terrain diffraction
    pub screening: bool,  // building screening + urban reflection
    pub vegetation: bool, // forest attenuation
    pub top_n: usize,     // full propagation for top-N candidates (rest = free-field)
    pub n_days: u16,      // number of days in aircraft dataset (for period normalization)
}

impl Default for ComputeConfig {
    fn default() -> Self {
        ComputeConfig {
            terrain: true,
            screening: true,
            vegetation: true,
            top_n: 100,
            n_days: 365,
        }
    }
}

/// Trait for raster lookups — implemented differently by popup (SRTM tiles) and pipeline (hex clips).
///
/// Path-valued queries (terrain profile, vegetation depth, ground G along a
/// path) are unified into [`build_path_profile`](RasterSampler::build_path_profile)
/// which populates a [`crate::propagation::PathProfile`]. Path-effect callers
/// in [`crate::propagation::path_effects`] read from the profile instead of
/// walking the path per-raster.
///
/// CONTRACT: every method must be VALUE-PURE — the same coordinates return the
/// same bits regardless of call order, interleaving, or calling thread.
/// Internal caching/locking is fine (the production tile stores use mutexed
/// LRU over immutable mmaps); observable state that feeds answers is not. The
/// parallel popup kernels (`compute_roads`/`compute_railways`) re-order and
/// interleave raster reads across rayon workers and are bit-reproducible only
/// under this contract; it held implicitly for every sampler before them.
pub trait RasterSampler: Send + Sync {
    fn elevation(&self, lat: f64, lon: f64) -> f64;
    fn building_height(&self, lat: f64, lon: f64) -> f64;
    fn ground_g(&self, lat: f64, lon: f64) -> f64;
    fn building_enclosure(&self, lat: f64, lon: f64) -> f64; // reflection boost 0-3 dB

    /// Populate a `PathProfile` using the unified bilateral cadence. Fills
    /// `elevation_m`, `building_h_m`, `forest_u8`, `imd_u8` at every t.
    ///
    /// Default implementation samples per-t via the other trait methods;
    /// `RealRasters` and `FusedGrid` override with a single fused loop so
    /// building/forest/IMD share tile-cache warmth per t.
    ///
    /// See `propagation::path_profile` for the canonical cadence.
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut crate::propagation::PathProfile,
    ) {
        crate::propagation::path_profile::build_default(
            self, src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out,
        );
    }

    /// Max building height along path, sampled every ~30m (matching raster cell).
    /// Returns (max_height, t_position). Pipeline overrides with tile-cached sampling.
    fn max_building_along_path(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        excl_start_m: f64,
    ) -> (f64, f64) {
        let cell_m = crate::propagation::path_profile::CELL_M;
        let step = if dist_m <= 1000.0 {
            cell_m
        } else if dist_m <= 3000.0 {
            cell_m * 3.0
        } else {
            cell_m * 6.0
        };
        let n = ((dist_m / step).ceil() as usize).clamp(2, 400);
        let mut max_bh = 0.0f64;
        let mut max_t = 0.5;
        for k in 1..n {
            let t = k as f64 / n as f64;
            if excl_start_m > 0.0 && t * dist_m < excl_start_m {
                continue;
            }
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = src_lon + t * (rcv_lon - src_lon);
            let bh = self.building_height(lat, lon);
            if bh > max_bh {
                max_bh = bh;
                max_t = t;
            }
        }
        (max_bh, max_t)
    }
}
