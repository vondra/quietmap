//! Octave bands, A-weighting, the END periods and Lden: the units every kernel shares.

/// Octave bands 63 Hz .. 8 kHz.
pub const BANDS: usize = 8;
/// Nominal mid-band frequencies (Hz), used for wavenumbers and wavelengths (CNOSSOS-EU).
pub const BAND_FREQUENCY_HZ: [f64; BANDS] =
    [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
/// Speed of sound (m/s) of CNOSSOS-EU 2.5.
pub const SPEED_OF_SOUND_M_PER_S: f64 = 340.0;
/// A-weighting per band (IEC 61672-1).
pub const A_WEIGHTING_DB: [f64; BANDS] = [-26.2, -16.1, -8.6, -3.2, 0.0, 1.2, 1.0, -1.1];

/// Day, evening, night.
pub const PERIODS: usize = 3;
/// END period lengths in hours (Directive 2002/49/EC): day 07-19, evening 19-23, night 23-07.
pub const PERIOD_HOURS: [f64; PERIODS] = [12.0, 4.0, 8.0];
/// Lden penalties of the evening and night periods.
pub const PERIOD_PENALTY_DB: [f64; PERIODS] = [0.0, 5.0, 10.0];

/// Linear energy of a level: 10^(L/10), as an exponential (twice as fast as a power, within
/// 5e-15 of it).
pub fn energy(level_db: f64) -> f64 {
    (level_db * (std::f64::consts::LN_10 / 10.0)).exp()
}

/// A pressure ratio of a level: 10^(L/20).
pub fn amplitude(level_db: f64) -> f64 {
    (level_db * (std::f64::consts::LN_10 / 20.0)).exp()
}

/// Level of a linear energy; silence is `-inf`. A NaN or negative energy is a bug and fails
/// closed instead of reading as quiet.
pub fn level_db(energy: f64) -> f64 {
    assert!(
        energy >= 0.0 && energy.is_finite(),
        "invalid energy {energy}"
    );
    if energy > 0.0 {
        10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Lden of per-period energies (A-weighted, linear).
pub fn lden_db(period_energy: [f64; PERIODS]) -> f64 {
    let weighted: f64 = (0..PERIODS)
        .map(|period| {
            PERIOD_HOURS[period] * period_energy[period] * energy(PERIOD_PENALTY_DB[period])
        })
        .sum();
    level_db(weighted / 24.0)
}

/// A-weighted energy of band levels.
pub fn a_weighted_energy(band_levels_db: &[f64; BANDS]) -> f64 {
    (0..BANDS)
        .map(|band| energy(band_levels_db[band] + A_WEIGHTING_DB[band]))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lden_of_60_55_50_is_60() {
        let lden = lden_db([energy(60.0), energy(55.0), energy(50.0)]);
        assert!((lden - 60.0).abs() < 0.01, "{lden}");
    }

    #[test]
    fn silence_is_minus_infinity_and_nan_fails_closed() {
        assert_eq!(level_db(0.0), f64::NEG_INFINITY);
        assert!(std::panic::catch_unwind(|| level_db(f64::NAN)).is_err());
        assert!((level_db(2.0 * energy(60.0)) - 63.0103).abs() < 1e-4);
    }
}
