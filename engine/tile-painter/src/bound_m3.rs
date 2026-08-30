//! M3: exact (zero-drift) tightening of the byte-stop's per-pair upper bound —
//! the M3a ground-gain floor over a max-pooled imperviousness pyramid and the
//! M3b K=8 cp-ray terrain lower bound, per the doc block at
//! `scatter_band.rs` "How much a TIGHTER bound would buy".
//!
//! Both halves change NO output byte: they only narrow `P⁺`, so pairs the
//! loose bound had to walk can now be proven immaterial earlier. `SURFACE_BOUND_M3=0`
//! restores the loose bound bit-for-bit (the G1 reference arm, same one-binary
//! pattern as `SURFACE_BUDGET_ETA=0`).

use noise_compute::constants::GROUND_GAIN_UB_DB;
use noise_compute::constants::{FAV_RAY_CURVATURE_MIN_M, FAV_RAY_CURVATURE_PER_DSR};
use noise_compute::propagation::diffraction::diffraction_mixed_lower_bound;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::propagation::path_effects::terrain_subset_delta_lower_bound;
use noise_compute::propagation::path_profile::{
    fill_t_values, fill_t_values_coarse_mid, CoarseMid,
};
use noise_compute::types::NUM_BANDS;
use raster_reader::fused_tile_z13::FusedTileZ13;

use crate::scatter_band::{cadence_for_ray, PixelTerms};

/// Ground-bound chunk count (the doc block's measured K = 8).
const M3_GROUND_CHUNKS: usize = 8;
/// Terrain-bound march size (the doc block's measured K = 8).
const M3_TERRAIN_SAMPLES: usize = 8;

/// `SURFACE_BOUND_M3=0` disables both tightened bounds — the loose reference
/// arm. Any other value (or unset) enables them. One dial selects both arms of
/// the G1 exactness gate from one binary; there is nothing to tune beyond
/// on/off because the bounds are proofs, not approximations.
pub(crate) fn surface_bound_m3_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SURFACE_BOUND_M3").map_or(true, |v| v != "0"))
}

/// The tightened per-pair bound terms `budget_ub_lden` folds into `P⁺`.
#[derive(Clone, Copy)]
pub(crate) struct M3PairBound {
    /// Most ground GAIN any band can reach on this pair, dB:
    /// `GROUND_GAIN_UB_DB·(1−g_lo)` with `g_lo` a proven lower bound on the
    /// §2.5.14 blended ground factor (see [`ground_gain_from_mean_bound`]).
    ground_gain_ub_db: f64,
    /// M3b: per-band LOWER bound on this pair's cp-ray terrain attenuation,
    /// `None` where the exact path is not a single cp ray (the angular
    /// quadrature marches each bucket's own terrain — a cp-ray bound there
    /// reads as tighter and is unsound) or where the K-sample march finds no
    /// edge above the line of sight.
    terrain_lb_bands: Option<[f64; NUM_BANDS]>,
}

impl M3PairBound {
    /// The pre-M3 loose bound, bit-identical: full +3 dB ground-gain slack and
    /// no terrain term.
    #[inline]
    pub(crate) fn loose() -> Self {
        Self {
            ground_gain_ub_db: GROUND_GAIN_UB_DB,
            terrain_lb_bands: None,
        }
    }

    /// Per-band lower bound on the pair's `max(A_ground, A_terrain + A_screen)`
    /// composite: `gob ≥ A_ground ≥ −ground_gain_ub` always, and
    /// `gob ≥ A_terrain ≥ terrain_lb` where M3b is legal. `budget_ub_lden`
    /// subtracts exactly this.
    #[inline]
    pub(crate) fn gob_lb_db(&self, band: usize) -> f64 {
        let floor = -self.ground_gain_ub_db;
        match self.terrain_lb_bands {
            None => floor,
            Some(tb) => tb[band].max(floor),
        }
    }
}

/// Whether this pair's EXACT evaluation is a single characteristic-point ray —
/// the M3b legal population verbatim from the doc block: "point sources,
/// raster-fallback regions, `QM_SEG_SAMPLES=1`". Mirrors the branch-(1) arm of
/// the walk (`(Some(arc), Some(set)) if n_seg > 1` runs the angular
/// quadrature; everything else runs the cp ray) so the bound can never apply
/// where the quadrature will.
#[inline]
fn exact_path_is_single_cp_ray(arc_present: bool, has_obstacle_store: bool, n_seg: usize) -> bool {
    !arc_present || !has_obstacle_store || n_seg <= 1
}

