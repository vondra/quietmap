//! Airport ground-ops scatter onto a Web Mercator heatmap tile — taxiway / runway /
//! apron line sources + GSE point rows. Mirrors the road/rail
//! [`crate::scatter_line`] and industrial/building [`crate::scatter_point`]
//! kernels: per-source reach-bbox prepare, receiver-BLOCK parallelism,
//! the exact byte-space interval stop ([`crate::byte_stop`]), and the metadata-free
//! `path_effects` variants — zero `RealRasters` mmap in the hot loop (terrain /
//! reflection / receiver altitude pre-baked into [`FusedTileZ13`]).
//!
//! Ground-ops-specific physics (vs the line/point kernels):
//!  * One [`AirportTrafficRowView`] is one (microseg × period × veh_kind) event
//!    row; rows are grouped by `(osm_id, segment_idx)` so ONE path build per
//!    (microseg, pixel) serves all the microseg's rows (the period/veh_kind sum at
//!    the end), preserving the V0 amortization.
//!  * Per-veh_kind geometric divergence: aircraft = CNOSSOS-EU line
//!    `10·log10(θ/d_perp)`, GSE = point `10·log10(25/d)` — both consumed in linear
//!    form, so the spec-form dB round-trip collapses to a direct division.
//!  * Half-pixel divergence floor on both `d_endpoint` and `d_perp`, anti-aliasing
//!    the bright dots a runway/taxiway crossing a pixel obliquely would produce.
//!
//! ## Byte-space stop (M7)
//!
//! Replaces the superseded energy-budget skip (`skipped ≤ η·kept`, η = 0.40) with
//! the exact byte-space interval stop. The pixel-major walk collects all candidate
//! microsegments reaching the pixel, evaluates their free-field upper bounds `ub`,
//! sorts them loudest-bound first, and checks [`crate::byte_stop::decided`].
//!
//! Once the interval `[P⁻, P⁺]` quantises to the same output byte (accounting for
//! f32 accumulation error via [`crate::byte_stop::accum_margin`]), the pixel commits
//! its byte and stops immediately. Contributions are accumulated in source-load
//! order (`ord`) to guarantee bit-deterministic f32 summation.
//!
//! Energy normalisation (Convention B / `airport_traffic_v6`): `band_energy_lin` is
//! raw Σ over n_days; the caller's [`crate::wire_hm3::collapse_lden_u8`] divides by
//! `n_days × period_seconds`, so the stop uses [`crate::byte_stop::ground_lden_scale`]
//! with `scale = 1.0 / (86400.0 × n_days)`.

use std::collections::HashMap;
use std::f64::consts::LN_10;

use noise_compute::compute::aircraft_v6::AirportTrafficRowView;
use noise_compute::constants::{ground_ops_max_radius, ALPHA_ATM, GROUND_GAIN_UB_DB};
use noise_compute::propagation::geo::{
    point_to_segment_full, reach_box_half_extents_deg, PointToSegment,
};
use noise_compute::propagation::iso9613::{aircraft_ground_atten_db, fast_exp_f64};
use noise_compute::propagation::obstacle_index::{CrossingCandidate, ObstacleSet};
use noise_compute::propagation::{path_effects, PathProfile};
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13, TILE_PX};
use rayon::prelude::*;

use crate::accumulator::TileAccumulator;
use crate::byte_stop::{self, ground_lden_scale};
use crate::scatter_band::{byte_stop_enabled, lat_to_py, lon_to_px, recv_block_regions, UB_SAFETY};

const NUM_BANDS: usize = 8;
const NUM_PERIODS: usize = 3;

const GROUND_OPS_REF_OFFSET_M: f64 = 25.0;
const GROUND_OPS_SOURCE_HEIGHT_M: f64 = 4.0;

/// Per-period Lden-energy weights for ground-ops: `collapse_lden_u8` → `period_leq`
/// divides each period's energy by `n_days × PERIOD_SECONDS[p]` ([43200, 14400, 28800]),
/// so inside `compute_lden` the period weights factor to `[1, √10, 10]`.
pub const GROUND_LDEN_WEIGHTS: [f64; NUM_PERIODS] = [1.0, 3.162_277_660_168_379_5, 10.0];

