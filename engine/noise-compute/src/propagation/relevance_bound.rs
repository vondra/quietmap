//! The one relevance bound behind every source–receiver skip and every reach (W2 BOUND.md):
//! `B_k,i(d) = L_W,k,i − A_div,min(d) − α_min,i·d/1000 + G_max`, never below what the method can
//! deliver at horizontal distance `d`.

use crate::constants::A_WEIGHTING;
use crate::propagation::line_quadrature::POINT_DIVERGENCE_LINEAR;
use crate::types::NUM_BANDS;

/// How a source spreads: a line piece bounds by the infinite line through it at its closest
/// horizontal distance (every point of a straight piece is at least that far, so its point sum
/// is at most `π/(10^1.1·d)`), a point by `20·lg d + 11`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSpread {
    Line,
    Point,
}

/// The bound's two method-dependent terms.
#[derive(Debug, Clone, Copy)]
pub struct RelevanceBound {
    /// Smallest air absorption any path of the source can meet, per band [dB/km].
    pub alpha_min_db_per_km: [f64; NUM_BANDS],
    /// Largest ground-and-diffraction gain over free field any path can reach [dB].
    pub gain_db: f64,
}

/// Largest gain over free field the ground and diffraction terms of one state can give [dB],
/// flat or relief: a grazing hard crest takes the favourable floor of (2.5.20), -9 dB per
/// side, with a blocked Δdif(S,R) of at least 10·lg 3, so 2·9 − 10·lg 3 = 13.2 dB at most
/// (13.09 dB found: 11.8 km, crest 14.57 m at 236 m, source 0.05 m, receiver 1.5 m, G = 0;
/// the flat favourable 9.53 dB, homogeneous 3.71 dB). Rounded up; with p = 1 (the bound must
/// assume it, orchestrator 2026-09-24) the mixed gain is the favourable one.
/// `no_sampled_path_gains_more_than_the_bound` guards the search.
pub const SURFACE_RELEVANCE_GAIN_DB: f64 = 13.3;
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

/// The bound of the surface propagation method under the given weather.
pub fn surface_relevance_bound(weather: &crate::propagation::meteorology::Meteorology) -> RelevanceBound {
    RelevanceBound {
        alpha_min_db_per_km: weather.minimum_absorption_db_per_km(),
        gain_db: SURFACE_RELEVANCE_GAIN_DB,
    }
}

impl RelevanceBound {
    /// Upper bound of the received band levels at horizontal distance `distance_m`.
    pub fn level_db(&self, emission_db: &[f64; NUM_BANDS], spread: SourceSpread, distance_m: f64) -> [f64; NUM_BANDS] {
        let d = distance_m.max(1.0);
        let divergence = match spread {
            SourceSpread::Line => 10.0 * (POINT_DIVERGENCE_LINEAR * d / std::f64::consts::PI).log10(),
            SourceSpread::Point => 20.0 * d.log10() + 11.0,
        };
        std::array::from_fn(|band| {
            emission_db[band] - divergence - self.alpha_min_db_per_km[band] * d / 1000.0 + self.gain_db
        })
    }

    /// True when no band of any period can reach 0 dB: the pair is skipped without effect on
    /// any output (#31: every period counts, a night-only source is not dropped).
    pub fn pair_is_inaudible(
        &self,
        period_emissions_db: &[[f64; NUM_BANDS]; 3],
        spread: SourceSpread,
        distance_m: f64,
    ) -> bool {
        period_emissions_db.iter().all(|emission| {
            self.level_db(emission, spread, distance_m).iter().all(|&level| level < 0.0)
        })
    }

    /// Upper bound of the A-weighted Lden at `distance_m`.
    pub fn lden_db(&self, period_emissions_db: &[[f64; NUM_BANDS]; 3], spread: SourceSpread, distance_m: f64) -> f64 {
        let [day, evening, night] = period_emissions_db.map(|emission| {
            let levels = self.level_db(&emission, spread, distance_m);
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
        gain_db: 3.0,
    };

    /// T1 of BOUND.md: a road carrying all its traffic at night is kept at a distance where
    /// the day-only gate it replaces dropped it (Lnight 53.8 dB there).
    #[test]
    fn a_night_only_road_is_not_skipped() {
        let silent = [f64::NEG_INFINITY; NUM_BANDS];
        let night = [80.0; NUM_BANDS];
        let periods = [silent, silent, night];
        assert!(!BOUND.pair_is_inaudible(&periods, SourceSpread::Line, 100.0));
        assert!(BOUND.pair_is_inaudible(&[silent; 3], SourceSpread::Line, 100.0));
    }

    #[test]
    fn the_line_bound_is_the_infinite_line_and_reach_inverts_it() {
        let emission = [70.0; NUM_BANDS];
        let level = BOUND.level_db(&emission, SourceSpread::Line, 100.0)[0];
        assert!((level - (70.0 - 20.0 - 6.0285 + 3.0)).abs() < 1e-3, "{level}");
        let periods = [emission; 3];
        let reach = BOUND.reach_m(&periods, SourceSpread::Line, 30.0, 1e6);
        assert!((BOUND.lden_db(&periods, SourceSpread::Line, reach) - 30.0).abs() < 1e-4);
        assert_eq!(BOUND.reach_m(&periods, SourceSpread::Line, 30.0, 50.0), 50.0);
    }
}