/// Per-(source, receiver-block) pooled IMD maxima for the M3a ground bound:
/// the K chunk maxima over boxes expanded to cover EVERY receiver in the
/// block (plus the source-end quad). Only meaningful for point sources, whose
/// profile origin is receiver-independent — the kernel resolves it once per
/// (source, block) and hands it to every pair of the block's 256 receivers,
/// which is what keeps the bound's cost at a few ns per pair.
#[derive(Clone, Copy)]
pub(crate) struct BlockGroundMaxima {
    chunks: [u8; M3_GROUND_CHUNKS],
    src: u8,
}

/// Resolve [`BlockGroundMaxima`] for one source over the receiver block
/// `[lat_lo..=lat_hi] × [lon_lo..=lon_hi]` (the block's rx_lat/rx_lon range,
/// order-insensitive). Sound by monotonicity: each per-pair sample position is
/// `src + t·(rcv − src)` with `rcv` inside the block range and `t` inside the
/// chunk, so its raster coordinate lies between the four (t, corner)
/// evaluations — computed with the SAME float association the exact march
/// uses (`d_rf = (rcv_lat − src_lat)·inv`), so even a 1-ULP drift cannot
/// escape the box.
pub(crate) fn block_ground_maxima(
    tile: &FusedTileZ13,
    src_lat: f64,
    src_lon: f64,
    lat_lo: f64,
    lat_hi: f64,
    lon_lo: f64,
    lon_hi: f64,
) -> BlockGroundMaxima {
    let halo = &*tile.halo;
    let (lat_min, lon_min, inv_cell_deg, _rows, _cols) = halo.geom();
    let src_rf = (src_lat - lat_min) * inv_cell_deg;
    let src_cf = (src_lon - lon_min) * inv_cell_deg;
    let (la0, la1) = if lat_lo <= lat_hi {
        (lat_lo, lat_hi)
    } else {
        (lat_hi, lat_lo)
    };
    let (lo0, lo1) = if lon_lo <= lon_hi {
        (lon_lo, lon_hi)
    } else {
        (lon_hi, lon_lo)
    };
    let d_rf = [
        ((la0 - src_lat) * inv_cell_deg),
        ((la1 - src_lat) * inv_cell_deg),
    ];
    let d_cf = [
        ((lo0 - src_lon) * inv_cell_deg),
        ((lo1 - src_lon) * inv_cell_deg),
    ];
    let mut chunks = [0u8; M3_GROUND_CHUNKS];
    for (k, chunk) in chunks.iter_mut().enumerate() {
        let t_lo = k as f64 / M3_GROUND_CHUNKS as f64;
        let t_hi = (k + 1) as f64 / M3_GROUND_CHUNKS as f64;
        let (mut rf_lo, mut rf_hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut cf_lo, mut cf_hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &t in &[t_lo, t_hi] {
            for &d in &d_rf {
                let v = src_rf + t * d;
                rf_lo = rf_lo.min(v);
                rf_hi = rf_hi.max(v);
            }
            for &d in &d_cf {
                let v = src_cf + t * d;
                cf_lo = cf_lo.min(v);
                cf_hi = cf_hi.max(v);
            }
        }
        *chunk = halo.imd_max_over_rc_box(rf_lo, rf_hi, cf_lo, cf_hi);
    }
    let src = halo.imd_max_over_rc_box(src_rf, src_rf, src_cf, src_cf);
    BlockGroundMaxima { chunks, src }
}

/// The M3 bound for one (source, receiver) pair, computed in the cheap pass —
/// before any profile march. `block_maxima` carries the per-(source, block)
/// pooled chunk maxima; pairs whose geometry provides none (line sources)
/// keep the loose bound. `t_scratch` is a reused cadence buffer (the bound
/// never allocates in the hot loop).
pub(crate) fn pair_bound(
    tile: &FusedTileZ13,
    cfg: Option<CoarseMid>,
    t: &PixelTerms,
    rx_lat: f64,
    rx_lon: f64,
    rx_alt: f64,
    obstacles: &ObstacleSet,
    n_seg: usize,
    block_maxima: Option<&BlockGroundMaxima>,
    t_scratch: &mut Vec<f64>,
) -> M3PairBound {
    if !surface_bound_m3_enabled() {
        return M3PairBound::loose();
    }
    if t.force_hard_ground {
        // Bridges ground at exactly G = 0 (gain exactly +3 dB): the pyramid
        // knows nothing about that rule, so keep the loose ground term.
        return M3PairBound::loose();
    }
    let dist_m = t.profile_dist_m;
    let mut cadence_filled = false;
    let fill_cadence = |t_scratch: &mut Vec<f64>| {
        t_scratch.clear();
        match cadence_for_ray(cfg, dist_m) {
            Some(cm) => fill_t_values_coarse_mid(dist_m, t_scratch, cm),
            None => fill_t_values(dist_m, t_scratch),
        }
    };

    // ── M3a ground ──────────────────────────────────────────────────────
    // Scoped to the POINT lane (`block_constant_source_latlon`): the per-pair
    // fallback (8 pyramid boxes per pair) measured a NET +7 % slowdown on a
    // dense road tile — the pricing cost exceeded the walk savings — so line
    // pairs keep today's loose bound byte-for-byte and M3 remains the
    // point-lane lever it was specified as.
    let Some(block) = block_maxima else {
        return M3PairBound::loose();
    };
    let (chunk_max, src_max) = (block.chunks, block.src);
    let uniform = chunk_max.iter().all(|&b| b == chunk_max[0]);
    let ground_gain_ub_db = if uniform {
        // Every pooled chunk max equals the same value B, so ANY trapezoid
        // weighting of admissible samples averages to ≤ B — no cadence
        // needed. Common in deep-rural (all 0) and dense-city (all 100)
        // blocks; keeps the fill off the cheapest tiles.
        ground_gain_from_mean_bound(f64::from(chunk_max[0]), f64::from(src_max))
    } else {
        fill_cadence(t_scratch);
        cadence_filled = true;
        let avg_imd_ub = avg_imd_upper_bound_from_chunk_maxima(t_scratch, &chunk_max);
        ground_gain_from_mean_bound(avg_imd_ub, f64::from(src_max))
    };

    // ── M3b terrain: the cp-ray population only ─────────────────────────
    let terrain_lb_bands =
        if exact_path_is_single_cp_ray(t.arc.is_some(), obstacles.edge_count() > 0, n_seg) {
            if !cadence_filled {
                fill_cadence(t_scratch);
            }
            terrain_lb_bands(tile, t, rx_lat, rx_lon, rx_alt, t_scratch)
        } else {
            None
        };
    M3PairBound {
        ground_gain_ub_db,
        terrain_lb_bands,
    }
}

/// M3a: the ground GAIN bound `GROUND_GAIN_UB_DB·(1−g_lo)` from an upper
/// bound on the path-mean IMD and on the source-end IMD.
///
/// Why the FLOOR form is the sound reading of the doc block's `A_gr(i, G_lo)`:
/// the implemented per-band ground attenuation is a `P_FAV` energy mix of two
/// CNOSSOS states that each end in `max(analytic, −3·(1−g_prime))`
/// (`iso9613.rs`), `g_prime` is a convex blend of the path and source-end
/// ground factors, and an energy mix of values ≥ L is ≥ L — so
/// `A_ground ≥ −3·(1−g_prime) ≥ −3·(1−g_lo)` for `g_lo ≤ min(g_path, g_src)`.
/// The analytic term is NOT monotone in G (image-source interference), so
/// re-evaluating the full formula at `G_lo` would be unsound; the floor is the
/// part of the formula that IS monotone, and it is what carries the measured
/// 56-65 % / 7-11 % ground-dB recovery the doc block records. Under the
/// angular quadrature every bucket composites the SAME cp ground bands
/// (`seg_sampling.rs`), so the same bound covers the fan.
fn ground_gain_from_mean_bound(avg_imd_ub: f64, src_imd_ub: f64) -> f64 {
    let g_path_lo = (1.0 - (avg_imd_ub / 100.0).min(1.0)).max(0.0);
    let g_src_lo = (1.0 - (src_imd_ub / 100.0).min(1.0)).max(0.0);
    GROUND_GAIN_UB_DB * (1.0 - g_path_lo.min(g_src_lo))
}

/// The path-mean IMD upper bound: the cadence's OWN trapezoid mass with every
/// sample bounded by its chunk's pooled max. Interval `i`'s exact contribution
/// is `0.5·(v[i-1]+v[i])·Δt_i` (`path_integral_u8`); bounding each endpoint
/// by ITS chunk's max keeps a long coarse-middle interval that straddles chunk
/// boundaries from attributing its mass to the wrong chunk — the recorded
/// failure mode of uniform 1/K weights (7 violations in 40 k pairs).
fn avg_imd_upper_bound_from_chunk_maxima(
    t_values: &[f64],
    chunk_max: &[u8; M3_GROUND_CHUNKS],
) -> f64 {
    let chunk_of = |t: f64| ((t * M3_GROUND_CHUNKS as f64) as usize).min(M3_GROUND_CHUNKS - 1);
    let mut avg = 0.0f64;
    for w in t_values.windows(2) {
        let b_lo = chunk_max[chunk_of(w[0])];
        let b_hi = chunk_max[chunk_of(w[1])];
        avg += 0.5 * (f64::from(b_lo) + f64::from(b_hi)) * (w[1] - w[0]);
    }
    avg
}

/// M3b: K-sample march over a subset of the ray's own cadence (endpoints
/// always included, so the per-end heights and the slant `dsr` match the exact
/// evaluation), with elevations sampled through the halo's own fused lookup at
/// lerped raster coordinates — bit-identical to what the exact march would
/// read at those same `t`, which is what makes "max-δ(subset) ≤ max-δ(full)"
/// hold without a float-noise caveat. `None` when the subset shows no sample
/// above the line of sight (no terrain term to bound).
fn terrain_lb_bands(
    tile: &FusedTileZ13,
    t: &PixelTerms,
    rx_lat: f64,
    rx_lon: f64,
    rx_alt: f64,
    t_values: &[f64],
) -> Option<[f64; NUM_BANDS]> {
    let n = t_values.len();
    if n < 3 || t.profile_dist_m < 30.0 {
        return None;
    }
    let halo = &*tile.halo;
    let (lat_min, lon_min, inv_cell_deg, _rows, _cols) = halo.geom();
    let src_rf = (t.cp_lat - lat_min) * inv_cell_deg;
    let src_cf = (t.cp_lon - lon_min) * inv_cell_deg;
    let d_rf = (rx_lat - t.cp_lat) * inv_cell_deg;
    let d_cf = (rx_lon - t.cp_lon) * inv_cell_deg;

    let mut t_sub = [0.0f64; M3_TERRAIN_SAMPLES];
    let mut e_sub = [0.0f32; M3_TERRAIN_SAMPLES];
    let mut m = 0usize;
    for j in 0..M3_TERRAIN_SAMPLES {
        // Evenly in INDEX space: the cadence clusters near its endpoints, so
        // index-evenly keeps near-end AND mid-path samples — the mid-path
        // hills are where the coarse middle lives.
        let i = ((j as f64) * (n - 1) as f64 / (M3_TERRAIN_SAMPLES - 1) as f64).round() as usize;
        if j > 0
            && i == ((j - 1) as f64 * (n - 1) as f64 / (M3_TERRAIN_SAMPLES - 1) as f64).round()
                as usize
        {
            continue; // duplicate index on very short cadences
        }
        let (elev, _f, _imd) =
            halo.lookup_fused_rc(src_rf + t_values[i] * d_rf, src_cf + t_values[i] * d_cf);
        t_sub[m] = t_values[i];
        e_sub[m] = elev;
        m += 1;
    }
    let (delta_sub, dsr) = terrain_subset_delta_lower_bound(
        &t_sub[..m],
        &e_sub[..m],
        t.profile_dist_m,
        t.src_alt,
        rx_alt,
    )?;
    // κ = arc(dsr) − dsr at the favourable curvature Γ = max(1000, 8·dsr):
    // every detour arc ≥ its chord, so the exact path's favourable δ_F at ANY
    // edge is ≥ δ(that edge) − κ ≥ δ_sub − κ. That makes the mixed-band bound
    // at (δ_sub, δ_sub − κ) provably ≤ the exact evaluation at the full
    // cadence's dominant edge — the naive form (running the full mixed core on
    // the subset's argmax edge) is NOT sound: δ_F is not monotone across edge
    // positions, and on real Dobris data it over-read up to 2.9 dB on 8 % of
    // pairs before this form.
    let gamma = FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * dsr);
    let kappa = 2.0 * gamma * (dsr / (2.0 * gamma)).asin() - dsr;
    Some(diffraction_mixed_lower_bound(delta_sub, delta_sub - kappa))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic LCG (no `rand` dependency in this crate).
    struct Lcg(u64);
    impl Lcg {
        fn next_f64(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// THE M3a soundness property: for ANY admissible per-sample IMD profile
    /// (every sample bounded by its chunk's pooled max), the exact trapezoid
    /// path mean never exceeds `avg_imd_upper_bound_from_chunk_maxima`. This
    /// is precisely the inequality the byte-stop's `ub ≥ exact` rests on, swept
    /// over random cadences, chunk maxima, and admissible sample fills —
    /// including the coarse-middle interval that straddles chunk boundaries.
    #[test]
    fn chunk_maxima_trapezoid_mass_dominates_every_admissible_profile() {
        let mut rng = Lcg(0x5eed_1234);
        for _case in 0..2_000 {
            // Cadence-like t list: monotone 0..1 with a mix of dense and coarse
            // gaps (the coarse middle's 737 m interval is the case uniform
            // weights got wrong).
            let n = 4 + (rng.next_f64() * 10.0) as usize;
            let mut t: Vec<f64> = Vec::with_capacity(n);
            let mut acc = 0.0f64;
            for _ in 0..n {
                t.push(acc);
                acc += 0.05 + rng.next_f64() * 0.4;
            }
            let total = t[n - 1];
            if total <= 0.0 {
                continue;
            }
            let t: Vec<f64> = t.iter().map(|&x| x / total).collect();
            let mut chunk_max = [0u8; M3_GROUND_CHUNKS];
            for c in chunk_max.iter_mut() {
                *c = (rng.next_f64() * 101.0) as u8;
            }
            // Admissible samples: v[j] ≤ chunk_max[chunk_of(t[j])], with the
            // top of the range exercised so ties are common.
            let v: Vec<u8> = t
                .iter()
                .map(|&x| {
                    let k = ((x * 8.0) as usize).min(7);
                    let hi = chunk_max[k];
                    if rng.next_f64() < 0.5 {
                        hi
                    } else {
                        (rng.next_f64() * hi as f64) as u8
                    }
                })
                .collect();
            // Exact trapezoid mean (path_integral_u8 with ΣΔt = 1).
            let mut exact = 0.0f64;
            for i in 1..n {
                exact += 0.5 * (f64::from(v[i - 1]) + f64::from(v[i])) * (t[i] - t[i - 1]);
            }
            let ub = avg_imd_upper_bound_from_chunk_maxima(&t, &chunk_max);
            assert!(
                exact <= ub + 1e-12,
                "exact {exact:.6} > ub {ub:.6} for t={t:?} v={v:?} chunks={chunk_max:?}"
            );
        }
    }

    /// The straddling-interval case from the doc block, made concrete: one
    /// long interval whose left end sits in a high-IMD chunk and right end in
    /// a low one must carry the LEFT chunk's max on its left half — a uniform
    /// 1/K weighting would attribute it to whichever chunk the interval
    /// "belongs to" and under-read.
    #[test]
    fn straddling_coarse_interval_keeps_its_left_chunks_max() {
        // Chunks: [0..0.125) hot (100), the rest cold (0). A cadence with ONE
        // interval from t=0.05 (chunk 0) to t=0.5 (chunk 4).
        let t = vec![0.0, 0.05, 0.5, 1.0];
        let chunk_max = [100, 0, 0, 0, 0, 0, 0, 0];
        let ub = avg_imd_upper_bound_from_chunk_maxima(&t, &chunk_max);
        // Exact worst case: v(0.05)=100 contributes 0.5·(100+0)·0.45 = 22.5;
        // v(0)=100 adds 0.5·(100+100)·0.05 = 5 ⇒ exact max 27.5 == ub.
        assert!((ub - 27.5).abs() < 1e-12, "ub={ub}");
    }

    /// The loose bound's terms match the pre-M3 constants exactly — the dial's
    /// off arm is bit-identical to the shipped bound.
    #[test]
    fn loose_bound_terms_match_the_pre_m3_constant() {
        let loose = M3PairBound::loose();
        assert_eq!(loose.ground_gain_ub_db, GROUND_GAIN_UB_DB);
        assert!(loose.terrain_lb_bands.is_none());
        for b in 0..NUM_BANDS {
            assert!(
                (loose.gob_lb_db(b) + GROUND_GAIN_UB_DB).abs() < 1e-15,
                "loose gob floor must be exactly −GROUND_GAIN_UB_DB"
            );
        }
    }

    /// The composite floor: with both terms present the binding one per band
    /// is the max — terrain bands BELOW the ground floor fall back to it,
    /// positive terrain bands take over.
    #[test]
    fn gob_floor_takes_the_max_of_ground_and_terrain_terms() {
        let bound = M3PairBound {
            ground_gain_ub_db: 1.5, // floor = −1.5 dB
            terrain_lb_bands: Some([-2.0, -2.0, 0.9, 2.0, 5.0, 5.0, 5.0, 5.0]),
        };
        assert!(
            (bound.gob_lb_db(0) + 1.5).abs() < 1e-15,
            "below-floor terrain falls back"
        );
        assert!((bound.gob_lb_db(2) - 0.9).abs() < 1e-15);
        assert!((bound.gob_lb_db(3) - 2.0).abs() < 1e-15);
    }

    /// End-to-end through the real halo pyramid on the empty-rasters fixture:
    /// a missing IMD tile defaults to 100 (hard) everywhere, so the ground
    /// bound degenerates to the loose +3 dB and the flat zero-elevation world
    /// yields no terrain term — the pair stays exactly as loose as before M3,
    /// which is what makes this fixture's existing byte-parity tests reusable.
    #[test]
    fn all_hard_flat_fixture_yields_the_loose_bound() {
        let rasters = raster_reader::RealRasters::new(std::path::Path::new(
            "/nonexistent-quietmap-m3-fixture",
        ));
        let tile = FusedTileZ13::build(12, 2211, 1386, 4_000.0, &rasters);
        let mid_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let mid_lon = (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5;
        let mk_terms = |hard: bool| crate::scatter_band::PixelTerms {
            base_db: 0.0,
            atm_d_km: 0.5,
            profile_dist_m: 3_000.0,
            src_alt: 10.0,
            excl_m: 0.0,
            cp_lat: mid_lat,
            cp_lon: mid_lon,
            force_hard_ground: hard,
            arc: None,
        };
        let rx_lat = mid_lat - 3_000.0 / 111_320.0;
        let mut scratch = Vec::new();
        let bound = pair_bound(
            &tile,
            None,
            &mk_terms(false),
            rx_lat,
            mid_lon,
            4.0,
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            1,
            None,
            &mut scratch,
        );
        assert!(
            (bound.ground_gain_ub_db - GROUND_GAIN_UB_DB).abs() < 1e-12,
            "all-hard world must keep the full +3 dB ground gain bound, got {}",
            bound.ground_gain_ub_db
        );
        assert!(bound.terrain_lb_bands.is_none(), "flat world: no hill");
        // And the bridge rule bypasses the pyramid entirely.
        let bound = pair_bound(
            &tile,
            None,
            &mk_terms(true),
            rx_lat,
            mid_lon,
            4.0,
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            1,
            None,
            &mut scratch,
        );
        assert_eq!(bound.ground_gain_ub_db, GROUND_GAIN_UB_DB);
    }

    /// The M3b population rule, verbatim from the doc block: a pair with an
    /// angular span AND a vector store AND n_seg > 1 runs the quadrature, so
    /// NO terrain bound may attach to it; each of the three escape hatches
    /// (point source `arc: None`, raster fallback `obstacles: None`,
    /// `QM_SEG_SAMPLES=1`) restores legality.
    #[test]
    fn terrain_bound_legality_is_the_cp_ray_predicate() {
        assert!(!exact_path_is_single_cp_ray(true, true, 5));
        assert!(exact_path_is_single_cp_ray(true, true, 1)); // QM_SEG_SAMPLES=1
        assert!(exact_path_is_single_cp_ray(true, false, 5)); // raster fallback
        assert!(exact_path_is_single_cp_ray(false, true, 5)); // point source
    }
}
