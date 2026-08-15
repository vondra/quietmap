//! Airport ground-ops scatter onto a Web Mercator z13 tile — taxiway / runway /
//! apron line sources + GSE point rows. Mirrors the road/rail
//! [`crate::scatter_line`] and industrial/building [`crate::scatter_point`]
//! kernels: per-source reach-bbox prepare, receiver-BLOCK parallelism,
//! the per-pixel energy-budget skip, and the metadata-free `path_effects`
//! variants — zero `RealRasters` mmap in the hot loop (terrain / reflection /
//! receiver altitude pre-baked into [`FusedTileZ13`]).
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
//! Energy-budget skip: a microseg whose best-case contribution at a pixel — no
//! terrain/screening/veg + max ground gain, exact geometry, provably ≥ exact —
//! keeps the pixel's total skipped Lden within `η` of its kept Lden is dropped
//! without the path build (bound `≤ 10·log10(1+η)`). The mixed-geometry upper
//! bound is two dot-products (one per veh_kind family) over the microseg's
//! Lden-weighted band energy.
//!
//! THIS IS THE LAST KERNEL STILL ON THAT RULE, and the rule is known bad: it
//! compares against a `kept` that starts at zero and only grows, so its verdict
//! depends on the load order and it admits up to `10·log10(1+η)` = 1.46 dB of
//! energy never counted. The line/point kernels replaced it with the exact
//! byte-space interval ([`crate::byte_stop`], [`crate::scatter_band`]) in
//! 2026-08. Converting this one is a change of its own size, not a rename: the
//! walk has to go pixel-major so a receiver sees all its microsegs before any is
//! computed, and the accumulation order has to be preserved separately. What it
//! does NOT need is new byte-stop machinery — `byte_stop::decided` takes the Lden
//! energy with the per-period weights already folded in, and this layer's are
//! `W_p / (n_days × period_seconds_p)` (Convention B, below) instead of the
//! surface layers' bare `W_p`.
//!
//! Energy normalisation (Convention B / `airport_traffic_v6`): `band_energy_lin` is
//! raw Σ over n_days; the caller's [`crate::wire_hm3::collapse_lden_u8`] divides by
//! `n_days × period_seconds`, so this kernel does no per-row scaling — and the skip
//! ratio is n_days-invariant (kept/skipped/ub are all pre-division).
//!
//! Microsegs are accumulated in sorted `(osm_id, segment_idx)` order, so the f32
//! summation and the skip decisions are deterministic run-to-run — the prior
//! `HashMap`-fold order was not.

use std::collections::HashMap;
use std::f64::consts::LN_10;

use noise_compute::compute::aircraft_v6::AirportTrafficRowView;
use noise_compute::constants::{ground_ops_max_radius, ALPHA_ATM, GROUND_GAIN_UB_DB};
use noise_compute::propagation::geo::{point_to_segment_full, reach_box_half_extents_deg};
use noise_compute::propagation::iso9613::{aircraft_ground_atten_db, fast_exp_f64};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::propagation::path_effects;
use noise_compute::types::{Barrier, RasterSampler};
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13, TILE_PX};
use rayon::prelude::*;

use crate::accumulator::TileAccumulator;
use crate::scatter_band::{
    budget_eta, lat_to_py, lon_to_px, recv_block_regions, BandScratch, UB_SAFETY,
};

const NUM_BANDS: usize = 8;

const GROUND_OPS_REF_OFFSET_M: f64 = 25.0;
const GROUND_OPS_SOURCE_HEIGHT_M: f64 = 4.0;

/// Per-period Lden-energy weights for the BUDGET SKIP — ground-ops flavour, NOT the
/// surface `scatter_line::LDEN_WEIGHTS [12, 4√10, 80]`. Ground ops collapse with
/// `collapse_lden_u8` → `period_leq` divides each period's energy by
/// `n_days × PERIOD_SECONDS[p]` ([43200, 14400, 28800]), whereas the surface kernels
/// (`collapse_lden_surface_u8 = 10·log10(power)`) do not. So a period's contribution
/// to the displayed Lden is `LDEN_WEIGHTS[p] / PERIOD_SECONDS[p] ∝ [1, √10, 10]`. The
/// skip's kept/skipped/ub MUST use these for `skipped ≤ η·kept` to bound the displayed
/// under-read (n_days cancels in the ratio); the surface weights would under-weight
/// evening ~3× and let the worst-case mix drift ~3.4 dB instead of the bound.
const GROUND_LDEN_WEIGHTS: [f64; 3] = [1.0, 3.162_277_660_168_379_5, 10.0];

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
    pub path_calls: u64,
    pub skipped_calls: u64,
}

/// One microsegment's geometry + reach-bbox + Lden-weighted emission (split by
/// veh_kind for the skip's mixed-geometry bound) + the rows sharing it.
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
    /// `Σ_rows LDEN_WEIGHTS[period] · band_energy_lin`, aircraft rows only (the
    /// aircraft geo term multiplies this in the skip's upper bound).
    emission_lden_aircraft: [f64; NUM_BANDS],
    /// Same, GSE rows only (the GSE geo term multiplies this).
    emission_lden_gse: [f64; NUM_BANDS],
}

