//! Point-source (industrial + building) scatter onto a Web Mercator base-zoom tile: a
//! thin entry point over the generic [`crate::scatter_band`] kernel with
//! [`PointGeometry`]. The shared machinery (receiver-block parallelism,
//! terrain ray-march, `max(A_gr, A_bar)` assembly, 3-period accumulation, and
//! the ordinary receiver-major byte-space interval) lives in `scatter_band`.
//! With `SURFACE_BUDGET_ETA=0`, a full-tile point scatter uses its exact
//! source-major bypass and computes every admitted pair; selected receivers and
//! the direct industrial surrogate retain the ordinary receiver-major path. The
//! point physics of the popup oracle `noise_compute::compute_point_sources` is
//! `PointGeometry::pixel`:
//!
//! * ISO 9613-2 SPHERICAL divergence `20·log10(d_slant)+11` (a line source is
//!   `10·log10(2π·d)`; there is no finite-line correction here).
//! * Free-field audibility pre-gate (`geo::below_free_field_threshold`) — a real
//!   per-pixel cull that drops a receiver before any path work, matching the
//!   oracle so the scatter culls the same sources the popup does.
//! * Exclusion radius: the source's own footprint shrinks the effective
//!   propagation distance (`geo::effective_area_source_dist`) and is passed to
//!   screening so footprint buildings are not counted as a barrier.
//! * Ground attenuation uses the same path-profile CNOSSOS evaluator as the
//!   line kernel and popup oracle; unlike a bridge line, a point never forces
//!   hard `G=0`.
//!
//! Per-period steady power accumulates into [`TileAccumulator`]; the caller
//! collapses with the surface (no time-division) Lden collapse.

