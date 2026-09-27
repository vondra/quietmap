//! The one relevance bound behind every road and rail reach and every point-source pair skip
//! (point reaches are still their layers' hand-set radii):
//! `B_k,i(d) = L_W,k,i − A_div,min(d) − α_min,i·d/1000 + G_max`, never below what the method can
//! deliver at horizontal distance `d`.

use crate::constants::A_WEIGHTING;
use crate::propagation::line_quadrature::POINT_DIVERGENCE_LINEAR;
use crate::propagation::point_sum::POINT_SOURCE_DIVERGENCE_OFFSET_DB;
use crate::types::NUM_BANDS;

/// How a source spreads: a line piece bounds by the infinite line through it at its closest
/// horizontal distance (every point of a straight piece is at least that far, so its point sum
/// is at most `π/(10^1.1·d)`), a point by `20·lg d + 11`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSpread {
    Line,
    Point,
}

impl SourceSpread {
    /// Geometric divergence `A_div` at `distance_m` floored at 1 m: an infinite line under the
    /// CNOSSOS point sum `10·lg(10^1.1·d/π)` (`line_quadrature`), a point `20·lg d + 11` (2.5.12).
    pub fn divergence_db(self, distance_m: f64) -> f64 {
        let d = distance_m.max(1.0);
        match self {
            SourceSpread::Line => 10.0 * (POINT_DIVERGENCE_LINEAR * d / std::f64::consts::PI).log10(),
            SourceSpread::Point => 20.0 * d.log10() + POINT_SOURCE_DIVERGENCE_OFFSET_DB,
        }
    }
}

/// The bound's two method-dependent terms.
#[derive(Debug, Clone, Copy)]
pub struct RelevanceBound {
    /// Smallest air absorption any path of the source can meet, per band [dB/km].
    pub alpha_min_db_per_km: [f64; NUM_BANDS],
    /// Largest mixed ground-and-diffraction gain over free field any path of the
    /// period can reach [dB], at the window's p_max of that period.
    pub gains_db: [f64; 3],
}

/// Largest favourable-state gain over free field [dB]: the (9)(h) below-plane corner, where
/// the capped Δdif(S,R) is replaced by the image path's (≥ 0 dB) while both sides sit at
/// the (2.5.20) floor (−9 dB each), so 0 + 9 + 9 = 18 dB at most (17.60 dB found: 6.2 km
/// hard path, receiver 7 m up in a dip below its side plane, image path grazing;
/// `below_plane_corners_stay_under_the_state_bounds`). The pre-slice-2 13.3 dB assumed a
/// blocked Δdif of at least 10·lg 3, which the replacement voids.
pub const FAVOURABLE_GAIN_BOUND_DB: f64 = 18.0;
/// Largest homogeneous-state gain [dB]: the same corner with the −3 dB homogeneous floor
/// per side (6.00 dB found).
pub const HOMOGENEOUS_GAIN_BOUND_DB: f64 = 6.0;
/// The reach edge: a row reaches as far as its bound's Lden stays above the 30 dB display floor
/// (owner decision via the orchestrator, 2026-09-24).
pub const REACH_EDGE_LDEN_DB: f64 = 30.0;
/// Longest ray the painter's 64-sample profile cadence holds (it first needs a 65th sample at
/// 11,872.35 m); every reach is capped so no ray outruns it.
pub const PROFILE_RAY_CEILING_M: f64 = 11_872.0;
/// Longest line piece the extract writes (every way is split at a hard 250 m).
pub const LINE_PIECE_MAXIMUM_LENGTH_M: f64 = 250.0;
/// Reach ceiling of a line piece's closest point: its farthest point stays within the profile.
pub const LINE_REACH_CEILING_M: f64 = PROFILE_RAY_CEILING_M - LINE_PIECE_MAXIMUM_LENGTH_M;
/// Far-field gate of the finite-piece cap, in piece lengths: beyond 20 lengths the piece
/// subtends under 3° (span ≤ L/d < 0.05 rad), so every quadrature bucket keeps its uniform
/// centre node — the wide-bucket path needs ≥ 3° per bucket — and the node weights partition
/// the subtended angle up to f64 rounding. Inside the gate the line bound stands alone.
pub const POINT_CAP_FAR_FIELD_LENGTHS: f64 = 20.0;
/// Length margin of the finite-piece cap: the f32 `length_m` rounding (≤ 2.6e-7 dB), the
/// extract's planimetric length versus the true geodesic (≤ 0.4% → 0.017 dB), and the
/// quadrature weight-partition dust (~1e-15) sum to under 0.02 dB; 1% overestimates the
/// piece power by 0.04 dB, covering all three with headroom. Sound direction: the margin
/// only ever keeps rows, never drops them.
pub const POINT_CAP_LENGTH_MARGIN: f64 = 1.01;

