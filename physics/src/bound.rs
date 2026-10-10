//! The upper bound of what one ground source can deliver at a receiver: free-field divergence at
//! the closest horizontal distance, the least air absorption, the largest mixed ground and
//! diffraction gain of CNOSSOS-EU and the receiver reflection. The popup skips sources only by
//! this bound, so it must stay an upper bound of `ray` and `line`.

use crate::bands::{A_WEIGHTING_DB, BANDS, PERIODS, energy};
use crate::line::{LINE_PERPENDICULAR_FLOOR_M, POINT_DIVERGENCE_LINEAR};

/// Largest favourable-state gain over free field (dB): the 2021/1226 (9)(h) below-plane corner,
/// the capped direct diffraction replaced by the image path's (>= 0 dB) while both sides sit at
/// the (2.5.20) floor of -9 dB, so 0 + 9 + 9 = 18 dB at most (17.60 dB found by search).
pub const FAVOURABLE_GAIN_BOUND_DB: f64 = 18.0;
/// Largest homogeneous-state gain (dB), held at the favourable bound. The below-plane corner at
/// the -3 dB floor gives 6.00 dB, but an elevated source gains more: a roof inside a short
/// source-side sub-path tilts its mean plane and throws S' past O, and (2.5.31) then gains
/// beyond the floor (the propagation audit found 11.4 dB in 300 k urban rays from sources 8-150 m
/// high, 17.8 dB on a synthetic path), so a 6 dB bound let the stop rule skip audible sources. At
/// the 32 benchmark and owner points the 18 dB bound moves the answers by at most 0.04 dB per
/// layer and the clicks no slower.
pub const HOMOGENEOUS_GAIN_BOUND_DB: f64 = FAVOURABLE_GAIN_BOUND_DB;
/// Point divergence 20 lg d + 11 (CNOSSOS-EU 2.5.12, 10 lg 4 pi printed as 11).
pub const POINT_DIVERGENCE_OFFSET_DB: f64 = 11.0;
/// The ray kernel floors point distances at 1 m.
const POINT_DISTANCE_FLOOR_M: f64 = 1.0;

/// Largest mixed gain at favourable probability `p`: mixing the two state maxima bounds the
/// mixed gain, and the mix grows with `p`.
pub fn mixed_gain_bound_db(p: f64) -> f64 {
    10.0 * (p * energy(FAVOURABLE_GAIN_BOUND_DB) + (1.0 - p) * energy(HOMOGENEOUS_GAIN_BOUND_DB))
        .log10()
}

/// How a source spreads, with what the bound needs of its geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spread {
    /// A straight piece of `length_m` (3D) carrying L_W' per metre.
    Line { length_m: f64 },
    /// A point carrying L_W.
    Point,
}

impl Spread {
    /// Divergence attenuation (dB) at horizontal distance `distance_m` to the source's nearest
    /// point. A line takes the smaller energy of its infinite line, `pi / (10^1.1 d)`, and of its
    /// whole power as a point, `L / (10^1.1 d^2)`: the point sum `theta / d_perp` equals the
    /// integral of ds / r^2 over the piece, and every point of it is at least `d` away.
    pub fn divergence_db(self, distance_m: f64) -> f64 {
        match self {
            Spread::Line { length_m } => {
                let d = distance_m.max(LINE_PERPENDICULAR_FLOOR_M);
                let infinite_line = std::f64::consts::PI / d;
                let whole_power = length_m / (d * d);
                -10.0 * (infinite_line.min(whole_power) / POINT_DIVERGENCE_LINEAR).log10()
            }
            Spread::Point => {
                20.0 * distance_m.max(POINT_DISTANCE_FLOOR_M).log10() + POINT_DIVERGENCE_OFFSET_DB
            }
        }
    }
}

/// A-weighted linear band energies of an emission (dB per band and period, `-inf` silent): what
/// both the bound and the full physics multiply by the transfer.
pub fn emission_energy(emission_db: &[[f64; BANDS]; PERIODS]) -> [[f64; BANDS]; PERIODS] {
    std::array::from_fn(|period| {
        std::array::from_fn(|band| energy(emission_db[period][band] + A_WEIGHTING_DB[band]))
    })
}

/// The receiver's share of the bound, the same for every source of one receiver: per period the
/// largest mixed gain at the largest favourable probability the rays can meet with the receiver
/// reflection, and the air absorption of the place per band (the rays' own).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceiverBound {
    pub gain: [f64; PERIODS],
    pub alpha_db_per_km: [f64; BANDS],
}

/// The [`ReceiverBound`] of a receiver.
pub fn receiver_bound(
    favourable_probability_max: [f64; PERIODS],
    reflection_db: f64,
    alpha_db_per_km: [f64; BANDS],
) -> ReceiverBound {
    ReceiverBound {
        gain: favourable_probability_max.map(|p| energy(mixed_gain_bound_db(p) + reflection_db)),
        alpha_db_per_km,
    }
}

