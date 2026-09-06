//! C2 receiver terrain horizon for airborne Doc 29 screening.
//!
//! Doc 29 has no explicit terrain-horizon or barrier term: its empirical
//! lateral attenuation already covers prescribed ground/terrain effects, so a
//! low aircraft behind an individual ridge otherwise keeps the same SEL as an
//! unobstructed path. Precedent for a terrain correction on an NPD kernel:
//! FAA AEDT 3f Technical Manual §4.3.6,
//! "Line-of-Sight Blockage" — terrain path-length difference δ → barrier
//! theory, capped at 18 dB, applied as max(LOS_adj, Λ) (mutually
//! exclusive with lateral attenuation, never summed; Berton, AIAA 2021).
//! The insertion-loss form here is the ISO 9613-2 §7.4 single-edge
//! `Dz = 10·log10(3 + (C2/λ)·C3·δ)` — the same form the surface layers
//! use (SPEC §4.6) — with λ_eff = 0.685 m (500 Hz broadband-
//! representative aircraft spectrum, audit §F.2): 20/0.685 ≈ 29.2.
//! ISO 9613-2:2024 explicitly excludes aircraft in flight; using its
//! diffraction shape here is Quiet Map's approximation, not ISO compliance.
//!
//! Input is the DEM terrain surface (DSM-biased: GLO-30 includes canopy
//! and buildings), NOT bare-earth — forested ridges screen slightly
//! high. `build` is sampler-agnostic so the popup (`RealRasters` bilinear)
//! and heatmap receiver grids (`FusedTileZ13`) share one implementation.

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;

use super::doc29::{fast_atan, M_PER_DEG_LAT};
use super::screening::single_edge_diffraction_db;
#[cfg(test)]
use super::screening::DIFFRACTION_CAP_DB;

/// Azimuth sectors (11.25° each).
pub const HORIZON_SECTORS: usize = 32;

/// RANGE-max radial bands: each band stores the strongest edge WITHIN
/// its own (prev_break, break] annulus. Distance banding fixes the
/// "ridge beyond the aircraft" fallacy — an edge can only screen when
/// it lies between receiver and aircraft.
///
/// WHY range-max, not cumulative-max (dual /gg 2026-06-12 consensus):
/// a cumulative bucket keeps only the single steepest edge up to its
/// breakpoint, so a steeper FARTHER ridge erases a nearer still-blocking
/// one — an aircraft between the two then screens against nothing
/// (nested ridgelines are the normal mountain-valley case C2 exists
/// for). Per-band maxima keep one candidate edge per annulus; the
/// query ranks candidates by their actual Dz, so a far edge with the
/// larger exact path difference wins over a near low lip even at a
/// smaller horizon angle. Within-band erasure remains only when both
/// ridges fall in ONE band — bands are sized to keep that rare.
pub const RECEIVER_HORIZON_BANDS: usize = 6;
pub const RECEIVER_HORIZON_MAX_M: f64 = 8_000.0;
pub const RECEIVER_HORIZON_TANGENT_SCALE: f64 = 4096.0;
/// Stored terrain ranges are whole metres. Ceiling the range moves a packed
/// edge away from the receiver, so quantization cannot make an edge screen an
/// aircraft that is physically in front of it. The GPU receives this same
/// scale from `build.rs` and applies the same ceiling rule.
pub const RECEIVER_HORIZON_RANGE_SCALE: f64 = 1.0;
const BUCKET_BREAK_M: [f64; RECEIVER_HORIZON_BANDS] =
    [500.0, 1_000.0, 2_000.0, 3_500.0, 5_500.0, 8_000.0];
const _: () = assert!(BUCKET_BREAK_M[RECEIVER_HORIZON_BANDS - 1] == RECEIVER_HORIZON_MAX_M);

/// Exponential radial march 30 m → 8 km, 48 samples/sector (~1.5 k
/// DEM reads per receiver) — matches the surface bilateral-cadence
/// philosophy (dense near the receiver where angular resolution
/// matters, coarse far out where the horizon changes slowly).
const MARCH_START_M: f64 = 30.0;
pub const RECEIVER_HORIZON_MARCH_SAMPLES: usize = 48;

