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
    fn ground_g(&self, lat: f64, lon: f64) -> f64;
    /// Receiver reflection boost, 0-3 dB.
    ///
    /// Reflection comes from building FOOTPRINTS, so only a sampler that has
    /// them can answer: `VectorReflectionSampler` for the popup, the
    /// vector-baked `rx_refl_db` for painted tiles. Everything else has no
    /// footprints and therefore no reflection — 0 dB is the honest answer, not
    /// a fallback.
    fn building_enclosure(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }

    /// Populate a `PathProfile` using the unified bilateral cadence. Fills
    /// `elevation_m`, `forest_u8`, `imd_u8` at every t.
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
}
