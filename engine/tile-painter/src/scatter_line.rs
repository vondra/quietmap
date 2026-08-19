//! Line-source (road + rail) scatter onto a Web Mercator base-zoom tile — surface
//! heatmap. ISO line-source physics (cylindrical divergence + finite-line
//! correction): a thin entry point over the generic [`crate::scatter_band`]
//! kernel with [`LineGeometry`]. The shared machinery (receiver-block
//! parallelism, energy-budget skip, terrain ray-march, `max(A_gr, A_bar)`
//! assembly, 3-period accumulation) lives in `scatter_band`; the line-specific
//! physics is `LineGeometry::pixel`:
//!
//! * ISO 9613-2 CYLINDRICAL line divergence `10·log10(2π·d_slant)` (a point
//!   source would be `20·log10(d)+11`; the aircraft kernel uses `θ/d_perp` —
//!   neither applies here).
//! * Finite-line correction paired with the divergence distance
//!   (`geo::finite_line_correction_for_divergence`): `10·log10(θ/π) +
//!   10·log10(d_div/d_perp)`, on the PERPENDICULAR distance to the segment's
//!   infinite line and the SIGNED foot fraction (matches the popup — fix-pack C).
//! * Arc-clipped screening (fix-pack Fix 1, `propagation::arc_screening`): a
//!   250 m microsegment subtends 136° at 50 m, so the cp ray's single screening
//!   verdict cannot stand for the whole segment. Vector regions energy-average
//!   the ground/barrier term over the segment's angular span; raster-fallback
//!   regions keep the cp verdict.
//! * CNOSSOS `L_W'/m` emission, pre-baked per period as linear band energy in
//!   the loader; the hot loop multiplies by a shared per-pixel path factor.
//! * Source height carried on the row (road 0.05 m, rail 0.5 m); receiver 4 m
//!   (pre-baked alt).
//!
//! Each segment scatters only the pixels inside its reach disk (a residential
//! road's 800 m reach touches ~18 000 of 262 144 px, not all) — the reach-bbox
//! clip that keeps dense city tiles viable at world scale.
//!
//! ground/barrier interaction is `max(A_gr, A_bar)` (ISO 9613-2 §7.3.1), NOT a
//! sum. Per-period steady power accumulates into [`TileAccumulator`]; the
//! caller collapses via `wire_hm3::collapse_lden_surface_u8` (no time-division).

use std::f64::consts::PI;

