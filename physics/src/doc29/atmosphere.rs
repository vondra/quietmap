//! NPD curves recalculated for the model's atmosphere (Doc 29 4th ed. Vol 2 Appendix D). The ANP
//! database normalises every NPD curve to the SAE AIR-1845 atmosphere (Table D-1), a notional
//! average of certification tests that absorbs 5.9 dB/km at 1 kHz; the model propagates every
//! ground source through CNOSSOS-EU's 15 C and 70 % (4.1 dB/km there), so the aircraft read up to
//! 2 dB quieter than the same atmosphere gives them. Per class and operation the anchor's spectral
//! class, at 305 m, is taken back to the source through AIR-1845 (Eq. D-1), out to each NPD distance
//! through both atmospheres (D-2, D-3), and the A-weighted difference (D-4) is added to its SEL and
//! LAmax curves. The model's absorption is ISO 9613-1 at the exact 1/3-octave centres (Doc 29
//! names SAE ARP-5534, which it matches within 2 % in the bands that carry an aircraft's level).

use super::npd::{METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, TAIL_ANCHOR_M};
use super::spectra_generated::SPECTRA;
use crate::atmosphere::{
    ALPHA_DB_PER_KM, DEFAULT_RELATIVE_HUMIDITY_PCT, DEFAULT_TEMPERATURE_C, REFERENCE_PRESSURE_KPA,
    alpha_db_per_km,
};
use crate::bands::BANDS;

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

/// ISO 9613-1 attenuation rates (dB/m) per 1/3-octave band at a temperature and humidity.
pub fn rates_db_per_m(temperature_c: f64, relative_humidity_pct: f64) -> [f64; THIRD_OCTAVES] {
    std::array::from_fn(|n| {
        alpha_db_per_km(
            third_octave_centre_hz(n),
            temperature_c,
            relative_humidity_pct,
            REFERENCE_PRESSURE_KPA,
        ) / 1_000.0
    })
}

/// The model's attenuation rates (dB/m) per 1/3-octave band: CNOSSOS-EU's 15 C and 70 %.
pub fn model_rates_db_per_m() -> [f64; THIRD_OCTAVES] {
    rates_db_per_m(DEFAULT_TEMPERATURE_C, DEFAULT_RELATIVE_HUMIDITY_PCT)
}

/// Doc 29 Eqs. 4-6 and 4-7: the acoustic impedance adjustment (dB) of the NPD levels at a
/// temperature and pressure, 10 lg(rho c / 409.81) with rho c = 416.86 delta / sqrt(theta).
pub fn impedance_adjustment_db(temperature_c: f64, pressure_kpa: f64) -> f64 {
    let delta = pressure_kpa / REFERENCE_PRESSURE_KPA;
    let theta = (temperature_c + 273.15) / (15.0 + 273.15);
    10.0 * (416.86 * delta / theta.sqrt() / 409.81).log10()
}

/// The A-weighted level (dB) at `distance_m` of a spectral class `spectrum_db` (at 305 m in the
/// AIR-1845 atmosphere, Eq. D-1 taking it back to the source) through the atmosphere of
/// `rates_db_per_m` (Eqs. D-2 to D-4).
fn a_weighted_db(
    spectrum_db: &[f64; THIRD_OCTAVES],
    rates_db_per_m: &[f64; THIRD_OCTAVES],
    distance_m: f64,
) -> f64 {
    let spreading = 20.0 * (distance_m / SPECTRUM_DISTANCE_M).log10();
    10.0 * (0..THIRD_OCTAVES)
        .map(|n| {
            let source = spectrum_db[n] + AIR_1845_DB_PER_100M[n] / 100.0 * SPECTRUM_DISTANCE_M;
            10f64.powf(
                (source - spreading - rates_db_per_m[n] * distance_m
                    + A_WEIGHTING_THIRD_OCTAVES_DB[n])
                    / 10.0,
            )
        })
        .sum::<f64>()
        .log10()
}

/// Eqs. D-1 to D-4: the increments (dB) at the ten NPD distances of a curve whose spectral class
/// is `spectrum_db`, going from AIR-1845 to the atmosphere of `rates_db_per_m`.
pub fn npd_increments_db(
    spectrum_db: &[f64; THIRD_OCTAVES],
    rates_db_per_m: &[f64; THIRD_OCTAVES],
) -> [f64; NPD_DISTANCES] {
    let air_1845: [f64; THIRD_OCTAVES] = AIR_1845_DB_PER_100M.map(|rate| rate / 100.0);
    std::array::from_fn(|k| {
        let distance_m = NPD_DISTANCES_FT[k] * METRES_PER_FOOT;
        a_weighted_db(spectrum_db, rates_db_per_m, distance_m)
            - a_weighted_db(spectrum_db, &air_1845, distance_m)
    })
}

