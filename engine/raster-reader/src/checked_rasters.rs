//! Operation-local validation of consumed raster channels over shared lazy caches.
//!
//! A missing or corrupt raster square refuses the click that needs it, with
//! the square's coordinates named. The numeric kernels cannot carry that
//! refusal (a NaN through a line integral panics the whole server), so this
//! guard records the first fault and emits a finite fallback instead: the
//! computation always completes on finite inputs, and the caller refuses the
//! click via [`CheckedRasters::ensure_valid`] before publishing anything.
//! Healthy squares never touch the fallback and compute bit-identical levels.

use crate::{RawTile, RealRasters};
use noise_compute::propagation::PathProfile;
use noise_compute::types::RasterSampler;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
pub struct RasterUnavailable {
    lat: f64,
    lon: f64,
}

impl std::fmt::Display for RasterUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DEM or surface raster unavailable at latitude {}, longitude {}",
            self.lat, self.lon
        )
    }
}

impl std::error::Error for RasterUnavailable {}

/// Fallback elevation (m) emitted after a fault is recorded. Sea level keeps
/// every downstream kernel finite; the answer is discarded by `ensure_valid`.
const FALLBACK_ELEVATION_M: f64 = 0.0;
/// Fallback ground factor emitted after a fault. Mixed ground; discarded too.
const FALLBACK_GROUND_G: f64 = 0.5;

/// One calculation owns this guard; the underlying mmap caches remain shared.
pub struct CheckedRasters<'a> {
    inner: &'a RealRasters,
    first_error: Mutex<Option<RasterUnavailable>>,
}

impl<'a> CheckedRasters<'a> {
    pub fn new(inner: &'a RealRasters) -> Self {
        Self {
            inner,
            first_error: Mutex::new(None),
        }
    }

    fn record_fault(&self, lat: f64, lon: f64) {
        self.first_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(RasterUnavailable { lat, lon });
    }

    pub fn ensure_valid(&self) -> Result<(), RasterUnavailable> {
        match *self
            .first_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn elevation_nearest_cached(
        &self,
        lat: f64,
        lon: f64,
        cached_key: &mut (i32, i32),
        cached_tile: &mut Option<Arc<RawTile>>,
    ) -> Result<f64, RasterUnavailable> {
        let elevation = self
            .inner
            .elevation_nearest_cached(lat, lon, cached_key, cached_tile);
        if elevation.is_finite() {
            Ok(elevation)
        } else {
            self.record_fault(lat, lon);
            Err(RasterUnavailable { lat, lon })
        }
    }
}

impl RasterSampler for CheckedRasters<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        let elevation = self.inner.elevation(lat, lon);
        if elevation.is_finite() {
            elevation
        } else {
            self.record_fault(lat, lon);
            FALLBACK_ELEVATION_M
        }
    }

    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        let ground_g = self.inner.ground_g(lat, lon);
        if ground_g.is_finite() {
            ground_g
        } else {
            self.record_fault(lat, lon);
            FALLBACK_GROUND_G
        }
    }

    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        self.inner.building_enclosure(lat, lon)
    }

    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut PathProfile,
    ) {
        self.inner
            .build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out);
        // The floating planes cannot carry NaN into the kernels: record the
        // first fault's coordinates, then sanitize every plane to finite so
        // the computation completes and `ensure_valid` refuses the click.
        let mut first_bad: Option<usize> = None;
        for (index, elevation) in out.elevation_m.iter_mut().enumerate() {
            if !elevation.is_finite() {
                first_bad.get_or_insert(index);
                *elevation = FALLBACK_ELEVATION_M as f32;
            }
        }
        for canopy in out.canopy_m.iter_mut() {
            if !canopy.is_finite() {
                *canopy = 0.0;
            }
        }
        if let Some(index) = first_bad {
            let t = out.t[index];
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = grid::geo::interpolate_longitude_short_arc(src_lon, rcv_lon, t);
            self.record_fault(lat, lon);
        }
    }

    fn weather(&self, lat: f64, lon: f64) -> noise_compute::propagation::meteorology::Meteorology {
        self.inner.weather(lat, lon)
    }
}