/// One immutable point in the radial terrain march. The CPU and CUDA builders
/// consume this table so radii, directions, and range-band assignment cannot
/// drift between painters.
#[derive(Clone, Copy)]
pub struct ReceiverHorizonSample {
    pub range_m: f64,
    pub north_m: f64,
    pub east_m: f64,
    pub band: usize,
}

pub fn receiver_horizon_samples(
) -> &'static [[ReceiverHorizonSample; RECEIVER_HORIZON_MARCH_SAMPLES]; HORIZON_SECTORS] {
    static SAMPLES: OnceLock<
        [[ReceiverHorizonSample; RECEIVER_HORIZON_MARCH_SAMPLES]; HORIZON_SECTORS],
    > = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let growth = (RECEIVER_HORIZON_MAX_M / MARCH_START_M)
            .powf(1.0 / (RECEIVER_HORIZON_MARCH_SAMPLES - 1) as f64);
        std::array::from_fn(|sector| {
            let azimuth = (sector as f64 + 0.5) * (TAU / HORIZON_SECTORS as f64);
            let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
            let mut range_m = MARCH_START_M;
            std::array::from_fn(|_| {
                let sample = ReceiverHorizonSample {
                    range_m,
                    north_m: range_m * sin_azimuth,
                    east_m: range_m * cos_azimuth,
                    band: BUCKET_BREAK_M
                        .iter()
                        .position(|&break_m| range_m <= break_m + 0.5)
                        .unwrap_or(RECEIVER_HORIZON_BANDS - 1),
                };
                range_m *= growth;
                sample
            })
        })
    })
}

/// Fixed-point tan quantization: i16 × 1/4096.
///
/// WHY i16 (not u16+offset): horizons are SIGNED — a hilltop receiver
/// sees below horizontal (negative tan) and an aircraft below the
/// receiver can still be blocked by the downslope. Zero-centered i16 is
/// the simplest correct encoding. Step 2^-12 ≈ 0.014° near horizontal
/// (finer at steep angles); range ±8.0 tan ≈ ±82.9°, clamped beyond
/// (slot-canyon horizons above 82.9° under-screen — acceptably rare).
/// WHY quantize: 4 B/entry keeps per-tile CPU and GPU storage bounded;
/// the popup shares the struct so its query is the same calculation.
/// Number of packed `(tangent, range)` entries in one receiver horizon.
pub const RECEIVER_HORIZON_ENTRY_COUNT: usize = HORIZON_SECTORS * RECEIVER_HORIZON_BANDS;

/// The §7.4 form's value at δ = 0 is removed so Dz → 0
/// exactly at the shadow boundary: the raw form lands at 4.77 dB when
/// the ray grazes the edge, and applying it blocked-only would step
/// 4.77 dB across one pixel (the hard cut the plan rejects). Anchoring
/// at 0 also makes the `max_sin_sq` kernel precheck EXACT — every pair
/// above every stored horizon gets Dz = 0 whether prechecked away or
/// fully evaluated. Deep shadow still reaches the full 18 dB cap.
/// Per-receiver terrain horizon: 32 azimuth sectors × 6 range-max
/// radial bands of (quantized horizon tangent, edge distance of the
/// band's maximum). Built once per popup receiver or selected heatmap
/// receiver node and queried inside `segment_energy_kernel`.
#[derive(Clone, Copy)]
pub struct ReceiverHorizon {
    /// `[sector][band] = (tan_q, r_q)`; `tan_q` = horizon tangent ×
    /// `RECEIVER_HORIZON_TANGENT_SCALE` (i16, signed — see above), `r_q` = ceil(edge distance ×
    /// `RECEIVER_HORIZON_RANGE_SCALE`) in whole metres (u16, ≤ 8 000) of the band's own maximum;
    /// ceiling keeps a packed edge away from the receiver; `r_q = 0`
    /// marks a band the march never sampled (skipped at query).
    sectors: [[(i16, u16); RECEIVER_HORIZON_BANDS]; HORIZON_SECTORS],
    /// sin² of the highest stored horizon across all sectors/buckets,
    /// clamped to 0 when every horizon is below horizontal. Kernel
    /// precheck: `rel_alt <= 0 || rel_alt² < slant_sq · max_sin_sq`
    /// (delta 3) — per-receiver-tight, skips the sector lookup for all
    /// geometry above every horizon. Computed from the QUANTIZED tans
    /// so precheck and `screening_dz` can never disagree.
    pub max_sin_sq: f64,
}

