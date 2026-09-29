//! Long-term weather of a path: probability of favourable propagation per period and
//! propagation direction, and air absorption per period and band. Sampled from the receiver
//! square's `meteorology.bin` window; built-in defaults stand only where the square has no file.

use super::air_absorption::{iso_9613_1_alpha_bands, AbsorptionClimate};
use crate::types::NUM_BANDS;

/// Direction sectors of the favourable-condition table: 22.5° each, sector s centred on the
/// propagation azimuth 22.5°·s clockwise from north (source→receiver).
pub const DIRECTION_SECTOR_COUNT: usize = 16;
/// Default probability of favourable conditions where a square has no file: the value the
/// engine has carried since 2026-07-28 (owner, one p for all periods).
pub const DEFAULT_FAVOURABLE_PROBABILITY: f64 = 0.5;
/// Default absorption climate where a square has no file: ISO 9613-1 at the CNOSSOS-EU
/// §2.5.6 default 15 °C / 70 % RH, no hourly variance.
pub const DEFAULT_ABSORPTION_TEMPERATURE_C: f64 = 15.0;
pub const DEFAULT_ABSORPTION_RELATIVE_HUMIDITY_PCT: f64 = 70.0;

/// The weather every path of one receiver meets: the receiver's own sample (p per
/// period and direction sector, absorption per period and band) plus the receiver
/// square window's extremes behind the relevance bound.
#[derive(Debug, Clone)]
pub struct Meteorology {
    /// Per period (day, evening, night) and direction sector.
    pub favourable_probability: [[f64; DIRECTION_SECTOR_COUNT]; 3],
    /// Per period and band.
    pub absorption: [[AbsorptionClimate; NUM_BANDS]; 3],
    /// Largest stored p per period over the receiver square's window (every
    /// interpolation inside the window stays under it).
    pub bound_probability_max: [f64; 3],
    /// Smallest absorption slope per band over the window (the linear bound of
    /// the long-term A_atm up to the profile ceiling).
    pub bound_alpha_min_db_per_km: [f64; NUM_BANDS],
}

impl Meteorology {
    pub fn defaults() -> Self {
        let alpha = iso_9613_1_alpha_bands(DEFAULT_ABSORPTION_TEMPERATURE_C, DEFAULT_ABSORPTION_RELATIVE_HUMIDITY_PCT);
        Self {
            favourable_probability: [[DEFAULT_FAVOURABLE_PROBABILITY; DIRECTION_SECTOR_COUNT]; 3],
            absorption: [alpha.map(AbsorptionClimate::steady); 3],
            bound_probability_max: [DEFAULT_FAVOURABLE_PROBABILITY; 3],
            bound_alpha_min_db_per_km: alpha,
        }
    }

    /// A bound-only weather from window extremes: the painter's row envelope spans
    /// receivers across squares, so no single receiver sample covers it; the maxima do.
    pub fn for_bound(probability_max: [f64; 3], alpha_min_db_per_km: [f64; NUM_BANDS]) -> Self {
        Self {
            bound_probability_max: probability_max,
            bound_alpha_min_db_per_km: alpha_min_db_per_km,
            ..Self::defaults()
        }
    }

    /// p of `period` for sound travelling from source to receiver along `azimuth_rad`
    /// (`atan2(north, east)`), interpolated linearly between the two nearest sector centres.
    pub fn favourable_probability(&self, period: usize, azimuth_rad: f64) -> f64 {
        let bearing_deg = (90.0 - azimuth_rad.to_degrees()).rem_euclid(360.0);
        let position = bearing_deg / (360.0 / DIRECTION_SECTOR_COUNT as f64);
        let lower = position.floor() as usize % DIRECTION_SECTOR_COUNT;
        let upper = (lower + 1) % DIRECTION_SECTOR_COUNT;
        let fraction = position - position.floor();
        let row = &self.favourable_probability[period];
        row[lower] + fraction * (row[upper] - row[lower])
    }

