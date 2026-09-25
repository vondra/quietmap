//! Atmospheric absorption: ISO 9613-1:1993 attenuation coefficients at exact mid-band
//! frequencies (ISO/TR 17534-4 §5.5), and the long-term A_atm of one period from the mean and
//! variance of its hourly coefficient (the W6 reader contract, orchestrator amendment
//! 2026-09-24).

use crate::types::NUM_BANDS;

/// ISO/TR 17534-4 §5.5 reference pressure [kPa].
pub const REFERENCE_PRESSURE_KPA: f64 = 101.325;

/// Exact mid-band frequency of octave band `band` (63 Hz … 8 kHz): `1000·10^(3k/10)`.
pub fn exact_mid_band_frequency_hz(band: usize) -> f64 {
    1000.0 * 10f64.powf(0.3 * (band as f64 - 4.0))
}

/// ISO 9613-1:1993 pure-tone attenuation coefficient α [dB/km] (eqs. 3–5, Annex B humidity).
pub fn iso_9613_1_alpha_db_per_km(frequency_hz: f64, temperature_c: f64, relative_humidity_pct: f64, pressure_kpa: f64) -> f64 {
    let temperature = temperature_c + 273.15;
    let (reference_temperature, triple_point) = (293.15, 273.16);
    let pressure_ratio = pressure_kpa / REFERENCE_PRESSURE_KPA;
    let saturation_exponent = -6.8346 * (triple_point / temperature).powf(1.261) + 4.6151;
    let humidity = relative_humidity_pct * 10f64.powf(saturation_exponent) / pressure_ratio;
    let oxygen = pressure_ratio * (24.0 + 4.04e4 * humidity * (0.02 + humidity) / (0.391 + humidity));
    let t_ratio = temperature / reference_temperature;
    let nitrogen = pressure_ratio
        * t_ratio.powf(-0.5)
        * (9.0 + 280.0 * humidity * (-4.170 * (t_ratio.powf(-1.0 / 3.0) - 1.0)).exp());
    let f2 = frequency_hz * frequency_hz;
    8.686
        * f2
        * (1.84e-11 / pressure_ratio * t_ratio.sqrt()
            + t_ratio.powf(-2.5)
                * (0.01275 * (-2239.1 / temperature).exp() / (oxygen + f2 / oxygen)
                    + 0.1068 * (-3352.0 / temperature).exp() / (nitrogen + f2 / nitrogen)))
        * 1000.0
}

/// α of every band at one temperature and humidity, reference pressure.
pub fn iso_9613_1_alpha_bands(temperature_c: f64, relative_humidity_pct: f64) -> [f64; NUM_BANDS] {
    std::array::from_fn(|band| {
        iso_9613_1_alpha_db_per_km(exact_mid_band_frequency_hz(band), temperature_c, relative_humidity_pct, REFERENCE_PRESSURE_KPA)
    })
}

/// The hourly attenuation coefficient of one period and band, summarised: mean, variance and the
/// smallest hour (which bounds the long-term transmission from above).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbsorptionClimate {
    pub mean_db_per_km: f64,
    pub variance_db2_per_km2: f64,
    pub minimum_db_per_km: f64,
}

impl AbsorptionClimate {
    /// A constant coefficient (no hour-to-hour variation).
    pub const fn steady(alpha_db_per_km: f64) -> Self {
        Self {
            mean_db_per_km: alpha_db_per_km,
            variance_db2_per_km2: 0.0,
            minimum_db_per_km: alpha_db_per_km,
        }
    }

    /// Long-term `A_atm = −10·lg E[10^(−α·d/10)]` over a slant distance, α normal with the stored
    /// mean and variance (second cumulant), never less than the smallest hour gives.
    pub fn attenuation_db(&self, slant_distance_m: f64) -> f64 {
        let km = slant_distance_m / 1000.0;
        let cumulant = self.mean_db_per_km * km
            - std::f64::consts::LN_10 / 20.0 * self.variance_db2_per_km2 * km * km;
        cumulant.max(self.minimum_db_per_km * km)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ISO/TR 17534-4 common input (10 °C, 70 %) gives the reference's coefficients.
    #[test]
    fn iso_9613_1_reproduces_the_iso_tr_17534_4_coefficients() {
        let expected = [0.12, 0.41, 1.04, 1.93, 3.66, 9.66, 32.77, 116.88];
        let got = iso_9613_1_alpha_bands(10.0, 70.0);
        for band in 0..NUM_BANDS {
            assert!((got[band] - expected[band]).abs() < 0.006, "band {band}: {}", got[band]);
        }
    }

    #[test]
    fn variance_lowers_the_long_term_attenuation_but_never_below_the_quietest_hour() {
        let climate = AbsorptionClimate {
            mean_db_per_km: 10.0,
            variance_db2_per_km2: 4.0,
            minimum_db_per_km: 6.0,
        };
        assert_eq!(AbsorptionClimate::steady(10.0).attenuation_db(1000.0), 10.0);
        let one_km = climate.attenuation_db(1000.0);
        assert!((one_km - (10.0 - 0.4605 * 1.0)).abs() < 1e-3, "{one_km}");
        assert_eq!(climate.attenuation_db(10_000.0), 60.0);
    }
}