impl ReceiverHorizon {
    /// March the DEM terrain surface (DSM-biased) outward from the
    /// receiver and record per-sector range-max horizons.
    /// `receiver_alt_m` must be the same datum the kernel's `rel_alt`
    /// uses (popup: `Receiver::altitude_m()`, terrain + mast height).
    /// The radial geometry uses the kernel's receiver-local
    /// equirectangular projection (`M_PER_DEG_LAT`, cos at receiver
    /// latitude) so horizon-space distances match kernel `lateral`.
    /// All-unsampled horizon: every band `(0, 0)` (skipped at query) and
    /// `max_sin_sq = 0` (the kernel precheck then never consults a sector).
    pub const EMPTY: ReceiverHorizon = ReceiverHorizon {
        sectors: [[(0i16, 0u16); RECEIVER_HORIZON_BANDS]; HORIZON_SECTORS],
        max_sin_sq: 0.0,
    };

    pub fn build(
        sampler: impl Fn(f64, f64) -> f64,
        lat: f64,
        lon: f64,
        receiver_alt_m: f64,
    ) -> Self {
        let mut one = [ReceiverHorizon::EMPTY];
        build_receiver_horizon_row(sampler, lat, &[(lon, receiver_alt_m)], &mut one);
        one[0]
    }

    /// Screening insertion loss (dB ≥ 0) for an aircraft whose CPA foot
    /// sits at receiver-local (`cpa_east_m`, `cpa_north_m`) — the
    /// kernel's (cpx, cpy) — with `lateral_m` = √(lateral_sq) and signed
    /// `rel_alt_m`. Returns 0 when unblocked.
    ///
    /// Band rule (delta 4, sharpened by the dual /gg consensus): every
    /// stored band edge with `r_edge ≤ lateral` is a CANDIDATE (an edge
    /// beyond the aircraft cannot screen), and the result is the MAX Dz
    /// over candidates — NOT the max horizon angle. The horizon angle
    /// is the wrong ranking proxy: through the L/(L−r) factor a far
    /// ridge near the aircraft can cap at 18 dB while a near low lip at
    /// a STEEPER angle yields a few dB.
    ///
    /// δ (delta 5) is the EXACT 2D single-edge path difference
    /// |RE| + |ES| − |RS| in the vertical receiver–aircraft plane, edge
    /// at (r_edge, r_edge·tan_h) — the small-angle r·(h−β)²/2 form drops
    /// the L/(L−r) factor and under-screens 3-11× near the ridge.
    #[inline]
    pub fn screening_dz(
        &self,
        cpa_east_m: f64,
        cpa_north_m: f64,
        lateral_m: f64,
        rel_alt_m: f64,
    ) -> f64 {
        let bands = &self.sectors[sector_of(cpa_east_m, cpa_north_m)];
        // The strict `<` below makes a NaN ratio compare unblocked.
        let tan_beta = rel_alt_m / lateral_m;
        let rs = (lateral_m * lateral_m + rel_alt_m * rel_alt_m).sqrt();
        let mut best_dz = 0.0_f64;
        for &(tan_q, r_q) in bands {
            // r_q = 0 marks an unsampled band; real edges have r ≥ 30 m.
            if r_q == 0 || (r_q as f64) > lateral_m {
                continue;
            }
            let tan_h = tan_q as f64 / RECEIVER_HORIZON_TANGENT_SCALE;
            // `!(a < b)` NOT `a >= b`: a NaN `tan_beta` (vertical / zero lateral)
            // must take the `continue` branch (edge does not block), which only the
            // negated strict `<` gives — `>=` would be false for NaN and wrongly
            // admit the edge. See the strict-`<` note above.
            #[allow(clippy::neg_cmp_op_on_partial_ord)]
            if !(tan_beta < tan_h) {
                continue; // this edge does not break the line of sight
            }
            // The packed ceiling range is the conservative eligibility edge:
            // an edge whose true range falls between the source and `r_q` is
            // rejected above. The old nearest-metre representation could have
            // been either end of this one-quantum interval, so use the smaller
            // diffraction of both endpoints. It is no greater than the old
            // value whichever endpoint nearest rounding selected, while the
            // CPU/GPU eligibility decision shares this ceiling-packed rule.
            let edge_dz = [(r_q - 1) as f64, r_q as f64]
                .into_iter()
                .map(|r_q| {
                    let r = r_q / RECEIVER_HORIZON_RANGE_SCALE;
                    let edge_z = r * tan_h;
                    let re = (r * r + edge_z * edge_z).sqrt();
                    let dx = lateral_m - r;
                    let dz = rel_alt_m - edge_z;
                    let es = (dx * dx + dz * dz).sqrt();
                    let delta = (re + es - rs).max(0.0);
                    single_edge_diffraction_db(delta)
                })
                .fold(f64::INFINITY, f64::min);
            best_dz = best_dz.max(edge_dz);
        }
        best_dz
    }
}

