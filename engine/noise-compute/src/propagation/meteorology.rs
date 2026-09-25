//! Long-term weather of a path (the W6 reader contract, `w2-method/CONTRACT-W6.md` with its
//! 2026-09-24 amendment): probability of favourable propagation per period and propagation
//! direction, and air absorption per period and band. Built-in defaults stand until
//! `meteorology.arrow` lands.

use super::air_absorption::{iso_9613_1_alpha_bands, AbsorptionClimate};
use crate::types::NUM_BANDS;

/// Direction sectors of the favourable-condition table: 22.5° each, sector s centred on the
/// propagation azimuth 22.5°·s clockwise from north (source→receiver).
pub const DIRECTION_SECTOR_COUNT: usize = 16;
/// Default probability of favourable conditions until W6 delivers climatology: the value the
/// engine has carried since 2026-07-28 (owner, one p for all periods).
pub const DEFAULT_FAVOURABLE_PROBABILITY: f64 = 0.5;
/// Default absorption climate until W6 delivers it: ISO 9613-1 at the CNOSSOS-EU §2.5.6 default
/// 15 °C / 70 % RH (W2 METHOD.md, BOUND.md), no hourly variance.
pub const DEFAULT_ABSORPTION_TEMPERATURE_C: f64 = 15.0;
pub const DEFAULT_ABSORPTION_RELATIVE_HUMIDITY_PCT: f64 = 70.0;

/// The weather every path of one receiver meets.
#[derive(Debug, Clone)]
pub struct Meteorology {
    /// Per period (day, evening, night) and direction sector.
    pub favourable_probability: [[f64; DIRECTION_SECTOR_COUNT]; 3],
    /// Per period and band.
    pub absorption: [[AbsorptionClimate; NUM_BANDS]; 3],
}

impl Meteorology {
    pub fn defaults() -> Self {
        let alpha = iso_9613_1_alpha_bands(DEFAULT_ABSORPTION_TEMPERATURE_C, DEFAULT_ABSORPTION_RELATIVE_HUMIDITY_PCT);
        Self {
            favourable_probability: [[DEFAULT_FAVOURABLE_PROBABILITY; DIRECTION_SECTOR_COUNT]; 3],
            absorption: [alpha.map(AbsorptionClimate::steady); 3],
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

    /// The smallest absorption any hour of any period reaches, per band (the relevance bound's
    /// α_min).
    pub fn minimum_absorption_db_per_km(&self) -> [f64; NUM_BANDS] {
        std::array::from_fn(|band| {
            self.absorption
                .iter()
                .map(|period| period[band].minimum_db_per_km)
                .fold(f64::INFINITY, f64::min)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