/// Largest mixed gain at favourable probability `p`: mixing the two state maxima
/// bounds the mixed gain, and the mix increases in `p`, so the window's p_max
/// covers every path of the period.
pub fn mixed_gain_bound_db(p: f64) -> f64 {
    10.0 * (p * 10f64.powf(FAVOURABLE_GAIN_BOUND_DB / 10.0)
        + (1.0 - p) * 10f64.powf(HOMOGENEOUS_GAIN_BOUND_DB / 10.0))
        .log10()
}

/// The bound of the surface propagation method under the given weather.
pub fn surface_relevance_bound(weather: &crate::propagation::meteorology::Meteorology) -> RelevanceBound {
    RelevanceBound {
        alpha_min_db_per_km: weather.bound_alpha_min_db_per_km,
        gains_db: weather.bound_probability_max.map(mixed_gain_bound_db),
    }
}

/// The bound for one row: gains mixed at the largest p over the row's azimuth span
/// (a point's single azimuth), never above the window maximum the extract-time
/// envelope was built at. A straight piece's node azimuths sweep monotonically
/// inside its endpoint span, so the span maximum covers every quadrature node.
pub fn bound_for_azimuth_span(
    weather: &crate::propagation::meteorology::Meteorology,
    span_rad: (f64, f64),
) -> RelevanceBound {
    RelevanceBound {
        alpha_min_db_per_km: weather.bound_alpha_min_db_per_km,
        gains_db: std::array::from_fn(|period| {
            mixed_gain_bound_db(weather.max_probability_over_span(period, span_rad.0, span_rad.1))
        }),
    }
}

/// The bound for one segment row from a receiver: the span of its endpoints' sight
/// lines, gains mixed at the largest p over that span. Points pass the same
/// coordinates twice (a point's single azimuth).
pub fn row_bound_for_segment(
    weather: &crate::propagation::meteorology::Meteorology,
    receiver_lat: f64,
    receiver_lon: f64,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
) -> RelevanceBound {
    let span = azimuth_span(receiver_lat, receiver_lon, start_lat, start_lon, end_lat, end_lon);
    bound_for_azimuth_span(weather, span)
}

/// Azimuth span `(lo, hi)` of the segment from a receiver, mathematical `atan2(north, east)`
/// radians in the local flat-earth frame (the evaluation's own convention): the endpoint
/// azimuths in sweep order, padded 1° each side against convention rounding. The sweep of
/// a straight segment seen from off the line is under π; a receiver on the line sees both
/// endpoints near ±π apart and the padded span still covers both.
pub fn azimuth_span(
    receiver_lat: f64,
    receiver_lon: f64,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
) -> (f64, f64) {
    use crate::propagation::geo::{m_per_deg_lon, wrapped_longitude_delta, M_PER_DEG_LAT};
    let mid = receiver_lat.to_radians();
    let azimuth = |lat: f64, lon: f64| {
        ((lat - receiver_lat) * M_PER_DEG_LAT)
            .atan2(wrapped_longitude_delta(receiver_lon, lon) * m_per_deg_lon(mid))
    };
    let pad = 1.0_f64.to_radians();
    let (a, b) = (azimuth(start_lat, start_lon), azimuth(end_lat, end_lon));
    let sweep = (b - a).rem_euclid(std::f64::consts::TAU);
    if sweep <= std::f64::consts::PI {
        (a - pad, a + sweep + pad)
    } else {
        (b - pad, b + (std::f64::consts::TAU - sweep) + pad)
    }
}