const A_WEIGHT_LIN: [f64; NUM_BANDS] = [
    0.002_398_832_919_019,
    0.024_547_089_156_851,
    0.138_038_426_460_289,
    0.478_630_092_322_638,
    1.0,
    1.318_256_738_556_407,
    1.258_925_411_794_167,
    0.776_247_116_628_692,
];

#[derive(Debug, Clone, Copy, Default)]
pub struct GroundOpsStats {
    pub rows_seen: usize,
    pub rows_in_reach: usize,
    pub unique_microsegs: usize,
    pub pairs: u64,
    pub path_calls: u64,
    pub skipped_calls: u64,
}

/// One microsegment's geometry + reach-bbox + Lden-weighted emission (split by
/// veh_kind for the cheap pass bound) + the rows sharing it.
struct PreparedMicroseg {
    /// Rows in `traffic` sharing this `(osm_id, segment_idx)`.
    row_indices: Vec<usize>,
    osm_id: u64,
    segment_idx: u16,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    seg_length_m: f64,
    /// Per-`ops_kind` propagation cutoff (runway/taxiway/apron differ).
    max_radius: f64,
    py0: usize,
    py1: usize,
    px0: usize,
    px1: usize,
    /// `Σ_rows GROUND_LDEN_WEIGHTS[period] · band_energy_lin`, aircraft rows only.
    emission_lden_aircraft: [f64; NUM_BANDS],
    /// Same, GSE rows only.
    emission_lden_gse: [f64; NUM_BANDS],
}

/// One (microsegment, receiver pixel) pair recorded during the cheap pass.
#[derive(Clone, Copy)]
struct GroundPairBound {
    /// Index into `prep` slice.
    src: u32,
    /// Position in receiver's source-load order for deterministic f32 accumulation.
    ord: u32,
    /// Free-field upper bound over all bands and periods.
    ub: f64,
    pts: PointToSegment,
    d_to_recv: f64,
    geo_aircraft_lin: f64,
    geo_gse_lin: f64,
}

/// Thread-local scratch buffers for receiver block processing.
struct GroundOpsScratch {
    local: TileAccumulator,
    profile: PathProfile,
    cand_scratch: Vec<CrossingCandidate>,
    pairs_cand: Vec<u32>,
    pairs: Vec<GroundPairBound>,
    suffix: Vec<f64>,
    pair_pow: Vec<[f32; NUM_PERIODS]>,
    pair_hit: Vec<bool>,
    pairs_seen: u64,
    path_calls: u64,
    skipped_calls: u64,
}

impl GroundOpsScratch {
    fn new() -> Self {
        Self {
            local: TileAccumulator::new(),
            profile: PathProfile::new(),
            cand_scratch: Vec::with_capacity(32),
            pairs_cand: Vec::with_capacity(512),
            pairs: Vec::with_capacity(512),
            suffix: Vec::with_capacity(513),
            pair_pow: Vec::with_capacity(512),
            pair_hit: Vec::with_capacity(512),
            pairs_seen: 0,
            path_calls: 0,
            skipped_calls: 0,
        }
    }
}

