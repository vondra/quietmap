//! NPD curves recalculated for the model's atmosphere (Doc 29 4th ed. Vol 2 Appendix D). The ANP
//! database normalises every NPD curve to the SAE AIR-1845 atmosphere (Table D-1), a notional
//! average of certification tests that absorbs 5.9 dB/km at 1 kHz; the model propagates every
//! ground source through CNOSSOS-EU's 15 C and 70 % (4.1 dB/km there), so the aircraft read up to
//! 2 dB quieter than the same atmosphere gives them. Per class and operation the anchor's spectral
//! class, at 305 m, is taken back to the source through AIR-1845 (Eq. D-1), out to each NPD distance
//! through both atmospheres (D-2, D-3), and the A-weighted difference (D-4) is added to its SEL and
//! LAmax curves. The model's absorption is ISO 9613-1 at the exact 1/3-octave centres (Doc 29
//! names SAE ARP-5534, which it matches within 2 % in the bands that carry an aircraft's level).

use super::npd::{METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT};
use super::spectra_generated::SPECTRA;
use crate::atmosphere::{
    DEFAULT_RELATIVE_HUMIDITY_PCT, DEFAULT_TEMPERATURE_C, REFERENCE_PRESSURE_KPA, alpha_db_per_km,
};

/// The 24 one-third-octave bands of the spectral classes, 50 Hz to 10 kHz.
pub const THIRD_OCTAVES: usize = 24;
/// Distance of the spectral classes' levels (m): 1,000 ft.
pub const SPECTRUM_DISTANCE_M: f64 = 1_000.0 * METRES_PER_FOOT;
/// Table D-1: the SAE AIR-1845 attenuation rates (dB per 100 m), 50 Hz to 10 kHz.
pub const AIR_1845_DB_PER_100M: [f64; THIRD_OCTAVES] = [
    0.033, 0.033, 0.033, 0.066, 0.066, 0.098, 0.131, 0.131, 0.197, 0.230, 0.295, 0.361, 0.459,
    0.590, 0.754, 0.983, 1.311, 1.705, 2.295, 3.115, 3.607, 5.246, 7.213, 9.836,
];
/// A-weighting of the 1/3-octave bands (IEC 61672), 50 Hz to 10 kHz.
pub const A_WEIGHTING_THIRD_OCTAVES_DB: [f64; THIRD_OCTAVES] = [
    -30.2, -26.2, -22.5, -19.1, -16.1, -13.4, -10.9, -8.6, -6.6, -4.8, -3.2, -1.9, -0.8, 0.0, 0.6,
    1.0, 1.2, 1.3, 1.2, 1.0, 0.5, -0.1, -1.1, -2.5,
];

/// A noise class's spectral classes (`spectra_generated`): unweighted 1/3-octave levels at 305 m
/// in the AIR-1845 atmosphere.
pub struct ClassSpectra {
    pub class_name: &'static str,
    pub anchor: &'static str,
    pub approach_class: u16,
    pub departure_class: u16,
    pub approach_db: [f64; THIRD_OCTAVES],
    pub departure_db: [f64; THIRD_OCTAVES],
}

/// The exact centre frequency (Hz) of 1/3-octave band `n` (0 = 50 Hz): 10^((n + 17) / 10).
pub fn third_octave_centre_hz(n: usize) -> f64 {
    10f64.powf((n as f64 + 17.0) / 10.0)
}

/// The model's attenuation rates (dB/m) per 1/3-octave band: CNOSSOS-EU's 15 C and 70 %.
pub fn model_rates_db_per_m() -> [f64; THIRD_OCTAVES] {
    std::array::from_fn(|n| {
        alpha_db_per_km(
            third_octave_centre_hz(n),
            DEFAULT_TEMPERATURE_C,
            DEFAULT_RELATIVE_HUMIDITY_PCT,
            REFERENCE_PRESSURE_KPA,
        ) / 1_000.0
    })
}