impl RelevanceBound {
    /// Upper bound of the received band levels of one period at horizontal distance `distance_m`.
    pub fn level_db(
        &self,
        emission_db: &[f64; NUM_BANDS],
        spread: SourceSpread,
        distance_m: f64,
        period: usize,
    ) -> [f64; NUM_BANDS] {
        let d = distance_m.max(1.0);
        let divergence = spread.divergence_db(d);
        std::array::from_fn(|band| {
            emission_db[band] - divergence - self.alpha_min_db_per_km[band] * d / 1000.0 + self.gains_db[period]
        })
    }

    /// True when no band of any period can reach 0 dB: the pair is skipped without effect on
    /// any output (#31: every period counts, a night-only source is not dropped). Line rows need
    /// no such test: inside their reach the bound's Lden exceeds 30 dB, which an inaudible pair
    /// (every band below 0 dB, so each period below 7 dB(A)) cannot reach.
    pub fn pair_is_inaudible(
        &self,
        period_emissions_db: &[[f64; NUM_BANDS]; 3],
        spread: SourceSpread,
        distance_m: f64,
    ) -> bool {
        period_emissions_db.iter().enumerate().all(|(period, emission)| {
            self.level_db(emission, spread, distance_m, period).iter().all(|&level| level < 0.0)
        })
    }

    /// Upper bound of the A-weighted Lden at `distance_m`.
    pub fn lden_db(&self, period_emissions_db: &[[f64; NUM_BANDS]; 3], spread: SourceSpread, distance_m: f64) -> f64 {
        let [day, evening, night] = [0, 1, 2].map(|period| {
            let levels = self.level_db(&period_emissions_db[period], spread, distance_m, period);
            let energy: f64 = (0..NUM_BANDS).map(|b| 10f64.powf((levels[b] + A_WEIGHTING[b]) / 10.0)).sum();
            assert!(energy.is_finite() && energy >= 0.0, "non-finite bound energy: {energy}");
            10.0 * energy.max(1e-30).log10()
        });
        crate::periods::compute_lden(day, evening, night)
    }

    /// True while the bound's Lden at `distance_m` exceeds the reach edge: the pair is within
    /// the row's reach (the popup's form of [`Self::reach_m`], without solving for it).
    pub fn within_reach(&self, period_emissions_db: &[[f64; NUM_BANDS]; 3], spread: SourceSpread, distance_m: f64) -> bool {
        self.lden_db(period_emissions_db, spread, distance_m) > REACH_EDGE_LDEN_DB
    }