/// March the DEM for a whole ROW of receivers sharing one latitude, writing
/// one horizon per entry of `receivers` (`(lon, receiver_alt_m)`) into `out`.
///
/// Same arithmetic and the same per-(receiver, sector, band) reduction order
/// as a per-receiver march, so the horizons are bit-identical — only the loop
/// order changes. The march offsets are receiver-independent, so one
/// (sector, sample) step lands on ONE DEM row for the whole receiver row and
/// the receivers' longitudes walk that row with a fixed stride. A
/// per-receiver march instead scatters its 1 536 samples over the whole 8 km
/// halo, and at the halo's 1 arcsec lattice (~30 m, five times coarser than a
/// z13 receiver pixel) neighbouring receivers each re-fetched the same cells.
///
/// Per-sector band maxima are held band-major (`RECEIVER_HORIZON_BANDS ×
/// receivers`) so one sample step — whose band is fixed — writes a contiguous
/// run.
pub fn build_receiver_horizon_row(
    sampler: impl Fn(f64, f64) -> f64,
    lat: f64,
    receivers: &[(f64, f64)],
    out: &mut [ReceiverHorizon],
) {
    assert_eq!(receivers.len(), out.len(), "one horizon per receiver");
    let n = receivers.len();
    let cos_lat = lat.to_radians().cos().max(0.2);
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;
    let mut max_tan_q = vec![i16::MIN; n];
    let mut best_tan = vec![f64::NEG_INFINITY; RECEIVER_HORIZON_BANDS * n];
    let mut best_range = vec![0.0_f64; RECEIVER_HORIZON_BANDS * n];
    out.fill(ReceiverHorizon::EMPTY);

    for (sector, samples) in receiver_horizon_samples().iter().enumerate() {
        best_tan.fill(f64::NEG_INFINITY);
        best_range.fill(0.0);
        for sample in samples {
            // Antimeridian wrap + pole clamp keep the DEM SAMPLING valid near
            // ±180°/the poles. NOTE: the kernel-side aircraft projections
            // (segment_sel) are dateline-naive (pre-existing —
            // dateline-crossing pairs never reach the kernel; their raw lon
            // delta fails every bbox prefilter), so this wrap is about sane
            // sampling, not end-to-end dateline support.
            let sample_lat = (lat + sample.north_m / M_PER_DEG_LAT).clamp(-90.0, 90.0);
            let east_deg = sample.east_m / m_per_deg_lon;
            let band = sample.band * n;
            let tans = &mut best_tan[band..band + n];
            let ranges = &mut best_range[band..band + n];
            for (i, &(lon, receiver_alt_m)) in receivers.iter().enumerate() {
                let sample_lon = (lon + east_deg + 540.0).rem_euclid(360.0) - 180.0;
                let tangent = (sampler(sample_lat, sample_lon) - receiver_alt_m) / sample.range_m;
                if tangent > tans[i] {
                    tans[i] = tangent;
                    ranges[i] = sample.range_m;
                }
            }
        }
        for b in 0..RECEIVER_HORIZON_BANDS {
            for i in 0..n {
                let r_edge = best_range[b * n + i];
                if r_edge == 0.0 {
                    continue; // band never sampled — stays (0, 0), skipped at query
                }
                let tan_q = (best_tan[b * n + i] * RECEIVER_HORIZON_TANGENT_SCALE)
                    .round()
                    .clamp(i16::MIN as f64, i16::MAX as f64) as i16;
                let range_q = (r_edge * RECEIVER_HORIZON_RANGE_SCALE)
                    .ceil()
                    .min(u16::MAX as f64) as u16;
                out[i].sectors[sector][b] = (tan_q, range_q);
                max_tan_q[i] = max_tan_q[i].max(tan_q);
            }
        }
    }

    for (horizon, &tan_q) in out.iter_mut().zip(&max_tan_q) {
        let max_tan = tan_q as f64 / RECEIVER_HORIZON_TANGENT_SCALE;
        // All horizons at/below horizontal: no positive-rel_alt pair can ever
        // be blocked → precheck skips them all; negative rel_alt still routes
        // through the explicit `<= 0` arm.
        horizon.max_sin_sq = if max_tan > 0.0 {
            (max_tan * max_tan) / (1.0 + max_tan * max_tan)
        } else {
            0.0
        };
    }
}

