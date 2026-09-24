//! Compute receiver enclosure from nine vector-footprint probes.

use super::{FootprintKey, ObstacleSet};
use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};

/// Receiver-local enclosure over a set of per-cell indexes — the vector twin
/// of the raster 3×3 probe (`RealRasters::building_enclosure`): fraction of 9
/// probe points at ±`ENCLOSURE`-metre offsets that sit inside a footprint
/// taller than 5 m → 0 / 1.5 / 3 dB. Same thresholds, same footprint metric
/// (a parcel split into several footprints cannot inflate it).
///
/// `own_footprint` is a façade receiver's own building: CNOSSOS §2.8 excludes
/// "reflections from the façade being considered", so a probe inside it counts
/// as open ground (the nine-probe denominator stays).
pub fn enclosure_db(
    set: &ObstacleSet,
    lat: f64,
    lon: f64,
    radius_m: f64,
    own_footprint: Option<FootprintKey>,
) -> f64 {
    let step_lat = radius_m / M_PER_DEG_LAT;
    let step_lon = radius_m / m_per_deg_lon(lat.to_radians());
    let mut built = 0u32;
    let mut scratch: Vec<(u32, u32, f32)> = Vec::new();
    for dr in [-1.0, 0.0, 1.0] {
        for dc in [-1.0_f64, 0.0, 1.0] {
            let plat = lat + dr * step_lat;
            let plon = ((lon + dc * step_lon + 180.0).rem_euclid(360.0)) - 180.0;
            if set.indexes.iter().any(|index| {
                let ignored_id = own_footprint
                    .filter(|own| own.square() == index.square())
                    .map(|own| own.id);
                index.contains_built_other_than(plat, plon, 5.0, ignored_id, &mut scratch)
            }) {
                built += 1;
            }
        }
    }
    let density = built as f64 / 9.0;
    if density > 0.5 {
        3.0
    } else if density > 0.2 {
        1.5
    } else {
        0.0
    }
}

/// Replace the raster reflection probe with exact vector-footprint enclosure.
pub struct VectorReflectionSampler<'a> {
    pub inner: &'a dyn crate::types::RasterSampler,
    pub set: &'a ObstacleSet,
    /// The building whose façade receiver this is; `None` for a point receiver.
    pub own_footprint: Option<FootprintKey>,
}

impl crate::types::RasterSampler for VectorReflectionSampler<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.inner.elevation(lat, lon)
    }
    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        self.inner.ground_g(lat, lon)
    }
    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        enclosure_db(
            self.set,
            lat,
            lon,
            crate::constants::ENCLOSURE_RADIUS_M,
            self.own_footprint,
        )
    }
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut crate::propagation::PathProfile,
    ) {
        self.inner
            .build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out)
    }
}