/// Scatter every airport-traffic microseg onto `tile`, accumulating per-period
/// event power. The caller collapses with `collapse_lden_u8` (Convention B:
/// ÷ n_days × period_seconds).
pub fn scatter_tile(
    tile: &FusedTileZ13,
    traffic: &[AirportTrafficRowView<'_>],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    // GA hybrid per-class weight LUT. Applied to `veh_kind == 0` (aircraft)
    // rows
    // only — GSE rows weight 1.0. Uniform for non-hybrid extracts.
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
    n_days: f64,
    accum: &mut TileAccumulator,
) -> GroundOpsStats {
    scatter_tile_impl(
        tile,
        traffic,
        barriers,
        obstacles,
        class_weights,
        n_days,
        byte_stop_enabled(),
        accum,
    )
}

pub(crate) fn scatter_tile_impl(
    tile: &FusedTileZ13,
    traffic: &[AirportTrafficRowView<'_>],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
    n_days: f64,
    stop_on: bool,
    accum: &mut TileAccumulator,
) -> GroundOpsStats {
    let mut stats = GroundOpsStats {
        rows_seen: traffic.len(),
        ..Default::default()
    };
    if traffic.is_empty() {
        return stats;
    }

    let bbox = &tile.bbox;
    let tile_centre_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
    let px_m = tile_pixel_size_m(tile.zoom, tile_centre_lat);
    let pixel_floor_m = px_m * 0.5;

    let prep = prepare_microsegs(tile, traffic, class_weights);
    stats.unique_microsegs = prep.len();
    stats.rows_in_reach = prep.iter().map(|m| m.row_indices.len()).sum();
    if prep.is_empty() {
        return stats;
    }

    let scale = ground_lden_scale(n_days);

    let (merged, pairs_seen, path_calls, skipped_calls) = recv_block_regions()
        .into_par_iter()
        .fold(
            GroundOpsScratch::new,
            |mut s, (py_lo, py_hi, px_lo, px_hi)| {
                if py_lo < py_hi && px_lo < px_hi {
                    scatter_band(
                        tile,
                        &prep,
                        traffic,
                        barriers,
                        obstacles,
                        class_weights,
                        py_lo,
                        py_hi,
                        px_lo,
                        px_hi,
                        pixel_floor_m,
                        scale,
                        stop_on,
                        &mut s,
                    );
                }
                s
            },
        )
        .map(|s| (s.local, s.pairs_seen, s.path_calls, s.skipped_calls))
        .reduce(
            || (TileAccumulator::new(), 0u64, 0u64, 0u64),
            |mut a, b| {
                a.0.merge_from(&b.0);
                (a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3)
            },
        );
    accum.merge_from(&merged);
    stats.pairs = pairs_seen;
    stats.path_calls = path_calls;
    stats.skipped_calls = skipped_calls;
    stats
}

/// Group rows by `(osm_id, segment_idx)`, compute each microseg's per-`ops_kind`
/// reach-bbox + Lden-weighted per-veh_kind emission, drop microsegs whose reach
/// can't touch the tile, and sort by `(osm_id, segment_idx)` for determinism.
fn prepare_microsegs(
    tile: &FusedTileZ13,
    traffic: &[AirportTrafficRowView<'_>],
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
) -> Vec<PreparedMicroseg> {
    let bbox = &tile.bbox;
    let mut by_microseg: HashMap<(u64, u16), Vec<usize>> = HashMap::new();
    for (i, row) in traffic.iter().enumerate() {
        by_microseg
            .entry((row.osm_id, row.segment_idx))
            .or_default()
            .push(i);
    }
    let mut prep: Vec<PreparedMicroseg> = by_microseg
        .into_iter()
        .filter_map(|((osm_id, segment_idx), row_indices)| {
            let head = &traffic[row_indices[0]];
            let reach = ground_ops_max_radius(head.ops_kind);
            let start_lat = head.start_lat as f64;
            let start_lon = head.start_lon as f64;
            let end_lat = head.end_lat as f64;
            let end_lon = head.end_lon as f64;
            let seg_s_lat = start_lat.min(end_lat);
            let seg_n_lat = start_lat.max(end_lat);
            let seg_w_lon = start_lon.min(end_lon);
            let seg_e_lon = start_lon.max(end_lon);
            let (reach_lat_deg, reach_lon_deg) =
                reach_box_half_extents_deg(seg_n_lat.abs().max(seg_s_lat.abs()), reach);
            if seg_s_lat - reach_lat_deg > bbox.north_lat
                || seg_n_lat + reach_lat_deg < bbox.south_lat
                || seg_w_lon - reach_lon_deg > bbox.east_lon
                || seg_e_lon + reach_lon_deg < bbox.west_lon
            {
                return None;
            }
            let mut emission_lden_aircraft = [0.0f64; NUM_BANDS];
            let mut emission_lden_gse = [0.0f64; NUM_BANDS];
            for &ri in &row_indices {
                let row = &traffic[ri];
                let w = GROUND_LDEN_WEIGHTS[row.period.min(2) as usize];
                let (dst, gw) = if row.veh_kind == 0 {
                    (
                        &mut emission_lden_aircraft,
                        class_weights.get(row.class_idx),
                    )
                } else {
                    (&mut emission_lden_gse, 1.0)
                };
                #[allow(clippy::needless_range_loop)]
                for i in 0..NUM_BANDS {
                    dst[i] += row.band_energy_lin[i] as f64 * w * gw;
                }
            }
            Some(PreparedMicroseg {
                osm_id,
                segment_idx,
                start_lat,
                start_lon,
                end_lat,
                end_lon,
                seg_length_m: head.length_m as f64,
                max_radius: reach,
                py0: lat_to_py(bbox, seg_n_lat + reach_lat_deg),
                py1: lat_to_py(bbox, seg_s_lat - reach_lat_deg),
                px0: lon_to_px(bbox, seg_w_lon - reach_lon_deg),
                px1: lon_to_px(bbox, seg_e_lon + reach_lon_deg),
                emission_lden_aircraft,
                emission_lden_gse,
                row_indices,
            })
        })
        .collect();
    prep.sort_by_key(|m| (m.osm_id, m.segment_idx));
    prep
}

/// Scatter microsegs reaching receiver block `[py_lo, py_hi) × [px_lo, px_hi)`
/// using the exact byte-space stop.
#[allow(clippy::too_many_arguments)]
fn scatter_band(
    tile: &FusedTileZ13,
    prep: &[PreparedMicroseg],
    traffic: &[AirportTrafficRowView<'_>],
    barriers: &[Barrier],
    obstacles: &ObstacleSet,
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
    py_lo: usize,
    py_hi: usize,
    px_lo: usize,
    px_hi: usize,
    pixel_floor_m: f64,
    scale: f64,
    stop_on: bool,
    s: &mut GroundOpsScratch,
) {
    // Block-level candidate shortlist to avoid re-checking all microsegs per pixel.
    s.pairs_cand.clear();
    for (i, pm) in prep.iter().enumerate() {
        if pm.py0 < py_hi && pm.py1 >= py_lo && pm.px0 < px_hi && pm.px1 >= px_lo {
            s.pairs_cand.push(i as u32);
        }
    }
    if s.pairs_cand.is_empty() {
        return;
    }

    for py in py_lo..py_hi {
        let rx_lat = tile.rx_lat[py];
        let row_base = py * TILE_PX;
        for px in px_lo..px_hi {
            let rx_lon = tile.rx_lon[px];
            let idx = row_base + px;
            let refl_db = tile.rx_refl_db[idx] as f64;

            // Cheap pass: evaluate geometry and free-field upper bound for reachable pairs.
            s.pairs.clear();
            for &ci in &s.pairs_cand {
                let pm = &prep[ci as usize];
                if py < pm.py0 || py > pm.py1 || px < pm.px0 || px > pm.px1 {
                    continue;
                }
                let pts = point_to_segment_full(
                    rx_lat,
                    rx_lon,
                    pm.start_lat,
                    pm.start_lon,
                    pm.end_lat,
                    pm.end_lon,
                );
                let d_to_recv = pts.d_endpoint_m.max(pixel_floor_m);
                if d_to_recv > pm.max_radius {
                    continue;
                }
                let d_minus_ref_km = (d_to_recv - GROUND_OPS_REF_OFFSET_M).max(0.0) / 1000.0;
                let geo_aircraft_lin = {
                    let d_perp = pts.d_perp_m.max(pixel_floor_m);
                    let rx_along = pts.fraction * pm.seg_length_m;
                    let theta =
                        ((pm.seg_length_m - rx_along) / d_perp).atan() + (rx_along / d_perp).atan();
                    theta.max(1e-12) / d_perp
                };
                let geo_gse_lin = GROUND_OPS_REF_OFFSET_M / d_to_recv;

                let mut ub = 0.0f64;
                for i in 0..NUM_BANDS {
                    let path_db_ub = refl_db - ALPHA_ATM[i] * d_minus_ref_km + GROUND_GAIN_UB_DB;
                    let aw_ub = fast_exp_f64(path_db_ub * LN_10 * 0.1) * A_WEIGHT_LIN[i];
                    ub += aw_ub
                        * (geo_aircraft_lin * pm.emission_lden_aircraft[i]
                            + geo_gse_lin * pm.emission_lden_gse[i]);
                }
                let ub = ub * UB_SAFETY;
                let ord = s.pairs.len() as u32;
                s.pairs.push(GroundPairBound {
                    src: ci,
                    ord,
                    ub,
                    pts,
                    d_to_recv,
                    geo_aircraft_lin,
                    geo_gse_lin,
                });
            }

            let n_pairs = s.pairs.len();
            if n_pairs == 0 {
                continue;
            }
            s.pairs_seen += n_pairs as u64;

            // Sort loudest-bound first to maximize interval closure speed.
            s.pairs.sort_unstable_by(|a, b| b.ub.total_cmp(&a.ub));

            s.suffix.clear();
            s.suffix.resize(n_pairs + 1, 0.0);
            for k in (0..n_pairs).rev() {
                s.suffix[k] = s.suffix[k + 1] + s.pairs[k].ub;
            }

            s.pair_hit.clear();
            s.pair_hit.resize(n_pairs, false);
            if s.pair_pow.len() < n_pairs {
                s.pair_pow.resize(n_pairs, [0.0; NUM_PERIODS]);
            }

            let margin = byte_stop::accum_margin(n_pairs);
            let mut p_lo = 0.0f64;
            let mut walked = n_pairs;

            let rx_alt = tile.rx_alt_m[idx] as f64;

            for k in 0..n_pairs {
                if stop_on && byte_stop::decided(p_lo, p_lo + s.suffix[k], scale, margin) {
                    walked = k;
                    break;
                }

                let pb = &s.pairs[k];
                let pm = &prep[pb.src as usize];
                let ord = pb.ord as usize;
                let ub = pb.ub;
                let pts = pb.pts;
                let d_to_recv = pb.d_to_recv;
                let d_minus_ref_km = (d_to_recv - GROUND_OPS_REF_OFFSET_M).max(0.0) / 1000.0;
                let geo_aircraft_lin = pb.geo_aircraft_lin;
                let geo_gse_lin = pb.geo_gse_lin;

                let src_alt = tile.elevation(pts.cp_lat, pts.cp_lon) + GROUND_OPS_SOURCE_HEIGHT_M;

                tile.build_path_profile(
                    pts.cp_lat,
                    pts.cp_lon,
                    rx_lat,
                    rx_lon,
                    d_to_recv,
                    &mut s.profile,
                );

                let ground_g = path_effects::ground_g_from_profile(&s.profile);
                let (terrain, terrain_delta_m) =
                    path_effects::terrain_attenuation(&mut s.profile, src_alt, rx_alt);
                obstacles.crossings(pts.cp_lat, pts.cp_lon, rx_lat, rx_lon, &mut s.cand_scratch);
                let obstacle_input = path_effects::ObstacleInput {
                    candidates: &s.cand_scratch,
                };
                let screening = path_effects::screening_attenuation(
                    &mut s.profile,
                    barriers,
                    obstacle_input,
                    src_alt,
                    rx_alt,
                    0.0,
                    &terrain,
                    terrain_delta_m,
                );
                let veg = path_effects::vegetation_attenuation_path(&s.profile);

                let mut path_aw_per_band = [0.0f64; NUM_BANDS];
                for i in 0..NUM_BANDS {
                    let atm_db = ALPHA_ATM[i] * d_minus_ref_km;
                    let a_gr = aircraft_ground_atten_db(i, ground_g);
                    let a_bar = terrain[i] + screening[i];
                    let gob = if a_bar > 0.0 { a_gr.max(a_bar) } else { a_gr };
                    let path_db = refl_db - atm_db - gob - veg[i];
                    path_aw_per_band[i] = fast_exp_f64(path_db * LN_10 * 0.1) * A_WEIGHT_LIN[i];
                }

                let mut kept_add = 0.0f64;
                let mut pow = [0.0f32; NUM_PERIODS];
                for &row_idx in &pm.row_indices {
                    let row = &traffic[row_idx];
                    let (geo_lin, gw) = if row.veh_kind == 0 {
                        (geo_aircraft_lin, class_weights.get(row.class_idx))
                    } else {
                        (geo_gse_lin, 1.0)
                    };
                    let mut aw_lin = 0.0f64;
                    #[allow(clippy::needless_range_loop)]
                    for i in 0..NUM_BANDS {
                        aw_lin += row.band_energy_lin[i] as f64 * path_aw_per_band[i];
                    }
                    aw_lin *= geo_lin * gw;
                    if aw_lin.is_finite() && aw_lin > 0.0 {
                        let period = row.period.min(2);
                        pow[period as usize] += aw_lin as f32;
                        kept_add += aw_lin * GROUND_LDEN_WEIGHTS[period as usize];
                    }
                }

                assert!(
                    kept_add <= ub,
                    "byte-stop bound violated in ground-ops: exact {kept_add:e} > ub {ub:e} \
                     (py={py} px={px} src={ci})",
                    ci = pb.src
                );

                p_lo += kept_add;
                s.pair_hit[ord] = true;
                s.pair_pow[ord] = pow;
            }

            s.path_calls += walked as u64;
            s.skipped_calls += (n_pairs - walked) as u64;

            // Accumulate in source-load order (ord) for deterministic f32 addition.
            for o in 0..n_pairs {
                if !s.pair_hit[o] {
                    continue;
                }
                let pow = s.pair_pow[o];
                for (p, &e) in pow.iter().enumerate() {
                    if e > 0.0 {
                        s.local.add_energy_at(py as u32, px as u32, p as u8, e);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire_hm3::{collapse_lden_u8, NO_DATA};
    use noise_compute::emission::aircraft::ClassWeights;
    use raster_reader::RealRasters;
    use std::path::Path;

    struct SyntheticRow {
        osm_id: u64,
        segment_idx: u16,
        ops_kind: u8,
        veh_kind: u8,
        period: u8,
        class_idx: u8,
        start_lat: f32,
        start_lon: f32,
        end_lat: f32,
        end_lon: f32,
        length_m: f32,
        band_energy_lin: [f32; 8],
        gse: [u32; 3],
    }

    impl SyntheticRow {
        fn view(&self) -> AirportTrafficRowView<'_> {
            AirportTrafficRowView {
                airport_key: "LKPR",
                osm_id: self.osm_id,
                segment_idx: self.segment_idx,
                geometry_kind: 1,
                start_lat: self.start_lat,
                start_lon: self.start_lon,
                end_lat: self.end_lat,
                end_lon: self.end_lon,
                length_m: self.length_m,
                ops_kind: self.ops_kind,
                is_departure: 0,
                veh_kind: self.veh_kind,
                class_idx: self.class_idx,
                period: self.period,
                band_energy_lin: &self.band_energy_lin,
                unique_movement_count: 10,
                unique_arr_count: 5,
                unique_dep_count: 5,
                unique_gse_count_per_class: &self.gse,
                microseg_unique_count: 10,
                microseg_unique_arr_count: 5,
                microseg_unique_dep_count: 5,
                microseg_unique_gse_count_per_class: &self.gse,
                microseg_unique_ga_count: 0,
                microseg_unique_ga_arr_count: 0,
                microseg_unique_ga_dep_count: 0,
            }
        }
    }

    fn make_test_tile() -> FusedTileZ13 {
        let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-bytestop-fixture"));
        FusedTileZ13::build(12, 2212, 1387, 2_500.0, &rasters)
    }

    fn make_synthetic_traffic(tile: &FusedTileZ13) -> Vec<SyntheticRow> {
        let mut rows = Vec::new();
        let bbox = &tile.bbox;
        let mid_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
        let mid_lon = (bbox.east_lon + bbox.west_lon) * 0.5;

        for seg_idx in 0..40u16 {
            let frac = seg_idx as f64 / 40.0;
            let start_lat = (mid_lat + (frac - 0.5) * 0.01) as f32;
            let start_lon = (mid_lon + (frac - 0.5) * 0.01) as f32;
            let end_lat = (mid_lat + (frac - 0.48) * 0.01) as f32;
            let end_lon = (mid_lon + (frac - 0.48) * 0.01) as f32;

            for period in 0..3u8 {
                let energy_scale = match period {
                    0 => 5e8,
                    1 => 1.5e8,
                    _ => 4e7,
                };
                rows.push(SyntheticRow {
                    osm_id: 1000 + (seg_idx % 4) as u64,
                    segment_idx: seg_idx,
                    ops_kind: 1, // runway
                    veh_kind: 0, // aircraft
                    period,
                    class_idx: 0,
                    start_lat,
                    start_lon,
                    end_lat,
                    end_lon,
                    length_m: 60.0,
                    band_energy_lin: [
                        (energy_scale * 0.05) as f32,
                        (energy_scale * 0.1) as f32,
                        (energy_scale * 0.2) as f32,
                        (energy_scale * 0.4) as f32,
                        (energy_scale * 0.5) as f32,
                        (energy_scale * 0.3) as f32,
                        (energy_scale * 0.15) as f32,
                        (energy_scale * 0.05) as f32,
                    ],
                    gse: [0; 3],
                });
            }
        }
        rows
    }

    /// Prove exactness: byte-stopped kernel (`stop_on = true`) produces
    /// identical output bytes to unstopped kernel (`stop_on = false`).
    #[test]
    fn byte_stop_matches_unstopped_exact_reference() {
        let tile = make_test_tile();
        let syn_rows = make_synthetic_traffic(&tile);
        let traffic: Vec<AirportTrafficRowView<'_>> = syn_rows.iter().map(|r| r.view()).collect();
        let class_weights = ClassWeights::uniform();
        let n_days = 365.0;

        let mut acc_stopped = TileAccumulator::new();
        let mut acc_unstopped = TileAccumulator::new();

        let st_stopped = scatter_tile_impl(
            &tile,
            &traffic,
            &[],
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            &class_weights,
            n_days,
            true,
            &mut acc_stopped,
        );

        let st_unstopped = scatter_tile_impl(
            &tile,
            &traffic,
            &[],
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            &class_weights,
            n_days,
            false,
            &mut acc_unstopped,
        );

        let bytes_stopped = collapse_lden_u8(&acc_stopped, n_days);
        let bytes_unstopped = collapse_lden_u8(&acc_unstopped, n_days);

        assert_eq!(
            bytes_stopped.len(),
            bytes_unstopped.len(),
            "byte vector lengths match"
        );

        let mut diff_count = 0;
        let mut painted_count = 0;
        for (i, (&b_stop, &b_unstop)) in
            bytes_stopped.iter().zip(bytes_unstopped.iter()).enumerate()
        {
            if b_unstop != NO_DATA {
                painted_count += 1;
            }
            if b_stop != b_unstop {
                diff_count += 1;
                let py = i / TILE_PX;
                let px = i % TILE_PX;
                eprintln!("Mismatch at ({py}, {px}): stopped={b_stop} unstopped={b_unstop}");
            }
        }

        assert_eq!(
            diff_count, 0,
            "Exactness failure: {diff_count} differing pixels out of {painted_count} painted pixels"
        );
        assert!(painted_count > 0, "Test painted at least some pixels");
        assert!(
            st_stopped.path_calls < st_unstopped.path_calls,
            "Byte-stop reduced path calls: {} vs {}",
            st_stopped.path_calls,
            st_unstopped.path_calls
        );
    }

    /// Prove source-order invariance: changing input row ordering commits the same byte output.
    #[test]
    fn source_order_invariance() {
        let tile = make_test_tile();
        let syn_rows1 = make_synthetic_traffic(&tile);
        let traffic1: Vec<AirportTrafficRowView<'_>> = syn_rows1.iter().map(|r| r.view()).collect();
        let mut syn_rows2 = make_synthetic_traffic(&tile);
        syn_rows2.reverse();
        let traffic2: Vec<AirportTrafficRowView<'_>> = syn_rows2.iter().map(|r| r.view()).collect();

        let class_weights = ClassWeights::uniform();
        let n_days = 365.0;

        let mut acc1 = TileAccumulator::new();
        let mut acc2 = TileAccumulator::new();

        scatter_tile(
            &tile,
            &traffic1,
            &[],
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            &class_weights,
            n_days,
            &mut acc1,
        );
        scatter_tile(
            &tile,
            &traffic2,
            &[],
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            &class_weights,
            n_days,
            &mut acc2,
        );

        let bytes1 = collapse_lden_u8(&acc1, n_days);
        let bytes2 = collapse_lden_u8(&acc2, n_days);

        assert_eq!(bytes1, bytes2, "Reversed source order changed output bytes");
    }
}