    /// Reach test for one finite line piece of `length_m` (per-metre `L_W′` emissions, closest-
    /// point `distance_m`): the line bound AND the finite-piece cap. The infinite-line bound
    /// suits long pieces up close but wastes ~10·lg(d/L) dB on a short piece far away (a 40 m
    /// residential piece at 5 km: line bound ≈ 47 dB Lden, cap ≈ 21 dB); the cap drops exactly those
    /// rows, which the line bound alone keeps to the profile ceiling.
    ///
    /// Soundness: the kernel evaluates the piece as the incoherent CNOSSOS point sum
    /// `E = W′/(10^1.1·d⊥)·Σ w_j·T_j` (`line_quadrature`). Past the far-field gate every
    /// bucket is uniform, so `Σ w_j` is the subtended angle Φ up to f64 dust, and
    /// `dx/r² = dφ/d⊥` gives `E ≤ W′·L·G·A/(10^1.1·d²)`: every node sits at 3D distance
    /// ≥ the horizontal closest point `d`, its transfer `T_j` is under the same mixed gain
    /// `G` and min absorption `A` the line bound assumes (directivity ≤ 1, omnidirectional
    /// today and `0.01 + 0.99·sin²` in the planned track dipole), and the 1% length margin
    /// covers the planimetric length, the f32 rounding, and the weight dust. In dB that is
    /// the point spread at the piece's total power `L_W′ + 10·lg(L)`, evaluated by the same
    /// [`Self::within_reach`] — a second sound upper bound, so the row is dropped only when
    /// BOTH bounds agree it sits below the reach edge. Degenerate lengths (≤ 0, NaN) fail
    /// closed to the line bound, exactly the old behaviour.
    pub fn line_piece_within_reach(
        &self,
        period_emissions_db: &[[f64; NUM_BANDS]; 3],
        length_m: f32,
        distance_m: f64,
    ) -> bool {
        if !self.within_reach(period_emissions_db, SourceSpread::Line, distance_m) {
            return false;
        }
        // NaN-safe: a non-positive or NaN length fails closed to the line bound.
        if length_m.is_nan() || length_m <= 0.0 {
            return true;
        }
        let eff_len = f64::from(length_m) * POINT_CAP_LENGTH_MARGIN;
        if distance_m <= POINT_CAP_FAR_FIELD_LENGTHS * eff_len {
            return true;
        }
        let total_db = 10.0 * eff_len.log10();
        let point_emissions: [[f64; NUM_BANDS]; 3] =
            period_emissions_db.map(|bands| bands.map(|lw| lw + total_db));
        self.within_reach(&point_emissions, SourceSpread::Point, distance_m)
    }