use noise_compute::propagation::geo::{
    below_free_field_threshold, effective_area_source_dist, flat_dist, reach_box_half_extents_deg,
    slant_dist,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::propagation::path_profile::CoarseMid;
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::FusedTileZ13;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::scatter_band::{
    coarse_mid_cfg, lat_to_py, lon_to_px, scatter_tile_with_cfg as band_scatter_tile_with_cfg,
    scatter_tile_with_cfg_and_options as band_scatter_tile_with_cfg_and_options, PixelGeometry,
    PixelTerms, PreparedSource, ScatterStats, LDEN_WEIGHTS, NUM_BANDS,
};
use crate::source_point::PointRow;

#[derive(Debug, Clone, Copy, Default)]
pub struct PointScatterStats {
    pub rows: usize,
    pub path_calls: u64,
    pub skipped_calls: u64,
    /// (source, receiver) pairs priced — the skip fraction's denominator.
    pub pairs: u64,
    /// Pairs the walk actually computed — the M3 walked-fraction census.
    pub walked_pairs: u64,
    /// Ray-march cadence samples (×4 = raster cell reads).
    pub raster_samples: u64,
}

impl From<ScatterStats> for PointScatterStats {
    fn from(s: ScatterStats) -> Self {
        Self {
            rows: s.rows,
            path_calls: s.path_calls,
            skipped_calls: s.skipped_calls,
            pairs: s.pairs,
            walked_pairs: s.walked_pairs,
            raster_samples: s.raster_samples,
        }
    }
}

/// Scatter every point source onto `tile`, accumulating per-period steady power.
///
/// `barriers` is the tile's noise-wall slice prepared by
/// `source_loader_barrier::BarrierData::for_tile` (sorted ascending,
/// conservative `dist_m` — see the `types::Barrier` contract). Barriers are
/// real obstacles even inside the source's exclusion radius (the radius only
/// suppresses the source's own vector footprint).
pub fn scatter_tile(
    tile: &FusedTileZ13,
    points: &[PointRow],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    accum: &mut TileAccumulator,
) -> PointScatterStats {
    scatter_tile_with_cfg(tile, points, barriers, obstacles, accum, coarse_mid_cfg())
}

/// [`scatter_tile`] with the coarse-middle cadence passed EXPLICITLY (bypassing
/// the process-wide env read) — the noise-floor harness renders exact (`None`)
/// and coarse point fields in one process. Production uses [`scatter_tile`].
pub fn scatter_tile_with_cfg(
    tile: &FusedTileZ13,
    points: &[PointRow],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
) -> PointScatterStats {
    band_scatter_tile_with_cfg(
        &PointGeometry { points },
        tile,
        barriers,
        obstacles,
        points.len(),
        accum,
        cfg,
    )
    .into()
}

/// Point-layer W1 direct-local surrogate (industrial, building): use the point geometry and a fixed
/// M3 loose path floor, but skip profile, terrain, obstacle, and barrier work.
/// The caller keeps this behind the explicit W1 candidate switch; the ordinary
/// [`scatter_tile`] and popup paths never call it.
pub(crate) fn scatter_tile_point_direct(
    tile: &FusedTileZ13,
    points: &[PointRow],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    accum: &mut TileAccumulator,
) -> PointScatterStats {
    band_scatter_tile_with_cfg_and_options(
        &PointGeometry { points },
        tile,
        barriers,
        obstacles,
        points.len(),
        accum,
        coarse_mid_cfg(),
        None,
        Some(0.0),
        None,
    )
    .into()
}

/// Exact point-layer scatter over a compact receiver index list. Each
/// selected pixel uses the unchanged point physical evaluator, but the generic
/// block/M3 setup is not constructed for any unselected receiver.
pub(crate) fn scatter_tile_point_exact_receivers(
    tile: &FusedTileZ13,
    points: &[PointRow],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    accum: &mut TileAccumulator,
    receivers: &[usize],
) -> PointScatterStats {
    band_scatter_tile_with_cfg_and_options(
        &PointGeometry { points },
        tile,
        barriers,
        obstacles,
        points.len(),
        accum,
        // W1 exact receivers must use exact terrain cadence. Keep the
        // process-wide coarse cadence for ordinary industrial, building, and
        // ground scatter paths.
        None,
        None,
        None,
        Some(receivers),
    )
    .into()
}

/// A point's tile-pixel reach box + Lden-weighted emission spectrum + a borrow of
/// its row, precomputed once so the per-block loop is a cheap reach-box ∩ block
/// clip + sweep.
pub(crate) struct PreparedPoint<'a> {
    point: &'a PointRow,
    py0: usize,
    py1: usize,
    px0: usize,
    px1: usize,
    /// Source altitude — pixel-independent, so resolved once here rather than
    /// re-sampled in every receiver block the point's reach spans.
    src_alt: f64,
    emission_lden: [f64; NUM_BANDS],
}

impl PreparedSource for PreparedPoint<'_> {
    #[inline]
    fn reach_box(&self) -> (usize, usize, usize, usize) {
        (self.py0, self.py1, self.px0, self.px1)
    }
    #[inline]
    fn emission_lin(&self) -> &[[f32; NUM_BANDS]; NUM_PERIODS] {
        &self.point.emission_lin
    }
    #[inline]
    fn emission_lden(&self) -> &[f64; NUM_BANDS] {
        &self.emission_lden
    }
    #[inline]
    fn block_constant_source_latlon(&self) -> Option<(f64, f64)> {
        // A point source IS its characteristic point: every receiver's
        // profile starts here, so the M3 ground-bound chunk maxima are
        // receiver-independent up to the receiver block's extent.
        Some((self.point.lat, self.point.lon))
    }
}

/// ISO 9613-2 point-source physics (industrial + building), matching the popup
/// oracle `noise_compute::compute_point_sources`: spherical divergence
/// `20·log10(d_slant)+11`, a free-field audibility pre-gate (a real per-pixel
/// cull), an exclusion radius (the footprint shrinks the effective distance and
/// is excluded from screening), path-profile CNOSSOS ground, and the source as
/// the profile sample point. The borrowed `points` slice is the source rows the
/// prepare phase clips.
pub(crate) struct PointGeometry<'a> {
    pub(crate) points: &'a [PointRow],
}

impl<'a> PixelGeometry for PointGeometry<'a> {
    type Prep = PreparedPoint<'a>;

    #[inline]
    fn exact_walk_order_is_stable(&self) -> bool {
        true
    }

