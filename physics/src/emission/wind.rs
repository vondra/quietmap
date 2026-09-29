//! Wind turbines (IEC 61400-11 sound powers): the published maximum by rated power, one annual
//! operating duty and the low-frequency-heavy spectrum of modern turbines, the same every period.

use super::spectrum::SoundPower;
use crate::bands::BANDS;

/// Energy mean of five published max-mode octave spectra, each relative to its own A-weighted
/// total and converted back to Z-weighted: Vestas V162-7.2, Siemens SWT-101-3.2, Vestas
/// V150-5.6 and V117-4.2 (Oliver Forest Wind Farm EIA, Technical Appendix 13.2 Table 3) and
/// Siemens Gamesa SG 6.0-155 AM0 (Ballinagree Wind Farm EIAR, Appendix 7.4 Table 7.4.2).
const TURBINE_SPECTRUM: [f64; BANDS] = [13.2, 11.1, 7.8, 4.4, 0.0, -3.9, -9.7, -19.4];

/// The annual operating level below the maximum (PLAN-z13 SIMPLIFY, one constant): the Dutch
/// statutory sum (Reken- en meetvoorschrift windturbines 2011) of dev4's generic sound power
/// curve over a Rayleigh wind distribution of 7.5 m/s mean is -2.14 dB; the nine published type
/// curves give -1.2 to -3.4 dB.
pub const WIND_DUTY_DB: f64 = -2.14;

/// Rated powers above this (kW) are tag errors and count as unknown (dev4 audit I-10b).
pub const TURBINE_MAXIMUM_PLAUSIBLE_POWER_KW: f64 = 8_000.0;

/// Published maximum L_WA (dB(A)) by rated power (dev4 audit I-10: a flat 104-106.5 band over
/// 1.8-6.6 MW): under 1 MW 98, 1-2 MW 104 (V90-2.0, E-82), 2-3 MW 105 (E-92), 3-5 MW 106
/// (V112-3.0, N149), 5 MW and above 106.5 (N163, E-160); unknown 105, the fleet median.
pub fn turbine_maximum_lw_dba(rated_power_kw: Option<f64>) -> f64 {
    let Some(kw) = rated_power_kw.filter(|kw| *kw > 0.0) else {
        return 105.0;
    };
    match kw as u32 {
        0 => 105.0,
        1..=999 => 98.0,
        1_000..=2_000 => 104.0,
        2_001..=2_999 => 105.0,
        3_000..=4_999 => 106.0,
        _ => 106.5,
    }
}

/// A turbine of `rated_power_kw` (`None` when unknown) at its annual operating level.
pub fn turbine_sound_power(rated_power_kw: Option<f64>) -> SoundPower {
    SoundPower {
        day_dba: turbine_maximum_lw_dba(rated_power_kw) + WIND_DUTY_DB,
        spectrum_db: TURBINE_SPECTRUM,
        evening_offset_db: 0.0,
        night_offset_db: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lookup_table_holds_the_published_band() {
        let table = |kw: f64| turbine_maximum_lw_dba(Some(kw));
        assert_eq!((table(500.0), table(999.0)), (98.0, 98.0));
        assert_eq!(
            (table(1_000.0), table(1_999.0), table(2_000.0)),
            (104.0, 104.0, 104.0)
        );
        assert_eq!((table(2_001.0), table(2_350.0)), (105.0, 105.0));
        assert_eq!((table(3_000.0), table(4_999.0)), (106.0, 106.0));
        assert_eq!((table(5_000.0), table(6_600.0)), (106.5, 106.5));
        assert_eq!(turbine_maximum_lw_dba(None), 105.0);
        assert_eq!(table(0.0), 105.0, "zero is unknown");
    }

    #[test]
    fn a_turbine_runs_its_duty_below_the_maximum_in_every_period() {
        let sound = turbine_sound_power(Some(3_000.0));
        assert!((sound.day_dba - (106.0 - 2.14)).abs() < 1e-12);
        assert_eq!((sound.evening_offset_db, sound.night_offset_db), (0.0, 0.0));
    }
}