    /// Smallest horizontal distance at which the bound's Lden falls to `edge_db`, capped at
    /// `ceiling_m`. The bound decreases monotonically with distance, so bisection in log distance
    /// finds it to well under a millimetre.
    pub fn reach_m(
        &self,
        period_emissions_db: &[[f64; NUM_BANDS]; 3],
        spread: SourceSpread,
        edge_db: f64,
        ceiling_m: f64,
    ) -> f64 {
        if self.lden_db(period_emissions_db, spread, ceiling_m) > edge_db {
            return ceiling_m;
        }
        let (mut lo, mut hi) = (1.0_f64, ceiling_m);
        if self.lden_db(period_emissions_db, spread, lo) <= edge_db {
            return lo;
        }
        for _ in 0..24 {
            let mid = (lo * hi).sqrt();
            if self.lden_db(period_emissions_db, spread, mid) > edge_db {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: RelevanceBound = RelevanceBound {
        alpha_min_db_per_km: [0.0; NUM_BANDS],
        gains_db: [3.0; 3],
    };

    /// A road carrying all its traffic at night is kept at a distance where
    /// the day-only gate it replaces dropped it (Lnight 53.8 dB there).
    #[test]
    fn a_night_only_road_is_not_skipped() {
        let silent = [f64::NEG_INFINITY; NUM_BANDS];
        let night = [80.0; NUM_BANDS];
        let periods = [silent, silent, night];
        assert!(BOUND.within_reach(&periods, SourceSpread::Line, 100.0));
        assert!(!BOUND.within_reach(&[silent; 3], SourceSpread::Line, 100.0));
    }

    /// Worst-case meteorology (favourable everywhere, no absorption): the bound at its
    /// loosest, where the finite-piece cap bites deepest.
    const LOUD_BOUND: RelevanceBound = RelevanceBound {
        alpha_min_db_per_km: [0.0; NUM_BANDS],
        gains_db: [FAVOURABLE_GAIN_BOUND_DB; 3],
    };

    fn flat_emissions(lw_per_m: f64) -> [[f64; NUM_BANDS]; 3] {
        [[lw_per_m; NUM_BANDS]; 3]
    }

    /// A 40 m residential piece (65 dB/m) at 5 km: the line bound keeps it (Lden ≈ 47 dB)
    /// but the finite-piece cap drops it (point Lden ≈ 21 dB) — the rows that used to
    /// fill the popup to the profile ceiling.
    #[test]
    fn the_cap_drops_a_short_quiet_piece_the_line_bound_keeps() {
        let periods = flat_emissions(65.0);
        assert!(LOUD_BOUND.within_reach(&periods, SourceSpread::Line, 5_000.0));
        assert!(!LOUD_BOUND.line_piece_within_reach(&periods, 40.0, 5_000.0));
    }

    /// A 250 m motorway piece (90 dB/m) at 9 km stays: past the far-field gate, both
    /// bounds agree it reaches (point Lden ≈ 49 dB).
    #[test]
    fn the_cap_keeps_a_long_loud_piece_far_away() {
        let periods = flat_emissions(90.0);
        assert!(LOUD_BOUND.line_piece_within_reach(&periods, 250.0, 9_000.0));
    }

    /// Inside the far-field gate the cap never bites: the piece test equals the line
    /// test at 500 m for the 40 m piece (gate at 808 m).
    #[test]
    fn inside_the_gate_the_piece_test_is_the_line_test() {
        let periods = flat_emissions(65.0);
        assert!(LOUD_BOUND.line_piece_within_reach(&periods, 40.0, 500.0));
        assert_eq!(
            LOUD_BOUND.line_piece_within_reach(&periods, 40.0, 500.0),
            LOUD_BOUND.within_reach(&periods, SourceSpread::Line, 500.0)
        );
    }

    /// Degenerate lengths fail closed to the line bound, keep and drop alike.
    #[test]
    fn degenerate_lengths_keep_the_old_answer() {
        let loud = flat_emissions(90.0);
        let quiet = flat_emissions(20.0);
        for length in [0.0f32, -40.0, f32::NAN, f32::INFINITY] {
            // +inf length: the gate `d ≤ 20·L` is true, so the line answer stands.
            assert_eq!(
                LOUD_BOUND.line_piece_within_reach(&loud, length, 5_000.0),
                LOUD_BOUND.within_reach(&loud, SourceSpread::Line, 5_000.0),
                "length {length}"
            );
            assert_eq!(
                LOUD_BOUND.line_piece_within_reach(&quiet, length, 5_000.0),
                LOUD_BOUND.within_reach(&quiet, SourceSpread::Line, 5_000.0),
                "length {length}"
            );
        }
    }

    /// Grid property: the cap only ever drops (piece ⟹ line), matches the line test
    /// inside the gate exactly, and stays monotone in distance (the kept set is a
    /// distance prefix — no gate-boundary flicker).
    #[test]
    fn the_cap_is_a_monotone_subset_of_the_line_bound() {
        let mut dist = 10.0f64;
        let mut dists = Vec::new();
        while dist <= 12_000.0 {
            dists.push(dist);
            dist *= 1.15;
        }
        for lw in [50.0, 65.0, 80.0, 95.0] {
            let periods = flat_emissions(lw);
            for length in [5.0f32, 40.0, 150.0, 250.0] {
                let mut seen_drop = false;
                for d in &dists {
                    let line = LOUD_BOUND.within_reach(&periods, SourceSpread::Line, *d);
                    let piece = LOUD_BOUND.line_piece_within_reach(&periods, length, *d);
                    assert!(!piece || line, "cap keeps what line drops: lw={lw} L={length} d={d}");
                    let eff = f64::from(length) * POINT_CAP_LENGTH_MARGIN;
                    if *d <= POINT_CAP_FAR_FIELD_LENGTHS * eff {
                        assert_eq!(piece, line, "gate mismatch: lw={lw} L={length} d={d}");
                    }
                    if !piece {
                        seen_drop = true;
                    } else {
                        assert!(!seen_drop, "non-monotone: lw={lw} L={length} d={d}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_mixed_gain_runs_from_the_homogeneous_to_the_favourable_maximum() {
        assert_eq!(mixed_gain_bound_db(0.0), HOMOGENEOUS_GAIN_BOUND_DB);
        assert_eq!(mixed_gain_bound_db(1.0), FAVOURABLE_GAIN_BOUND_DB);
        let half = mixed_gain_bound_db(0.5);
        assert!((half - 15.26).abs() < 0.01, "{half}");
        assert!(mixed_gain_bound_db(0.25) < half && half < mixed_gain_bound_db(0.75));
    }

    #[test]
    fn span_bounds_tighten_quiet_sectors_and_never_exceed_the_window() {
        use crate::propagation::meteorology::Meteorology;
        let mut weather = Meteorology::defaults();
        weather.favourable_probability = [[0.1; 16], [0.5; 16], [0.9; 16]];
        weather.bound_probability_max = [0.1, 0.5, 0.9];
        // Due east sees p 0.1/0.5/0.9; due west the same table (uniform rows).
        let east = bound_for_azimuth_span(&weather, (-0.05, 0.05));
        assert!((east.gains_db[0] - mixed_gain_bound_db(0.1)).abs() < 1e-9);
        assert!((east.gains_db[2] - mixed_gain_bound_db(0.9)).abs() < 1e-9);
        // One hot eastern sector in every period: a western span mixes low.
        let hot = std::array::from_fn(|s: usize| if s == 4 { 0.9 } else { 0.1 });
        weather.favourable_probability = [hot; 3];
        weather.bound_probability_max = [0.9; 3];
        let west = bound_for_azimuth_span(&weather, (2.0, 4.0));
        let full = surface_relevance_bound(&weather);
        assert!(west.gains_db[1] < full.gains_db[1] - 1.0);
        for period in 0..3 {
            assert!(west.gains_db[period] <= full.gains_db[period] + 1e-9);
        }
        // A far upwind row the window bound keeps, the span bound skips.
        let emission = [75.0; NUM_BANDS];
        let periods = [emission; 3];
        let dist = 11_000.0;
        assert!(full.within_reach(&periods, SourceSpread::Line, dist));
        assert!(!west.within_reach(&periods, SourceSpread::Line, dist));
    }

    #[test]
    fn azimuth_spans_follow_the_sight_lines() {
        // Due east / west / north of the receiver.
        let (lo, hi) = azimuth_span(50.0, 14.0, 50.0, 14.1, 50.0, 14.2);
        assert!(lo < 0.0 && hi > 0.0 && hi - lo < 0.1, "{lo} {hi}");
        let (lo, hi) = azimuth_span(50.0, 14.0, 50.1, 14.0, 50.2, 14.0);
        assert!((lo - std::f64::consts::FRAC_PI_2).abs() < 0.05, "{lo} {hi}");
        // A segment across the ±π branch cut spans narrowly, not the long way round.
        let (lo, hi) = azimuth_span(50.0, 14.0, 50.0, 13.9, 50.001, 13.9);
        assert!(hi - lo < 0.1, "{lo} {hi}");
    }

    #[test]
    fn the_line_bound_is_the_infinite_line_and_reach_inverts_it() {
        let emission = [70.0; NUM_BANDS];
        let level = BOUND.level_db(&emission, SourceSpread::Line, 100.0, 0)[0];
        assert!((level - (70.0 - 20.0 - 6.0285 + 3.0)).abs() < 1e-3, "{level}");
        let periods = [emission; 3];
        let reach = BOUND.reach_m(&periods, SourceSpread::Line, 30.0, 1e6);
        assert!((BOUND.lden_db(&periods, SourceSpread::Line, reach) - 30.0).abs() < 1e-4);
        assert_eq!(BOUND.reach_m(&periods, SourceSpread::Line, 30.0, 50.0), 50.0);
    }
}