/// Eqs. D-1 to D-4: the increments (dB) at the ten NPD distances of a curve whose spectral class
/// is `spectrum_db`, going from AIR-1845 to the atmosphere of `rates_db_per_m`.
pub fn npd_increments_db(
    spectrum_db: &[f64; THIRD_OCTAVES],
    rates_db_per_m: &[f64; THIRD_OCTAVES],
) -> [f64; NPD_DISTANCES] {
    let source: [f64; THIRD_OCTAVES] = std::array::from_fn(|n| {
        spectrum_db[n] + AIR_1845_DB_PER_100M[n] / 100.0 * SPECTRUM_DISTANCE_M
    });
    let a_weighted = |distance_m: f64, rate: &dyn Fn(usize) -> f64| {
        let spreading = 20.0 * (distance_m / SPECTRUM_DISTANCE_M).log10();
        10.0 * (0..THIRD_OCTAVES)
            .map(|n| {
                10f64.powf(
                    (source[n] - spreading - rate(n) * distance_m
                        + A_WEIGHTING_THIRD_OCTAVES_DB[n])
                        / 10.0,
                )
            })
            .sum::<f64>()
            .log10()
    };
    std::array::from_fn(|k| {
        let distance_m = NPD_DISTANCES_FT[k] * METRES_PER_FOOT;
        a_weighted(distance_m, &|n| rates_db_per_m[n])
            - a_weighted(distance_m, &|n| AIR_1845_DB_PER_100M[n] / 100.0)
    })
}

/// The increments of a class's approach (`departure` false) or departure curves to the model's
/// atmosphere; zero for a class without spectral classes.
pub fn class_increments_db(class: usize, departure: bool) -> [f64; NPD_DISTANCES] {
    match &SPECTRA[class] {
        Some(spectra) => npd_increments_db(
            if departure {
                &spectra.departure_db
            } else {
                &spectra.approach_db
            },
            &model_rates_db_per_m(),
        ),
        None => [0.0; NPD_DISTANCES],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc29::profiles_generated::CLASS_NAMES;

    /// Appendix D's example (Tables D-2 to D-4): the V2527A's departure class 103 and approach
    /// class 205 taken to 10 C / 80 % with the SAE ARP-5534 rates of Table D-3 read the published
    /// increments within 0.15 dB (the table's levels are rounded to 0.1 dB).
    #[test]
    fn appendix_d_example_reproduces() {
        let arp_5534_10c_80: [f64; THIRD_OCTAVES] = [
            0.007, 0.011, 0.017, 0.026, 0.039, 0.056, 0.078, 0.104, 0.134, 0.166, 0.201, 0.241,
            0.292, 0.364, 0.471, 0.636, 0.893, 1.297, 1.931, 2.922, 4.461, 6.826, 10.398, 15.661,
        ]
        .map(|per_100m| per_100m / 100.0);
        let a320 = SPECTRA[1].as_ref().expect("the A320 class has spectra");
        assert_eq!((a320.departure_class, a320.approach_class), (103, 205));
        let departure = npd_increments_db(&a320.departure_db, &arp_5534_10c_80);
        let approach = npd_increments_db(&a320.approach_db, &arp_5534_10c_80);
        let published_departure = [0.1, 0.3, 0.4, 0.7, 1.2, 1.8, 2.1, 2.4, 2.9, 3.6];
        let published_approach = [0.0, 0.2, 0.3, 0.6, 1.1, 1.7, 2.2, 2.7, 3.2, 3.7];
        for k in 0..NPD_DISTANCES {
            assert!(
                (departure[k] - published_departure[k]).abs() < 0.15,
                "{k}: {departure:?}"
            );
            assert!(
                (approach[k] - published_approach[k]).abs() < 0.15,
                "{k}: {approach:?}"
            );
        }
    }

    /// The model's 15 C / 70 % absorbs less than AIR-1845 where an aircraft's level lies: the
    /// jets gain 0.4-0.8 dB at 1,000 ft and 1.2-2.4 dB at 10,000 ft; the table follows the classes.
    #[test]
    fn the_model_atmosphere_lifts_the_jets() {
        for (class, spectra) in SPECTRA.iter().enumerate() {
            let Some(spectra) = spectra else {
                assert_eq!(CLASS_NAMES[class], "HELICOPTER");
                continue;
            };
            assert_eq!(spectra.class_name, CLASS_NAMES[class]);
            if !spectra.class_name.starts_with("PROP") {
                for departure in [false, true] {
                    let increments = class_increments_db(class, departure);
                    assert!(
                        (0.3..1.0).contains(&increments[3]),
                        "{} {increments:?}",
                        spectra.class_name
                    );
                    assert!(
                        (1.0..2.6).contains(&increments[7]),
                        "{} {increments:?}",
                        spectra.class_name
                    );
                }
            }
        }
    }
}