/// The 1/3-octave absorption rates (dB/m) of a place whose octave-band absorption is
/// `octave_alpha_db_per_km` (the weather table's): the model's ISO 9613-1 rates times the place's
/// ratio to the model, the ratio's logarithm interpolated between the octave centres and continued
/// past the end ones (from -10 C / 60 % to 35 C / 10 % within 8 % of ISO 9613-1 at the exact
/// thirds; held per octave the thirds missed it by up to 40 %).
pub fn place_rates_db_per_m(octave_alpha_db_per_km: &[f64; BANDS]) -> [f64; THIRD_OCTAVES] {
    let model = model_rates_db_per_m();
    let ln_ratio: [f64; BANDS] = std::array::from_fn(|octave| {
        (octave_alpha_db_per_km[octave] / ALPHA_DB_PER_KM[octave]).ln()
    });
    std::array::from_fn(|n| {
        // The third's position among the octave centres: 63 Hz (third 1) is 0, 8 kHz (22) is 7.
        let position = (n as f64 - 1.0) / 3.0;
        let lower = (position.floor().max(0.0) as usize).min(BANDS - 2);
        let t = position - lower as f64;
        model[n] * (ln_ratio[lower] + t * (ln_ratio[lower + 1] - ln_ratio[lower])).exp()
    })
}

/// The distances at which the place's shift is stated: the ten NPD distances and the boxes' tail
/// anchor.
pub const SHIFT_DISTANCES: usize = NPD_DISTANCES + 1;

/// How far a place's yearly atmosphere moves every class's curves from the model's (Doc 29
/// Appendix D between the two atmospheres), per class and operation at the ten NPD distances and
/// the tail anchor (dB): the place's A-weighted level of the class's spectrum less the model's.
/// The helicopter class has no spectra and stays.
pub struct PlaceAtmosphere {
    shifts: Vec<[[f64; SHIFT_DISTANCES]; 2]>,
}

impl PlaceAtmosphere {
    pub fn new(octave_alpha_db_per_km: &[f64; BANDS]) -> Self {
        let (place, model) = (
            place_rates_db_per_m(octave_alpha_db_per_km),
            model_rates_db_per_m(),
        );
        let distance = |k: usize| {
            if k < NPD_DISTANCES {
                NPD_DISTANCES_FT[k] * METRES_PER_FOOT
            } else {
                TAIL_ANCHOR_M
            }
        };
        let shifts = SPECTRA
            .iter()
            .map(|spectra| {
                let Some(spectra) = spectra else {
                    return [[0.0; SHIFT_DISTANCES]; 2];
                };
                [&spectra.approach_db, &spectra.departure_db].map(|spectrum| {
                    std::array::from_fn(|k| {
                        a_weighted_db(spectrum, &place, distance(k))
                            - a_weighted_db(spectrum, &model, distance(k))
                    })
                })
            })
            .collect();
        PlaceAtmosphere { shifts }
    }

    /// The model atmosphere itself: no shift.
    pub fn model() -> Self {
        PlaceAtmosphere {
            shifts: vec![[[0.0; SHIFT_DISTANCES]; 2]; SPECTRA.len()],
        }
    }

    /// The shift of a class's departure or approach curves.
    pub fn shift_db(&self, class: usize, departure: bool) -> &[f64; SHIFT_DISTANCES] {
        &self.shifts[class][usize::from(departure)]
    }
}