/// Azimuth sector of a receiver-local (east, north) offset. Angle is
/// measured CCW from +east — only build/query consistency matters, not
/// compass convention. `fast_atan` octant fold (~10 cyc); its 0.17° max
/// error can pick a neighbouring 11.25° sector near a boundary, where
/// adjacent terrain horizons are similar — accepted.
#[inline]
fn sector_of(east_m: f64, north_m: f64) -> usize {
    if east_m.abs() < 1e-9 && north_m.abs() < 1e-9 {
        return 0; // degenerate overhead foot; caller's bucket rule yields 0 dB
    }
    let angle = if east_m.abs() >= north_m.abs() {
        let a = fast_atan(north_m / east_m);
        if east_m >= 0.0 {
            a
        } else {
            a + PI
        }
    } else {
        let a = FRAC_PI_2 - fast_atan(east_m / north_m);
        if north_m >= 0.0 {
            a
        } else {
            a - PI
        }
    };
    ((angle * (HORIZON_SECTORS as f64) / TAU).floor() as i64).rem_euclid(HORIZON_SECTORS as i64)
        as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const RX_LAT: f64 = 50.0;
    const RX_LON: f64 = 14.0;

    /// Sampler over receiver-local east/north meters, inverting the
    /// build's own projection so test ridges sit at exact local offsets.
    fn local_sampler(f: impl Fn(f64, f64) -> f64) -> impl Fn(f64, f64) -> f64 {
        let m_per_deg_lon = M_PER_DEG_LAT * RX_LAT.to_radians().cos().max(0.2);
        move |lat: f64, lon: f64| {
            f(
                (lon - RX_LON) * m_per_deg_lon,
                (lat - RX_LAT) * M_PER_DEG_LAT,
            )
        }
    }

    /// East-facing ridge band `[r0, r1]` at `top_m`, else flat `base_m`.
    fn east_ridge(r0: f64, r1: f64, top_m: f64, base_m: f64) -> impl Fn(f64, f64) -> f64 {
        local_sampler(move |east, north| {
            if east >= r0 && east <= r1 && north.abs() < 600.0 {
                top_m
            } else {
                base_m
            }
        })
    }

    #[test]
    fn flat_terrain_zero_max_sin_sq() {
        // Receiver mast 4 m above uniform ground: every horizon tan is
        // negative ⇒ max_sin_sq clamps to exactly 0 ⇒ the kernel
        // precheck skips every positive-rel_alt pair.
        let hz = ReceiverHorizon::build(|_, _| 300.0, RX_LAT, RX_LON, 304.0);
        assert_eq!(hz.max_sin_sq, 0.0);
        // And the full query agrees: nothing above horizontal screens.
        assert_eq!(hz.screening_dz(2_000.0, 0.0, 2_000.0, 150.0), 0.0);
    }

    #[test]
    fn single_ridge_lands_in_correct_sector_and_band() {
        // 200 m ridge band 1.9-2.1 km due east over flat 300 m ground.
        let hz = ReceiverHorizon::build(
            east_ridge(1_900.0, 2_100.0, 500.0, 300.0),
            RX_LAT,
            RX_LON,
            300.0,
        );

        // Sector 0 (az 0-11.25°, marched at 5.625°): ONLY the
        // (1 km, 2 km] band holds the ridge (range-max — bands beyond
        // it stay flat, unlike the rejected cumulative carry).
        let ridge_band = hz.sectors[0][2];
        let t = ridge_band.0 as f64 / RECEIVER_HORIZON_TANGENT_SCALE;
        assert!(
            (0.095..0.110).contains(&t),
            "(1,2] km band tan = {t} (expected ≈ 200 m / ~1.9-2.0 km)"
        );
        assert!(
            (1_900..=2_100).contains(&ridge_band.1),
            "edge distance {} outside ridge",
            ridge_band.1
        );
        for b in [0, 1, 3, 4, 5] {
            assert!(
                hz.sectors[0][b].0 <= 0,
                "band {b} must stay flat under range-max, got tan_q {}",
                hz.sectors[0][b].0
            );
        }

        // Opposite sector (due west) sees only flat ground.
        assert!(hz.sectors[16][2].0 <= 0);

        // max_sin_sq derives from the quantized global max tan.
        assert!((hz.max_sin_sq - t * t / (1.0 + t * t)).abs() < 1e-12);
    }

    /// Range-max regression (Gemini /gg 2026-06-12): a NEARER, LOWER
    /// ridge must survive a STEEPER ridge farther out — the rejected
    /// cumulative-max stored only the far ridge, whose r_edge > lateral
    /// then disqualified it, leaving an aircraft BETWEEN nested ridges
    /// unscreened (the normal mountain-valley case C2 exists for).
    #[test]
    fn nested_ridges_nearer_lower_still_screens() {
        let sampler = local_sampler(|east, north| {
            if north.abs() >= 600.0 {
                300.0
            } else if (3_800.0..=4_200.0).contains(&east) {
                710.0 // tan ≈ 0.10 from the receiver
            } else if (6_400.0..=7_200.0).contains(&east) {
                1_200.0 // tan ≈ 0.125 — steeper but FARTHER
            } else {
                300.0
            }
        });
        let hz = ReceiverHorizon::build(sampler, RX_LAT, RX_LON, 304.0);
        let dz = hz.screening_dz(5_000.0, 0.0, 5_000.0, 250.0);
        assert!(
            dz > 0.0,
            "nearer 4 km ridge must screen despite the steeper 7 km one, got {dz}"
        );
    }

    /// Dz-ranking regression (Codex /gg 2026-06-12): a far edge close
    /// under the aircraft (large exact δ via the L/(L−r) factor) must
    /// outrank a near low lip at a STEEPER horizon angle — ranking by
    /// horizon angle alone kept the lip's few dB and dropped the far
    /// ridge's 18 dB cap.
    #[test]
    fn far_edge_with_larger_delta_outranks_near_lip() {
        let sampler = local_sampler(|east, north| {
            if north.abs() >= 600.0 {
                300.0
            } else if (370.0..=440.0).contains(&east) {
                350.0 // lip: tan ≈ 0.122 (steepest)
            } else if (4_200.0..=4_800.0).contains(&east) {
                750.0 // ridge: tan ≈ 0.10, a few hundred m from the aircraft
            } else {
                300.0
            }
        });
        let hz = ReceiverHorizon::build(sampler, RX_LAT, RX_LON, 300.0);
        // β tan = 0.08 — under both edges. Exact δ: lip ≈ 0.3 m (~6 dB
        // anchored), ridge ≈ 9 m (18 dB cap).
        let dz = hz.screening_dz(5_000.0, 0.0, 5_000.0, 400.0);
        assert!(
            dz > 15.0,
            "far ridge's δ must dominate the near lip, got {dz}"
        );
    }

    /// Delta 4 regression: a 4 km ridge must screen a 5 km-lateral
    /// aircraft. The rejected "bucket whose breakpoint ≤ lateral" rule
    /// would select the ≤3 km bucket (flat) and miss the ridge held by
    /// the ≤8 km bucket.
    #[test]
    fn bucket_blind_zone_4km_ridge_5km_lateral_screens() {
        let hz = ReceiverHorizon::build(
            east_ridge(3_900.0, 4_100.0, 710.0, 300.0),
            RX_LAT,
            RX_LON,
            304.0,
        );
        // Aircraft at β ≈ 2.9° (tan 0.05), below the ≈ 0.105 ridge tan.
        let dz = hz.screening_dz(5_000.0, 0.0, 5_000.0, 250.0);
        assert!(
            dz > 0.0,
            "4 km ridge must bind for a 5 km-lateral aircraft, got {dz}"
        );
    }

    /// Ridge beyond the aircraft cannot screen: every stored edge has
    /// r_edge > lateral, so no bucket qualifies.
    #[test]
    fn ridge_beyond_aircraft_does_not_screen() {
        let hz = ReceiverHorizon::build(
            east_ridge(3_900.0, 4_100.0, 710.0, 300.0),
            RX_LAT,
            RX_LON,
            304.0,
        );
        let dz = hz.screening_dz(3_500.0, 0.0, 3_500.0, 175.0);
        assert_eq!(
            dz, 0.0,
            "edge at ~3.9 km lies beyond a 3.5 km-lateral aircraft"
        );
    }

    #[test]
    fn range_ceiling_cannot_move_a_terrain_edge_in_front_of_source() {
        // Pick a march sample whose true range would round DOWN under the old
        // nearest-metre pack. Put the source between that old packed range and
        // the true edge: nearest packing would admit it, while conservative
        // ceiling must leave the edge beyond the source and return no Dz.
        let sample = receiver_horizon_samples()[0]
            .iter()
            .find(|sample| sample.range_m > 500.0 && (0.1..0.4).contains(&sample.range_m.fract()))
            .expect("march table must contain a fractional range below half a metre");
        let true_range = sample.range_m;
        let old_nearest_range = true_range.floor();
        let source_lateral = (old_nearest_range + true_range) * 0.5;
        let (sin_azimuth, cos_azimuth) = (TAU * 0.5 / HORIZON_SECTORS as f64).sin_cos();
        let hz = ReceiverHorizon::build(
            local_sampler(move |east, north| {
                if (east - sample.east_m).hypot(north - sample.north_m) < 0.5 {
                    800.0
                } else {
                    300.0
                }
            }),
            RX_LAT,
            RX_LON,
            300.0,
        );

        let range_q = hz.sectors[0][sample.band].1 as f64 / RECEIVER_HORIZON_RANGE_SCALE;
        assert!(
            range_q > source_lateral,
            "ceiling must keep edge beyond source: true={true_range}, packed={range_q}, source={source_lateral}"
        );
        assert_eq!(
            hz.screening_dz(
                source_lateral * cos_azimuth,
                source_lateral * sin_azimuth,
                source_lateral,
                0.0,
            ),
            0.0,
            "a terrain edge beyond the source must not screen"
        );
    }

    /// Signed horizons: a hilltop receiver above a downslope
    /// stores negative tans; an aircraft below the receiver is blocked
    /// only when its (negative) β drops under the (negative) horizon.
    /// Also covers the negative-rel_alt path: no NaN / no squaring trap.
    #[test]
    fn negative_horizon_blocks_only_below_downslope() {
        // Terrain falls at −0.1 from a 1 000 m summit in every direction.
        let sampler =
            local_sampler(|east, north| 1_000.0 - 0.1 * (east * east + north * north).sqrt());
        let hz = ReceiverHorizon::build(sampler, RX_LAT, RX_LON, 1_000.0);
        assert_eq!(hz.max_sin_sq, 0.0, "all-downhill horizon must clamp to 0");

        // β = −2.9° (above the −5.7° horizon): line of sight over the slope.
        assert_eq!(hz.screening_dz(1_000.0, 0.0, 1_000.0, -50.0), 0.0);
        // β = −11.3° (below the horizon): blocked by the near slope edge.
        let dz = hz.screening_dz(1_000.0, 0.0, 1_000.0, -200.0);
        assert!(
            dz.is_finite() && dz > 0.0,
            "below-horizon descent must screen, got {dz}"
        );
    }

    #[test]
    fn dz_cap_is_18_db() {
        // Near-unity-tan wall at ~500 m, aircraft at β = 0 → δ ≈ 250 m,
        // far past the ~6.4 m needed to saturate the cap.
        let hz = ReceiverHorizon::build(
            east_ridge(480.0, 530.0, 800.0, 300.0),
            RX_LAT,
            RX_LON,
            300.0,
        );
        let dz = hz.screening_dz(3_000.0, 0.0, 3_000.0, 0.0);
        assert_eq!(dz, DIFFRACTION_CAP_DB);
    }

    /// Maekawa-smooth boundary: sweeping β in 0.1° steps across the
    /// horizon angle, consecutive Dz steps stay < 0.5 dB and Dz meets 0
    /// exactly at/above the boundary. (The in-shadow ramp steepens with
    /// edge distance and L/(L−r) — that slope is diffraction physics;
    /// the criterion guards the BOUNDARY against the rejected hard cut.)
    #[test]
    fn dz_smooth_across_shadow_boundary() {
        // Gentle edge: tan ≈ 0.126 at ~158 m, aircraft lateral 5 km.
        let hz = ReceiverHorizon::build(
            east_ridge(150.0, 250.0, 320.0, 300.0),
            RX_LAT,
            RX_LON,
            300.0,
        );
        let lateral = 5_000.0;
        let mut prev: Option<f64> = None;
        let mut saw_blocked = false;
        let mut saw_open = false;
        for step in 0..=10 {
            let beta_deg = 6.7 + 0.1 * step as f64;
            let rel_alt = lateral * beta_deg.to_radians().tan();
            let dz = hz.screening_dz(lateral, 0.0, lateral, rel_alt);
            if dz > 0.0 {
                saw_blocked = true;
            } else {
                saw_open = true;
            }
            if let Some(p) = prev {
                assert!(
                    (dz - p).abs() < 0.5,
                    "Dz step {} dB between β {:.1}° and {:.1}°",
                    (dz - p).abs(),
                    beta_deg - 0.1,
                    beta_deg
                );
            }
            prev = Some(dz);
        }
        assert!(
            saw_blocked && saw_open,
            "sweep must straddle the shadow boundary"
        );
    }

    #[test]
    fn sector_of_cardinal_directions() {
        assert_eq!(sector_of(1_000.0, 100.0), 0); // az 5.7° (east-ish)
        assert_eq!(sector_of(100.0, 1_000.0), 7); // az 84.3° (north-ish)
        assert_eq!(sector_of(-1_000.0, 100.0), 15); // az 174.3° (west-ish)
        assert_eq!(sector_of(100.0, -1_000.0), 24); // az 275.7° (south-ish)
        assert_eq!(sector_of(0.0, 0.0), 0); // degenerate guard
    }
}