/// Upper bound of the received A-weighted energy per period of a source with A-weighted band
/// energies `emission_energy` (per metre for lines) at horizontal distance `distance_m`.
pub fn received_energy_bound(
    emission_energy: &[[f64; BANDS]; PERIODS],
    spread: Spread,
    distance_m: f64,
    receiver: &ReceiverBound,
) -> [f64; PERIODS] {
    let divergence = energy(-spread.divergence_db(distance_m));
    let air: [f64; BANDS] =
        std::array::from_fn(|band| energy(-receiver.alpha_db_per_km[band] * distance_m / 1000.0));
    std::array::from_fn(|period| {
        let received: f64 = (0..BANDS)
            .map(|band| emission_energy[period][band] * air[band])
            .sum();
        received * divergence * receiver.gain[period]
    })
}

/// A ground source is loud at a receiver where its bound there reaches this Lden (dB): the painter
/// evaluates the loud ones exactly and estimates the quiet rest as the popup does, never dropping
/// it (the distant quiet hum is up to 40 % of a quiet forest's level). The map is drawn from 20 dB
/// (owner, 2026-10-10) (`evidence/2026-10-10/painter-plan`).
pub const REACH_EDGE_LDEN_DB: f64 = 20.0;

/// The farthest a source of A-weighted Lden power `lden_power` (per metre for a line) can be loud:
/// where its bound at the gain `gain_db`, without air absorption, falls to [`REACH_EDGE_LDEN_DB`]
/// (the inverse of [`Spread::divergence_db`], 0 for a source under the edge even at the distance
/// floor). A receiver's own bound decides; this only finds the sources to bound.
pub fn reach_m(lden_power: f64, spread: Spread, gain_db: f64) -> f64 {
    // The bound at distance d is lden_power x gain x the divergence's energy; for a point d^2 = k.
    let k = lden_power * energy(gain_db - REACH_EDGE_LDEN_DB) / POINT_DIVERGENCE_LINEAR;
    let (reach, floor) = match spread {
        Spread::Point => (k.sqrt(), POINT_DISTANCE_FLOOR_M),
        // Within L / pi of a piece its infinite line bounds it, beyond that its whole power.
        Spread::Line { length_m } => {
            let infinite_line = std::f64::consts::PI * k;
            let reach = if infinite_line <= length_m / std::f64::consts::PI {
                infinite_line
            } else {
                (length_m * k).sqrt()
            };
            (reach, LINE_PERPENDICULAR_FLOOR_M)
        }
    };
    if reach < floor { 0.0 } else { reach }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At its reach a source's bound is the edge, for a point, a short piece seen as its power
    /// and a long piece seen as its infinite line.
    #[test]
    fn the_reach_is_where_the_bound_falls_to_the_edge() {
        let gain_db = FAVOURABLE_GAIN_BOUND_DB + 3.0;
        for (power_db, spread) in [
            (70.0, Spread::Point),
            (60.0, Spread::Line { length_m: 40.0 }),
            (80.0, Spread::Line { length_m: 250.0 }),
            (60.0, Spread::Line { length_m: 250.0 }),
        ] {
            let reach = reach_m(energy(power_db), spread, gain_db);
            let bound_db = power_db + gain_db - spread.divergence_db(reach);
            assert!(
                (bound_db - REACH_EDGE_LDEN_DB).abs() < 1e-9,
                "{power_db} {spread:?}: {reach} m, {bound_db} dB"
            );
        }
        assert_eq!(reach_m(energy(-30.0), Spread::Point, gain_db), 0.0);
    }

    /// Both states now hold the same 18 dB, whatever the favourable probability.
    #[test]
    fn mixed_gain_spans_the_two_state_bounds() {
        for p in [0.0, 0.5, 1.0] {
            assert!((mixed_gain_bound_db(p) - 18.0).abs() < 1e-12, "{p}");
        }
    }

    #[test]
    fn a_long_line_bounds_as_its_infinite_line_and_a_far_piece_as_its_power() {
        // Infinite line: 10 lg(10^1.1 d / pi) = 10 lg d + 6.0285 dB.
        let near = Spread::Line { length_m: 250.0 }.divergence_db(10.0);
        assert!((near - (10.0 + 6.0285)).abs() < 1e-3, "{near}");
        // Far away the 40 m piece acts as a point of 40 m of power: 20 lg d + 11 - 10 lg 40.
        let far = Spread::Line { length_m: 40.0 }.divergence_db(5_000.0);
        assert!(
            (far - (20.0 * 5_000f64.log10() + 11.0 - 10.0 * 40f64.log10())).abs() < 0.01,
            "{far}"
        );
    }

    /// A night-only source keeps its night energy (a day-only gate would drop it).
    #[test]
    fn every_period_keeps_its_own_energy() {
        let mut emission = [[f64::NEG_INFINITY; BANDS]; PERIODS];
        emission[2] = [80.0; BANDS];
        let bound = received_energy_bound(
            &emission_energy(&emission),
            Spread::Point,
            100.0,
            &receiver_bound([0.5; PERIODS], 0.0, *crate::atmosphere::ALPHA_DB_PER_KM),
        );
        assert_eq!((bound[0], bound[1]), (0.0, 0.0));
        assert!(bound[2] > 0.0);
    }
}
