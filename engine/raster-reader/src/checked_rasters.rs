//! Operation-local validation of consumed raster channels over shared lazy caches.

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

/// One calculation owns this guard; the underlying mmap caches remain shared.
/// Numeric kernels may discard NaNs, so callers must check it before publication.
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

    fn validate(&self, lat: f64, lon: f64, elevation: f64) -> Result<f64, RasterUnavailable> {
        if elevation.is_finite() {
            return Ok(elevation);
        }
        let error = RasterUnavailable { lat, lon };
        self.first_error.lock().unwrap().get_or_insert(error);
        Err(error)
    }

    pub fn ensure_valid(&self) -> Result<(), RasterUnavailable> {
        match *self.first_error.lock().unwrap() {
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
        self.validate(
            lat,
            lon,
            self.inner
                .elevation_nearest_cached(lat, lon, cached_key, cached_tile),
        )
    }
}

impl RasterSampler for CheckedRasters<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.validate(lat, lon, self.inner.elevation(lat, lon))
            .unwrap_or(f64::NAN)
    }

    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        self.validate(lat, lon, self.inner.ground_g(lat, lon))
            .unwrap_or(f64::NAN)
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
        if let Some(index) = out
            .elevation_m
            .iter()
            .position(|elevation| !elevation.is_finite())
        {
            let t = out.t[index];
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = grid::geo::interpolate_longitude_short_arc(src_lon, rcv_lon, t);
            let _ = self.validate(lat, lon, f64::from(out.elevation_m[index]));
        }
    }
}