    #[inline]
    fn cache_pixel_terms(&self) -> bool {
        true
    }

    fn prepare(&self, tile: &FusedTileZ13, prep: &mut Vec<PreparedPoint<'a>>) {
        let bbox = &tile.bbox;
        prep.extend(self.points.iter().filter_map(|point| {
            if point
                .emission_lin
                .iter()
                .all(|p| p.iter().all(|&e| e <= 0.0))
            {
                return None;
            }
            let reach = point.max_distance_m;
            // Reach disk in degrees (coarse — the per-pixel distance gate is exact).
            // ONE place for this geometry: cos at the POLEWARD EDGE, clamped
            // where `m_per_deg_lon` clamps, so the box always contains the disk.
            // The `cos(point.lat).max(0.2)` this replaces under-covered longitude
            // by up to 2.3x above 78.46 deg and dropped audible sources there.
            let (reach_lat_deg, reach_lon_deg) = reach_box_half_extents_deg(point.lat, reach);
            if point.lat - reach_lat_deg > bbox.north_lat
                || point.lat + reach_lat_deg < bbox.south_lat
                || point.lon - reach_lon_deg > bbox.east_lon
                || point.lon + reach_lon_deg < bbox.west_lon
            {
                return None;
            }
            let mut emission_lden = [0.0f64; NUM_BANDS];
            // Band-outer / period-inner accumulation order kept verbatim from the
            // pre-refactor kernel — the f32→f64 sum order is part of byte parity.
            #[allow(clippy::needless_range_loop)]
            for i in 0..NUM_BANDS {
                for p in 0..NUM_PERIODS {
                    emission_lden[i] += point.emission_lin[p][i] as f64 * LDEN_WEIGHTS[p];
                }
            }
            Some(PreparedPoint {
                point,
                py0: lat_to_py(bbox, point.lat + reach_lat_deg),
                py1: lat_to_py(bbox, point.lat - reach_lat_deg),
                px0: lon_to_px(bbox, point.lon - reach_lon_deg),
                px1: lon_to_px(bbox, point.lon + reach_lon_deg),
                src_alt: tile.elevation(point.lat, point.lon) + point.source_height_m,
                emission_lden,
            })
        }));
    }

    #[inline]
    fn pixel(
        &self,
        pp: &PreparedPoint<'a>,
        _tile: &FusedTileZ13,
        rx_lat: f64,
        rx_lon: f64,
        rx_alt: f64,
        refl: f64,
    ) -> Option<PixelTerms> {
        let point = pp.point;
        let reach = point.max_distance_m;
        let src_alt = pp.src_alt;
        let dist_m = flat_dist(rx_lat, rx_lon, point.lat, point.lon);
        if dist_m > reach {
            return None;
        }
        // Free-field audibility pre-gate (oracle parity): skip receivers where even
        // the unattenuated level is inaudible. A REAL per-pixel divergence — must
        // cull BEFORE any budget/path work.
        if below_free_field_threshold(point.max_day_emission_db, dist_m, 0.0) {
            return None;
        }
        // Area source: the footprint shrinks the effective distance.
        let prop_dist = effective_area_source_dist(dist_m, point.exclusion_radius_m);
        let d_slant = slant_dist(prop_dist, src_alt, rx_alt).max(1.0);
        let geo_div = 20.0 * d_slant.log10() + 11.0;
        let atm_d_km = d_slant / 1000.0;
        let base_db = refl - geo_div;
        Some(PixelTerms {
            base_db,
            atm_d_km,
            // The profile is sampled along the PRE-exclusion flat distance (NOT
            // prop_dist, which only shrinks the divergence d_slant).
            profile_dist_m: dist_m,
            src_alt,
            excl_m: point.exclusion_radius_m,
            cp_lat: point.lat,
            cp_lon: point.lon,
            // Point sources now share the line path's bare-earth OLS and
            // path-averaged IMD semantics.  The shared scatter kernel forms it
            // only after the byte-stop admits this pair.
            force_hard_ground: false,
            // No angular span to clip: a point source IS its characteristic
            // point, so the cp-ray verdict already covers every direction.
            arc: None,
        })
    }
}