    /// The largest p of `period` over the azimuth interval [`lo_rad`, `hi_rad`], exact:
    /// p is piecewise linear with breakpoints at the sector centres, so the maximum
    /// sits at an endpoint or an enclosed centre. Never below the evaluation's p at
    /// any covered node (gains mixed at this cover every node) and never above the
    /// window maximum (the extract-time envelope was built at the window maximum, so
    /// a popup reach beyond it would miss unloaded rows).
    pub fn max_probability_over_span(&self, period: usize, lo_rad: f64, hi_rad: f64) -> f64 {
        use std::f64::consts::TAU;
        let window_max = self.bound_probability_max[period];
        if (hi_rad - lo_rad).abs() >= TAU - 1e-9 {
            return window_max;
        }
        let span = (hi_rad - lo_rad).rem_euclid(TAU);
        let mut max = self
            .favourable_probability(period, lo_rad)
            .max(self.favourable_probability(period, hi_rad));
        // Sector s is centred on bearing 22.5°·s, azimuth π/2 − s·π/8.
        for s in 0..DIRECTION_SECTOR_COUNT {
            let centre = std::f64::consts::FRAC_PI_2 - s as f64 * TAU / DIRECTION_SECTOR_COUNT as f64;
            let shift = (centre - lo_rad).rem_euclid(TAU);
            if shift > 0.0 && shift < span {
                max = max.max(self.favourable_probability(period, centre));
            }
        }
        max.min(window_max)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_maximum_is_exact_and_clamped_to_the_window() {
        let mut weather = Meteorology::defaults();
        // One hot sector (due east, sector 4) at 0.9, the rest 0.1.
        weather.favourable_probability[0] = std::array::from_fn(|s| if s == 4 { 0.9 } else { 0.1 });
        weather.bound_probability_max[0] = 0.9;
        // A span covering due east sees the peak.
        let hit = weather.max_probability_over_span(0, -0.2, 0.2);
        assert!((hit - 0.9).abs() < 1e-12, "{hit}");
        // A span due west sees only 0.1 (centres excluded at the endpoints).
        let miss = weather.max_probability_over_span(0, std::f64::consts::PI - 0.1, std::f64::consts::PI + 0.1);
        assert!((miss - 0.1).abs() < 1e-12, "{miss}");
        // Dense sampling never exceeds the span maximum anywhere.
        for k in 0..720 {
            let az = k as f64 * std::f64::consts::TAU / 720.0;
            assert!(weather.favourable_probability(0, az) <= hit + 1e-12);
        }
        // A degenerate span is the point value; a full circle is the window max.
        assert!((weather.max_probability_over_span(0, 0.0, 0.0) - 0.9).abs() < 1e-12);
        assert!((weather.max_probability_over_span(0, 0.0, std::f64::consts::TAU) - 0.9).abs() < 1e-12);
        // The clamp holds even when the table overshoots the recorded window max.
        weather.bound_probability_max[0] = 0.5;
        assert!((weather.max_probability_over_span(0, -0.2, 0.2) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn favourable_probability_is_continuous_across_sectors_and_north() {
        let mut weather = Meteorology::defaults();
        weather.favourable_probability[2] = std::array::from_fn(|s| s as f64 / 16.0);
        // Due east (bearing 90°) is sector 4.
        assert!((weather.favourable_probability(2, 0.0) - 0.25).abs() < 1e-12);
        // Halfway between sectors 4 and 5.
        let between = weather.favourable_probability(2, (-11.25_f64).to_radians());
        assert!((between - 4.5 / 16.0).abs() < 1e-12);
        // Just west of north wraps from sector 15 to 0.
        let west_of_north = weather.favourable_probability(2, (90.0_f64 + 11.25).to_radians());
        assert!((west_of_north - 7.5 / 16.0).abs() < 1e-12);
    }
}