use noise_compute::propagation::geo::{
    finite_line_correction_for_divergence, point_to_segment_full, reach_box_half_extents_deg,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::propagation::path_profile::CoarseMid;
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::FusedTileZ13;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::scatter_band::{
    coarse_mid_cfg, lat_to_py, lon_to_px, scatter_tile_with_cfg as band_scatter_tile_with_cfg,
    ArcSegment, PixelGeometry, PixelTerms, PreparedSource, ScatterStats, LDEN_WEIGHTS, NUM_BANDS,
};
use crate::source_line::LineRow;

#[derive(Debug, Clone, Copy, Default)]
pub struct LineScatterStats {
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

impl From<ScatterStats> for LineScatterStats {
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

/// Scatter every line segment onto `tile`, accumulating per-period steady
/// power. `accum` is collapsed by the caller with the surface (no
/// time-division) Lden collapse.
///
/// `barriers` is the tile's noise-wall slice prepared by
/// `source_loader_barrier::BarrierData::for_tile` (sorted ascending,
/// conservative `dist_m` — see the `types::Barrier` contract). Empty for
/// the 98.5% of regions without a `barriers.arrow`.
pub fn scatter_tile(
    tile: &FusedTileZ13,
    lines: &[LineRow],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    accum: &mut TileAccumulator,
) -> LineScatterStats {
    scatter_tile_with_cfg(tile, lines, barriers, obstacles, accum, coarse_mid_cfg())
}

/// [`scatter_tile`] with the coarse-middle cadence passed EXPLICITLY (bypassing
/// the process-wide `coarse_mid_cfg` env read). The noise-floor harness uses
/// this to render the exact (`None`) and coarse fields in ONE process; the
/// production path uses [`scatter_tile`].
pub fn scatter_tile_with_cfg(
    tile: &FusedTileZ13,
    lines: &[LineRow],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    accum: &mut TileAccumulator,
    cfg: Option<CoarseMid>,
) -> LineScatterStats {
    band_scatter_tile_with_cfg(
        &LineGeometry { lines },
        tile,
        barriers,
        obstacles,
        lines.len(),
        accum,
        cfg,
    )
    .into()
}

/// A segment's tile-pixel reach box + Lden-weighted emission spectrum + a borrow
/// of its row, precomputed once so the per-block loop is a cheap reach-box ∩ block
/// clip + pixel sweep.
pub(crate) struct PreparedLine<'a> {
    pub(crate) line: &'a LineRow,
    py0: usize,
    py1: usize,
    px0: usize,
    px1: usize,
    emission_lden: [f64; NUM_BANDS],
}

impl PreparedSource for PreparedLine<'_> {
    #[inline]
    fn reach_box(&self) -> (usize, usize, usize, usize) {
        (self.py0, self.py1, self.px0, self.px1)
    }
    #[inline]
    fn emission_lin(&self) -> &[[f32; NUM_BANDS]; NUM_PERIODS] {
        &self.line.emission_lin
    }
    #[inline]
    fn emission_lden(&self) -> &[f64; NUM_BANDS] {
        &self.emission_lden
    }
    #[inline]
    fn block_constant_source_latlon(&self) -> Option<(f64, f64)> {
        // The line profile's sample point is the segment FOOT nearest each
        // receiver — receiver-dependent, so no per-block cache.
        None
    }
}

/// ISO 9613-2 line-source physics (road + rail): cylindrical divergence
/// `10·log10(2π·d_slant)`, finite-line correction with the clamped foot distance
/// as its perpendicular argument, path-averaged ground (hard `G=0` on a bridge),
/// and the segment foot as the profile sample point. The borrowed `lines` slice
/// is the source rows the prepare phase clips.
pub(crate) struct LineGeometry<'a> {
    pub(crate) lines: &'a [LineRow],
}

impl<'a> PixelGeometry for LineGeometry<'a> {
    type Prep = PreparedLine<'a>;

    fn prepare(&self, tile: &FusedTileZ13, prep: &mut Vec<PreparedLine<'a>>) {
        let bbox = &tile.bbox;
        prep.extend(self.lines.iter().filter_map(|line| {
            if line
                .emission_lin
                .iter()
                .all(|p| p.iter().all(|&e| e <= 0.0))
            {
                return None;
            }
            let reach = line.max_distance_m;
            let seg_s_lat = line.start_lat.min(line.end_lat);
            let seg_n_lat = line.start_lat.max(line.end_lat);
            let seg_w_lon = line.start_lon.min(line.end_lon);
            let seg_e_lon = line.start_lon.max(line.end_lon);
            // Shared with the point and ground-ops kernels: cos at the POLEWARD
            // EDGE, clamped where `m_per_deg_lon` clamps. The `max(0.2)` this
            // replaces under-covered longitude above 78.46 deg (world sweep of the
            // same defect fixed in `scatter_point`).
            let (reach_lat_deg, reach_lon_deg) =
                reach_box_half_extents_deg(seg_n_lat.abs().max(seg_s_lat.abs()), reach);
            if seg_s_lat - reach_lat_deg > bbox.north_lat
                || seg_n_lat + reach_lat_deg < bbox.south_lat
                || seg_w_lon - reach_lon_deg > bbox.east_lon
                || seg_e_lon + reach_lon_deg < bbox.west_lon
            {
                return None;
            }
            let mut emission_lden = [0.0f64; NUM_BANDS];
            // Band-outer / period-inner accumulation order kept verbatim from the
            // pre-refactor kernel — the f32→f64 sum order is part of byte parity.
            #[allow(clippy::needless_range_loop)]
            for i in 0..NUM_BANDS {
                for p in 0..NUM_PERIODS {
                    emission_lden[i] += line.emission_lin[p][i] as f64 * LDEN_WEIGHTS[p];
                }
            }
            Some(PreparedLine {
                line,
                py0: lat_to_py(bbox, seg_n_lat + reach_lat_deg),
                py1: lat_to_py(bbox, seg_s_lat - reach_lat_deg),
                px0: lon_to_px(bbox, seg_w_lon - reach_lon_deg),
                px1: lon_to_px(bbox, seg_e_lon + reach_lon_deg),
                emission_lden,
            })
        }));
    }

