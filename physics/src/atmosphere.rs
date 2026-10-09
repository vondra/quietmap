//! Air absorption: ISO 9613-1:1993 attenuation coefficients at exact mid-band frequencies
//! (ISO/TR 17534-4 5.5): A_atm = alpha * slant distance. Propagation takes the place's yearly
//! absorption from the weather table (`crate::weather`); the CNOSSOS-EU default of 15 C and 70 %
//! relative humidity at 101.325 kPa is that table's node without data and the reference its
//! third-octave rates are scaled from.

use crate::bands::BANDS;
use std::sync::LazyLock;

/// ISO/TR 17534-4 5.5 reference pressure (kPa).
pub const REFERENCE_PRESSURE_KPA: f64 = 101.325;
/// The CNOSSOS-EU 2.5.6 default atmosphere.
pub const DEFAULT_TEMPERATURE_C: f64 = 15.0;
pub const DEFAULT_RELATIVE_HUMIDITY_PCT: f64 = 70.0;

/// Exact mid-band frequency of octave band `band` (63 Hz .. 8 kHz): 1000 * 10^(0.3 (k - 4)).
pub fn exact_mid_band_frequency_hz(band: usize) -> f64 {
    1000.0 * 10f64.powf(0.3 * (band as f64 - 4.0))
}

/// ISO 9613-1:1993 pure-tone attenuation coefficient (dB/km), equations 3-5 with Annex B humidity.
pub fn alpha_db_per_km(
    frequency_hz: f64,
    temperature_c: f64,
    relative_humidity_pct: f64,
    pressure_kpa: f64,
) -> f64 {
    let temperature = temperature_c + 273.15;
    let (reference_temperature, triple_point) = (293.15, 273.16);
    let pressure_ratio = pressure_kpa / REFERENCE_PRESSURE_KPA;
    let saturation_exponent = -6.8346 * (triple_point / temperature).powf(1.261) + 4.6151;
    let humidity = relative_humidity_pct * 10f64.powf(saturation_exponent) / pressure_ratio;
    let oxygen =
        pressure_ratio * (24.0 + 4.04e4 * humidity * (0.02 + humidity) / (0.391 + humidity));
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

/// Alpha of every band at one temperature and humidity, reference pressure.
pub fn alpha_bands(temperature_c: f64, relative_humidity_pct: f64) -> [f64; BANDS] {
    std::array::from_fn(|band| {
        alpha_db_per_km(
            exact_mid_band_frequency_hz(band),
            temperature_c,
            relative_humidity_pct,
            REFERENCE_PRESSURE_KPA,
        )
    })
}

/// Alpha of the CNOSSOS-EU default: 0.10, 0.38, 1.13, 2.36, 4.08, 8.75, 26.39, 93.71 dB/km.
pub static ALPHA_DB_PER_KM: LazyLock<[f64; BANDS]> =
    LazyLock::new(|| alpha_bands(DEFAULT_TEMPERATURE_C, DEFAULT_RELATIVE_HUMIDITY_PCT));

#[cfg(test)]
mod tests {
    use super::*;

    /// ISO/TR 17534-4 publishes alpha at 10 C / 70 %: 0.12 .. 116.88 dB/km.
    #[test]
    fn reproduces_the_iso_tr_coefficients_at_ten_degrees() {
        let expected = [0.12, 0.41, 1.04, 1.93, 3.66, 9.66, 32.77, 116.88];
        let alpha = alpha_bands(10.0, 70.0);
        for band in 0..BANDS {
            assert!(
                (alpha[band] - expected[band]).abs() < 0.006,
                "band {band}: {}",
                alpha[band]
            );
        }
    }

    #[test]
    fn propagation_uses_fifteen_degrees_seventy_percent() {
        let expected = [0.10, 0.38, 1.13, 2.36, 4.08, 8.75, 26.39, 93.71];
        for band in 0..BANDS {
            assert!(
                (ALPHA_DB_PER_KM[band] - expected[band]).abs() < 0.005,
                "band {band}"
            );
        }
    }
}