// Ground ops reuses the shared `scatter_band::BandScratch` (it ignores the extra
// `raster_samples` field — its telemetry doesn't track the ray-march cadence).

/// Scatter every airport-traffic microseg onto `tile`, accumulating per-period
/// event power. The caller collapses with `collapse_lden_u8` (Convention B:
/// ÷ n_days × period_seconds).
pub fn scatter_tile(
    tile: &FusedTileZ13,
    traffic: &[AirportTrafficRowView<'_>],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    // GA 365-day hybrid per-class weight LUT (`ga-365d-hybrid-plan.md` §2,
    // layer-ground.md §4). Applied to `veh_kind == 0` (aircraft) rows
    // only — GSE rows weight 1.0. Uniform for non-hybrid extracts.
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
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

    let eta = budget_eta();
    let (merged, path_calls, skipped_calls) = recv_block_regions()
        .into_par_iter()
        .fold(BandScratch::new, |mut s, (py_lo, py_hi, px_lo, px_hi)| {
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
                    eta,
                    &mut s,
                );
            }
            s
        })
        .map(|s| (s.local, s.path_calls, s.skipped_calls))
        .reduce(
            || (TileAccumulator::new(), 0u64, 0u64),
            |mut a, b| {
                a.0.merge_from(&b.0);
                (a.0, a.1 + b.1, a.2 + b.2)
            },
        );
    accum.merge_from(&merged);
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
            // Shared with the point kernel (this rule was right here and wrong
            // there; now there is one copy).
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
                // GA hybrid weight (`ga-365d-hybrid-plan.md` §2 /
                // layer-ground.md §4): aircraft rows scale by `w[class]`,
                // GSE rows by 1.0. Applied per row here so the skip bound
                // matches the (weighted) energy scattered in `scatter_band`.
                let (dst, gw) = if row.veh_kind == 0 {
                    (
                        &mut emission_lden_aircraft,
                        class_weights.get(row.class_idx),
                    )
                } else {
                    (&mut emission_lden_gse, 1.0)
                };
                // Band accumulation order is part of HM3 byte parity — kept as an
                // index loop (`dst[i] += …`) rather than a zip/iterator rewrite.
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