    #[inline]
    fn pixel(
        &self,
        pl: &PreparedLine<'a>,
        tile: &FusedTileZ13,
        rx_lat: f64,
        rx_lon: f64,
        rx_alt: f64,
        refl: f64,
    ) -> Option<PixelTerms> {
        let line = pl.line;
        let length_m = line.length_m as f64;
        let reach = line.max_distance_m;
        let pts = point_to_segment_full(
            rx_lat,
            rx_lon,
            line.start_lat,
            line.start_lon,
            line.end_lat,
            line.end_lon,
        );
        let dist_m = pts.d_endpoint_m;
        if dist_m > reach {
            return None;
        }
        let src_alt = tile.elevation(pts.cp_lat, pts.cp_lon) + line.source_height_m;
        let d_slant = (dist_m * dist_m + (src_alt - rx_alt).powi(2))
            .sqrt()
            .max(1.0);
        // FLC paired with the DIVERGENCE distance (fix-pack C, popup parity):
        // the finite-line geometry runs on the perpendicular distance to the
        // segment's INFINITE line and the SIGNED foot position, while
        // divergence/atmosphere stay on the endpoint distance. Feeding the
        // endpoint distance as if it were the perpendicular one read a segment
        // the receiver sits PAST +1.9 dB loud.
        let flc =
            finite_line_correction_for_divergence(length_m, pts.d_perp_m, pts.fraction, dist_m);
        let geo_div = 10.0 * (2.0 * PI * d_slant).log10();
        let atm_d_km = d_slant / 1000.0;
        let base_db = refl + flc - geo_div;
        Some(PixelTerms {
            base_db,
            atm_d_km,
            profile_dist_m: dist_m,
            src_alt,
            excl_m: 0.0,
            cp_lat: pts.cp_lat,
            cp_lon: pts.cp_lon,
            force_hard_ground: line.bridge,
            // Arc screening (fix-pack Fix 1): the segment the cp ray stands for.
            arc: Some(ArcSegment {
                start_lat: line.start_lat,
                start_lon: line.start_lon,
                end_lat: line.end_lat,
                end_lon: line.end_lon,
                source_height_m: line.source_height_m,
                length_m,
                dist_m,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
    use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
    use raster_reader::fused_tile_z13::TILE_PX;
    use raster_reader::RealRasters;
    use std::path::Path;
    use std::sync::Arc;

    /// Lden-weighted energy at one pixel (the accumulator's `[pix·3 + period]`
    /// layout), in dB relative to an arbitrary reference — only RATIOS are read.
    fn pixel_db(accum: &TileAccumulator, py: usize, px: usize) -> f64 {
        let i = (py * TILE_PX + px) * NUM_PERIODS;
        let e: f64 = (0..NUM_PERIODS)
            .map(|p| accum.energy[i + p] as f64 * LDEN_WEIGHTS[p])
            .sum();
        10.0 * e.max(1e-30).log10()
    }

    /// Ground factor at the tile centre. Both fixtures below are uniform, so
    /// this is also what the line kernel's path-averaged `G` will be.
    fn tile_ground_g(tile: &FusedTileZ13) -> f64 {
        tile.ground_g(
            (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5,
            (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5,
        )
    }

    /// Road and rail use the shared polar reach-box geometry, not the retired
    /// `cos(lat).max(0.2)` copy. Each source sits just far enough east of its
    /// tile that the old box rejected it while the exact per-pixel reach disk
    /// still touches the tile. The five rows pin the inhabited 79-83 deg band.
    #[test]
    fn polar_line_reach_box_keeps_sources_the_exact_gate_can_reach() {
        let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-polar-line-fixture"));
        for (expected_lat, tile_y) in [
            (79.0_f64, 522_u32),
            (80.0, 459),
            (81.6, 345),
            (82.5, 271),
            (83.0, 226),
        ] {
            let tile = FusedTileZ13::build_receiver_altitude_only(12, 2207, tile_y, &rasters);
            let source_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
            assert!((source_lat - expected_lat).abs() < 0.01, "lat={source_lat}");

            let reach_m = 281.84;
            let (_, fixed_lon_deg) = reach_box_half_extents_deg(source_lat, reach_m);
            let retired_lon_deg = reach_m / (111_320.0 * source_lat.to_radians().cos().max(0.2));
            assert!(fixed_lon_deg > retired_lon_deg);
            let source_lon = tile.bbox.east_lon + (fixed_lon_deg + retired_lon_deg) * 0.5;
            let nearest_tile_distance_m = noise_compute::propagation::geo::flat_dist(
                source_lat,
                source_lon,
                source_lat,
                tile.bbox.east_lon,
            );
            assert!(
                nearest_tile_distance_m <= reach_m,
                "line at {source_lat:.2} deg is {nearest_tile_distance_m:.2} m outside the tile"
            );
            let line = LineRow {
                start_lat: source_lat,
                start_lon: source_lon,
                end_lat: source_lat,
                end_lon: source_lon,
                length_m: 1.0,
                max_distance_m: reach_m,
                source_height_m: 0.05,
                bridge: false,
                emission_lin: [[1.0; NUM_BANDS]; NUM_PERIODS],
            };
            let mut prepared = Vec::new();
            LineGeometry {
                lines: std::slice::from_ref(&line),
            }
            .prepare(&tile, &mut prepared);
            assert_eq!(
                prepared.len(),
                1,
                "line at {source_lat:.2} deg was dropped by its reach box"
            );
        }
    }

    /// The scene, run twice on one tile: `(clear_db, screened_db)` at the pixel
    /// 50 m north of the segment start — a 214 m E-W segment through the tile
    /// centre and a 30 × 10 × 8 m box straddling the receiver's perpendicular
    /// foot, 25 m north of the segment. Shared so both ground regimes below
    /// measure the SAME geometry.
    fn arc_scene_pixel_db(tile: &FusedTileZ13) -> (f64, f64) {
        let c_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let c_lon = (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5;
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(c_lat.to_radians());

        let line = LineRow {
            start_lat: c_lat,
            start_lon: c_lon,
            end_lat: c_lat,
            end_lon: c_lon + d_lon(214.0),
            length_m: 214.0,
            max_distance_m: 2_000.0,
            source_height_m: 0.05,
            bridge: false,
            emission_lin: [[1.0e6; NUM_BANDS]; NUM_PERIODS],
        };
        let mut b = ObstacleIndex::builder(c_lat, c_lon);
        b.add_ring(
            &[
                (c_lat + d_lat(25.0), c_lon + d_lon(-15.0)),
                (c_lat + d_lat(25.0), c_lon + d_lon(15.0)),
                (c_lat + d_lat(35.0), c_lon + d_lon(15.0)),
                (c_lat + d_lat(35.0), c_lon + d_lon(-15.0)),
            ],
            8.0,
            ObstacleKind::Building,
            0,
        );
        let obstacles = ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        };

        let py = lat_to_py(&tile.bbox, c_lat + d_lat(50.0));
        let px = lon_to_px(&tile.bbox, c_lon);
        let run = |set: Option<&ObstacleSet>| {
            let mut accum = TileAccumulator::new();
            scatter_tile(tile, std::slice::from_ref(&line), &[], set, &mut accum);
            pixel_db(&accum, py, px)
        };
        (run(None), run(Some(&obstacles)))
    }

    /// Stripe regression (fix-pack Fix 1), the TILE twin of noise-compute's
    /// `roads::box_on_the_cp_ray_screens_only_its_angular_share`: a 30 m
    /// building straddling the cp ray must NOT screen the whole 214 m segment.
    /// Its shadow covers a slice of the fan the receiver sees, so the loss is
    /// ~2 dB — the cp-ray verdict alone applied the full ~15 dB diffraction to
    /// every metre of the segment, which IS the constant-width shadow stripe.
    ///
    /// Flat synthetic tile from an EMPTY rasters dir, so the only obstacle in
    /// the scene is the vector footprint. Built through
    /// `build_receiver_altitude_only`, whose empty halo gives the path profile
    /// flat AND SOFT ground (IMD 0 ⇒ G = 1) — the same ground its noise-compute
    /// twin uses (`FlatRasters`). The hard-ground half of the same scene, where
    /// the arc lane's non-negative handback used to lose the effect entirely, is
    /// `hard_ground_keeps_its_partial_screening` below.
    #[test]
    fn box_on_the_cp_ray_screens_only_its_angular_share() {
        let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-arc-fixture"));
        let tile = FusedTileZ13::build_receiver_altitude_only(12, 2211, 1386, &rasters);
        assert_eq!(tile_ground_g(&tile), 1.0, "fixture must be soft ground");
        let (clear, screened) = arc_scene_pixel_db(&tile);
        let loss = clear - screened;
        assert!(
            loss > 0.2,
            "the box must screen the arc it covers, got {loss:.2} dB"
        );
        assert!(
            loss < 3.0,
            "…but not the whole segment: {loss:.2} dB (the cp-ray verdict was ~15 dB)"
        );
    }

    /// The SPEC §3.5c hole, CLOSED on the tile path — and this test is the
    /// tripwire that was written to fail on the day it was.
    ///
    /// The same scene over HARD ground (the missing-IMD default of 100 ⇒ G = 0)
    /// used to lose its partial screening entirely: the fan is ~20 % blocked, so
    /// its energy mean sits near −2.6 dB, a net BOOST, below the 0 dB a
    /// non-negative screening increment can express — and arc screening, which
    /// has to hand its answer back through that increment, saturated at 0 and
    /// let the caller fall back to the full −3 dB hard-ground term. The pixel
    /// read LOUDER than exact by up to 3 dB, exactly where buildings live.
    ///
    /// The angular quadrature (`propagation::seg_sampling`) hands back the
    /// COMPOSITE `max(A_ground, A_terrain + A_screen)` instead of an increment,
    /// so there is no channel to saturate and the boost survives: the box now
    /// screens 2.2 dB here. The popup lane still goes through the increment and
    /// still has the hole — SPEC §3.5c stays open for it — which is why this
    /// test pins the TILE number specifically.
    #[test]
    fn hard_ground_keeps_its_partial_screening() {
        let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-arc-fixture"));
        let tile = FusedTileZ13::build(12, 2211, 1386, 2_000.0, &rasters);
        assert_eq!(tile_ground_g(&tile), 0.0, "fixture must be hard ground");
        let (clear, screened) = arc_scene_pixel_db(&tile);
        let loss = clear - screened;
        assert!(
            loss > 0.2,
            "hard ground must keep the arc the box covers, got {loss:.2} dB \
             (0.00 dB was the increment channel saturating — SPEC §3.5c)"
        );
        assert!(loss < 3.0, "…but still not the whole segment: {loss:.2} dB");
    }
}