/// The increments of a class's approach (`departure` false) or departure curves to the model's
/// atmosphere: the absorption's and the impedance adjustment Doc 29 4.2.1 applies to the ANP's
/// standard NPD levels (0.074 dB at 15 C and sea level). The helicopter class, calibrated to the
/// EASA certification levels rather than ANP curves, has neither.
pub fn class_increments_db(class: usize, departure: bool) -> [f64; NPD_DISTANCES] {
    let Some(spectra) = &SPECTRA[class] else {
        return [0.0; NPD_DISTANCES];
    };
    let impedance = impedance_adjustment_db(DEFAULT_TEMPERATURE_C, REFERENCE_PRESSURE_KPA);
    let spectrum = if departure {
        &spectra.departure_db
    } else {
        &spectra.approach_db
    };
    npd_increments_db(spectrum, &model_rates_db_per_m()).map(|increment| increment + impedance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc29::profiles_generated::{CLASS_NAMES, IS_JET};

    fn class_named(name: &str) -> usize {
        CLASS_NAMES
            .iter()
            .position(|&n| n == name)
            .expect("a class")
    }

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
        let a320 = SPECTRA[class_named("A320-232")]
            .as_ref()
            .expect("the A320 class has spectra");
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

    /// UBA Texte 11/2022 (the German BUF test tasks), Tables 28 and 2.5: the A320 departure class
    /// 103 recalculated to 10 C / 70 % reads 97.1, 90.5, 86.0, 81.2, 73.6, 65.3, 59.5, 53.2, 46.2
    /// and 38.9 dB(A) at the NPD distances (ARP-5534); ISO 9613-1 at the exact centres gives the
    /// same to 0.1 dB. The impedance adjustment there is 0.11 dB, at 15 C 0.074 dB (Doc 29 4.2.1).
    #[test]
    fn the_buf_test_tasks_recalculation_reproduces() {
        let a320 = SPECTRA[class_named("A320-232")]
            .as_ref()
            .expect("the A320 class has spectra");
        let increments = npd_increments_db(&a320.departure_db, &rates_db_per_m(10.0, 70.0));
        let reference = [97.0, 90.3, 85.6, 80.6, 72.5, 63.6, 57.4, 50.7, 43.3, 35.3];
        let published = [97.1, 90.5, 86.0, 81.2, 73.6, 65.3, 59.5, 53.2, 46.2, 38.9];
        for k in 0..NPD_DISTANCES {
            let recalculated = reference[k] + increments[k];
            assert!(
                (recalculated - published[k]).abs() < 0.11,
                "{k}: {recalculated} vs {}",
                published[k]
            );
        }
        assert!((impedance_adjustment_db(10.0, REFERENCE_PRESSURE_KPA) - 0.11).abs() < 0.005);
        assert!((impedance_adjustment_db(15.0, REFERENCE_PRESSURE_KPA) - 0.074).abs() < 0.001);
    }

    /// The place's rates from its octave absorption: the model's own atmosphere moves nothing, and
    /// at 30 C / 20 % (a desert) and -10 C / 60 % the interpolated thirds stay within 8 % of ISO
    /// 9613-1 at the exact centres in the bands that carry an aircraft's A-weighted level (200 Hz
    /// to 5 kHz).
    #[test]
    fn a_place_scales_the_model_rates_by_its_octaves() {
        let model = PlaceAtmosphere::new(&ALPHA_DB_PER_KM);
        for class in 0..SPECTRA.len() {
            for departure in [false, true] {
                assert!(
                    model
                        .shift_db(class, departure)
                        .iter()
                        .all(|s| s.abs() < 1e-12)
                );
            }
        }
        for (temperature_c, humidity_pct) in [(30.0, 20.0), (-10.0, 60.0)] {
            let octaves = crate::atmosphere::alpha_bands(temperature_c, humidity_pct);
            let scaled = place_rates_db_per_m(&octaves);
            let exact = rates_db_per_m(temperature_c, humidity_pct);
            for n in 6..21 {
                let ratio = scaled[n] / exact[n];
                assert!(
                    (0.92..1.08).contains(&ratio),
                    "{temperature_c} C band {n}: {ratio}"
                );
            }
        }
    }

    /// A desert's dry air takes 2-5 dB off an A320's departure curve 3-8 km away against the
    /// model's 15 C / 70 %; Prague's yearly air (the weather table's node) moves it under 1 dB.
    #[test]
    fn the_place_atmosphere_moves_far_aircraft() {
        let a320 = CLASS_NAMES.iter().position(|&n| n == "A320-232").unwrap();
        let desert = PlaceAtmosphere::new(&crate::atmosphere::alpha_bands(30.0, 20.0));
        let shift = desert.shift_db(a320, true);
        assert!(shift[5] < -1.0 && shift[5] > -4.0, "{shift:?}");
        assert!(shift[7] < -2.0 && shift[7] > -8.0, "{shift:?}");
        assert!(
            shift.windows(2).all(|pair| pair[1] <= pair[0] + 1e-9),
            "{shift:?}"
        );
        let prague = PlaceAtmosphere::new(&[0.12, 0.38, 0.99, 2.07, 4.21, 10.93, 34.83, 112.69]);
        let shift = prague.shift_db(a320, true);
        assert!(shift.iter().all(|s| s.abs() < 1.0), "{shift:?}");
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
            if IS_JET[class] {
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