/// Scatter every microseg that reaches the block `[py_lo, py_hi) × [px_lo, px_hi)`
/// into its pixels, applying the per-pixel energy-budget skip; one path build per
/// (microseg, pixel) shared by all the microseg's rows.
#[allow(clippy::too_many_arguments)]
fn scatter_band(
    tile: &FusedTileZ13,
    prep: &[PreparedMicroseg],
    traffic: &[AirportTrafficRowView<'_>],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    class_weights: &noise_compute::emission::aircraft::ClassWeights,
    py_lo: usize,
    py_hi: usize,
    px_lo: usize,
    px_hi: usize,
    pixel_floor_m: f64,
    eta: f64,
    s: &mut BandScratch,
) {
    for pm in prep {
        let py0 = pm.py0.max(py_lo);
        let py1 = pm.py1.min(py_hi - 1);
        if py0 > py1 {
            continue;
        }
        let px0 = pm.px0.max(px_lo);
        let px1 = pm.px1.min(px_hi - 1);
        if px0 > px1 {
            continue;
        }
        let max_radius = pm.max_radius;
        let seg_length_m = pm.seg_length_m;
        for py in py0..=py1 {
            let rx_lat = tile.rx_lat[py];
            let row_base = py * TILE_PX;
            for px in px0..=px1 {
                let rx_lon = tile.rx_lon[px];
                let pts = point_to_segment_full(
                    rx_lat,
                    rx_lon,
                    pm.start_lat,
                    pm.start_lon,
                    pm.end_lat,
                    pm.end_lon,
                );
                // Floor at half a pixel: the receiver is a ~`px_m × px_m` cell, not
                // a point, and `1/d` divergence sampled at the cell centre aliases
                // against the grid when a centreline crosses obliquely.
                let d_to_recv = pts.d_endpoint_m.max(pixel_floor_m);
                if d_to_recv > max_radius {
                    continue;
                }
                let idx = row_base + px;
                let d_minus_ref_km = (d_to_recv - GROUND_OPS_REF_OFFSET_M).max(0.0) / 1000.0;
                let refl_db = tile.rx_refl_db[idx] as f64;

                // Per-veh_kind receiver geometric term (scalar across bands),
                // consumed in linear form (the spec-form `10^(10·log10(x)/10) = x`
                // collapses to a direct division). Needed by the skip bound, so
                // hoisted above the path build.
                let geo_aircraft_lin = {
                    let d_perp = pts.d_perp_m.max(pixel_floor_m);
                    let rx_along = pts.fraction * seg_length_m;
                    let theta =
                        ((seg_length_m - rx_along) / d_perp).atan() + (rx_along / d_perp).atan();
                    theta.max(1e-12) / d_perp
                };
                let geo_gse_lin = GROUND_OPS_REF_OFFSET_M / d_to_recv;

                // Best-case Lden energy upper bound: no terrain/screening/veg, the
                // most favourable ground any band can meet (`GROUND_GAIN_UB_DB` =
                // 3.0 dB — the CNOSSOS hard-ground floor, attained at G = 0 in every
                // band; see the constant for why it is no longer the per-band
                // `(-CF).max(0)`), exact geometry — provably ≥ the exact Lden
                // contribution of ALL the microseg's rows (two dot-products, one per
                // veh_kind family).
                let mut ub = 0.0f64;
                for i in 0..NUM_BANDS {
                    let path_db_ub = refl_db - ALPHA_ATM[i] * d_minus_ref_km + GROUND_GAIN_UB_DB;
                    let aw_ub = fast_exp_f64(path_db_ub * LN_10 * 0.1) * A_WEIGHT_LIN[i];
                    ub += aw_ub
                        * (geo_aircraft_lin * pm.emission_lden_aircraft[i]
                            + geo_gse_lin * pm.emission_lden_gse[i]);
                }
                let ub = ub * UB_SAFETY;
                if s.skipped[idx] + ub <= eta * s.kept[idx] {
                    s.skipped[idx] += ub;
                    s.skipped_calls += 1;
                    continue;
                }

                let rx_alt = tile.rx_alt_m[idx] as f64;
                let src_alt = tile.elevation(pts.cp_lat, pts.cp_lon) + GROUND_OPS_SOURCE_HEIGHT_M;

                // One path build per (microseg, pixel), meta-free variants (the
                // heatmap discards the popup obstacle traces).
                tile.build_path_profile(
                    pts.cp_lat,
                    pts.cp_lon,
                    rx_lat,
                    rx_lon,
                    d_to_recv,
                    &mut s.profile,
                );
                s.path_calls += 1;
                let ground_g = path_effects::ground_g_from_profile(&s.profile);
                let terrain = path_effects::terrain_attenuation(&mut s.profile, src_alt, rx_alt);
                let obstacle_input = match obstacles {
                    Some(set) => {
                        set.crossings(pts.cp_lat, pts.cp_lon, rx_lat, rx_lon, &mut s.cand_scratch);
                        path_effects::ObstacleInput {
                            candidates: &s.cand_scratch,
                            replace_sample_buildings: true,
                        }
                    }
                    None => path_effects::ObstacleInput::CANDIDATES_OFF,
                };
                let screening = path_effects::screening_attenuation(
                    &mut s.profile,
                    barriers,
                    obstacle_input,
                    src_alt,
                    rx_alt,
                    0.0,
                    &terrain,
                );
                let veg = path_effects::vegetation_attenuation_path(&s.profile);

                // Per-band shared path effects (geometric divergence factored OUT —
                // applied per row via `geo_*_lin`).
                let mut path_aw_per_band = [0.0f64; NUM_BANDS];
                for i in 0..NUM_BANDS {
                    let atm_db = ALPHA_ATM[i] * d_minus_ref_km;
                    let a_gr = aircraft_ground_atten_db(i, ground_g);
                    let a_bar = terrain[i] + screening[i];
                    // ISO 9613-2 §7.3.1: barrier REPLACES ground (max), never adds.
                    let gob = if a_bar > 0.0 { a_gr.max(a_bar) } else { a_gr };
                    let path_db = refl_db - atm_db - gob - veg[i];
                    path_aw_per_band[i] = fast_exp_f64(path_db * LN_10 * 0.1) * A_WEIGHT_LIN[i];
                }

                let mut kept_add = 0.0f64;
                for &row_idx in &pm.row_indices {
                    let row = &traffic[row_idx];
                    // GA hybrid weight: aircraft rows scale by `w[class]`,
                    // GSE by 1.0 — folded into the scattered energy so a
                    // 365-day-sampled GA one-off divides by 365 not 12.
                    let (geo_lin, gw) = if row.veh_kind == 0 {
                        (geo_aircraft_lin, class_weights.get(row.class_idx))
                    } else {
                        (geo_gse_lin, 1.0)
                    };
                    let mut aw_lin = 0.0f64;
                    // A-weighted band sum — f64 accumulation order is byte-parity.
                    #[allow(clippy::needless_range_loop)]
                    for i in 0..NUM_BANDS {
                        aw_lin += row.band_energy_lin[i] as f64 * path_aw_per_band[i];
                    }
                    aw_lin *= geo_lin * gw;
                    if aw_lin.is_finite() && aw_lin > 0.0 {
                        let period = row.period.min(2);
                        s.local
                            .add_energy_at(py as u32, px as u32, period, aw_lin as f32);
                        kept_add += aw_lin * GROUND_LDEN_WEIGHTS[period as usize];
                    }
                }
                s.kept[idx] += kept_add;
            }
        }
    }
}
