//! How loud the click sounds, on Zwicker's loudness scale (ISO 532-1, sone: twice the sone sound
//! twice as loud; a quiet room is about 1 sone, a busy street 20 to 40): N5, the loudness of the
//! sound exceeded 5 % of the time, the percentile loudness ISO 532-1 gives time-varying sound and
//! a soundscape indicator of ISO/TS 12913-3; per period, and for the whole day as Lden weighs its
//! periods: each period's spectrum at its L5, the evening's 5 dB and the night's 10 dB up, over
//! their hours. The spectrum is the received one: every ground layer's octave bands as its
//! evaluated pieces arrive (A-weighted, unweighted again, each octave shared by its thirds), the
//! flights' as the loudest flight's spectral classes at its slant distance through the place's
//! air, each scaled to its layer's energy; its loudness is taken as steady.

use crate::selection::LayerSelection;
use physics::bands::{
    A_WEIGHTING_DB as OCTAVE_A_DB, BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy,
};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START, THIRD_OCTAVES, zwicker_loudness_sone};
use tiles::sources::Layer;

/// N5 per period and for the whole day (sone; 0 for silence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub n5_sone: [f64; PERIODS],
    pub n5_den_sone: f64,
}

/// The heard flights: their A-weighted energy per period and their spectrum's shape (unweighted
/// third-octave levels from 50 Hz, dB up to a constant).
pub struct FlightSound {
    pub energy: [f64; PERIODS],
    pub spectrum_db: Option<[f64; CLASS_THIRDS]>,
}

/// Third-octave levels (dB) of linear energies.
fn levels_db(thirds: &[f64; THIRD_OCTAVES]) -> [f64; THIRD_OCTAVES] {
    thirds.map(|value| {
        if value > 0.0 {
            10.0 * value.log10()
        } else {
            f64::NEG_INFINITY
        }
    })
}

/// The click's loudness from its layers, its flights and the levels exceeded 5 % of each period.
pub fn loudness(
    selections: &[LayerSelection],
    flights: &FlightSound,
    l5_db: [f64; PERIODS],
) -> Loudness {
    // Each period's received spectrum set to its L5 (none in a silent period).
    let at_l5: [Option<[f64; THIRD_OCTAVES]>; PERIODS] = std::array::from_fn(|period| {
        let thirds = unweighted_thirds(selections, flights, period);
        let a_weighted: f64 = thirds
            .iter()
            .zip(A_WEIGHTING_DB)
            .map(|(value, weight)| value * energy(weight))
            .sum();
        (a_weighted > 0.0 && l5_db[period].is_finite()).then(|| {
            let scale = energy(l5_db[period]) / a_weighted;
            thirds.map(|value| value * scale)
        })
    });
    let n5_sone = at_l5.map(|thirds| thirds.map_or(0.0, |t| zwicker_loudness_sone(&levels_db(&t))));
    let mut day = [0.0; THIRD_OCTAVES];
    for (period, thirds) in at_l5.iter().enumerate() {
        let Some(thirds) = thirds else { continue };
        let weight = PERIOD_HOURS[period] / 24.0 * energy(PERIOD_PENALTY_DB[period]);
        for (total, value) in day.iter_mut().zip(thirds) {
            *total += weight * value;
        }
    }
    let n5_den_sone = if day.iter().any(|&value| value > 0.0) {
        zwicker_loudness_sone(&levels_db(&day))
    } else {
        0.0
    };
    Loudness {
        n5_sone,
        n5_den_sone,
    }
}

/// The click's unweighted third-octave energies (linear, 25 Hz to 12.5 kHz) of one period, at the
/// layers' answer energies.
fn unweighted_thirds(
    selections: &[LayerSelection],
    flights: &FlightSound,
    period: usize,
) -> [f64; THIRD_OCTAVES] {
    let mut thirds = [0.0; THIRD_OCTAVES];
    for selection in selections {
        let mut layer = selection.answer_energy()[period];
        // The flights leave the layer's shape only for their own spectrum (a helicopter has none).
        if selection.layer == Layer::Aircraft && flights.spectrum_db.is_some() {
            layer -= flights.energy[period];
        }
        let shape = &selection.spectrum[period];
        let sum: f64 = shape.iter().sum();
        if layer <= 0.0 || sum <= 0.0 {
            continue;
        }
        for band in 0..BANDS {
            let unweighted = layer * shape[band] / sum / energy(OCTAVE_A_DB[band]);
            for third in 0..3 {
                thirds[OCTAVE_START - 1 + 3 * band + third] += unweighted / 3.0;
            }
        }
    }
    if let (Some(spectrum), true) = (flights.spectrum_db, flights.energy[period] > 0.0) {
        let a_weighted: f64 = (0..CLASS_THIRDS)
            .map(|n| energy(spectrum[n] + A_WEIGHTING_DB[n + OCTAVE_START - 1]))
            .sum();
        for n in 0..CLASS_THIRDS {
            thirds[n + OCTAVE_START - 1] +=
                flights.energy[period] * energy(spectrum[n]) / a_weighted;
        }
    }
    thirds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(kind: Layer, energy_db: f64, shape: [f64; BANDS]) -> LayerSelection {
        let mut selection = LayerSelection::new(kind);
        selection.energy = [energy(energy_db); PERIODS];
        selection.spectrum = [shape; PERIODS];
        selection
    }

    const NONE: FlightSound = FlightSound {
        energy: [0.0; PERIODS],
        spectrum_db: None,
    };
    /// A road's received A-weighted octave energies at 50 km/h (relative).
    const ROAD_SHAPE: [f64; BANDS] = [0.002, 0.03, 0.09, 0.28, 0.42, 0.18, 0.03, 0.004];

    /// A road's N5 grows by about 2x per 10 dB of L5 and follows L5, not the mean; silence and a
    /// shape-less layer give 0; an aircraft's distant rumble sounds less loud than a road at the
    /// same L5 (its energy sits low, where the ear is deaf); flights without a spectral class keep
    /// their layer's shape.
    #[test]
    fn loudness_follows_the_level_exceeded_five_percent_of_the_time() {
        let road = [layer(Layer::Road, 60.0, ROAD_SHAPE)];
        let at = |l5: f64| loudness(&road, &NONE, [l5; PERIODS]).n5_sone[0];
        let ratio = at(70.0) / at(60.0);
        assert!((1.8..2.3).contains(&ratio), "{ratio}");
        assert!((10.0..30.0).contains(&at(65.0)), "{}", at(65.0));
        assert_eq!(
            loudness(&road, &NONE, [f64::NEG_INFINITY; PERIODS]).n5_sone[0],
            0.0
        );
        assert_eq!(
            loudness(
                &[layer(Layer::Road, 60.0, [0.0; BANDS])],
                &NONE,
                [65.0; PERIODS]
            )
            .n5_sone[0],
            0.0
        );
        let mut rumble = [0.0; CLASS_THIRDS];
        for (n, level) in rumble.iter_mut().enumerate() {
            *level = 80.0 - 4.0 * n as f64;
        }
        let aircraft = [layer(Layer::Aircraft, 60.0, [0.0; BANDS])];
        let flights = FlightSound {
            energy: [energy(60.0); PERIODS],
            spectrum_db: Some(rumble),
        };
        let heard = loudness(&aircraft, &flights, [65.0; PERIODS]).n5_sone[0];
        assert!(heard > 0.0 && heard < at(65.0), "{heard} {}", at(65.0));
        // A helipad: its loudest flight has no spectral class, so the layer keeps its own shape.
        let helipad = [layer(Layer::Aircraft, 60.0, ROAD_SHAPE)];
        let unclassed = FlightSound {
            energy: [energy(60.0); PERIODS],
            spectrum_db: None,
        };
        let with_flights = loudness(&helipad, &unclassed, [65.0; PERIODS]).n5_sone[0];
        assert!(with_flights > 0.0);
        assert_eq!(
            with_flights,
            loudness(&helipad, &NONE, [65.0; PERIODS]).n5_sone[0]
        );
    }

    /// The whole day's N5 weighs its periods as Lden does: equal L5 all day reads as the day's
    /// spectrum at Lden's 6.4 dB above the day; an evening 5 dB and a night 10 dB quieter than the
    /// day, just what Lden adds back, read as the day; quieter still, below it; a silent evening
    /// and night leave the day's share of 12 hours in 24.
    #[test]
    fn the_whole_day_weighs_evening_and_night_as_lden_does() {
        let road = [layer(Layer::Road, 60.0, ROAD_SHAPE)];
        let lden_offset = 10.0 * ((12.0 + 4.0 * energy(5.0) + 8.0 * energy(10.0)) / 24.0).log10();
        let day_at = |db: f64| loudness(&road, &NONE, [db; PERIODS]).n5_sone[0];
        let flat = loudness(&road, &NONE, [60.0; PERIODS]).n5_den_sone;
        let expected = day_at(60.0 + lden_offset);
        assert!((flat / expected - 1.0).abs() < 0.02, "{flat} vs {expected}");
        let balanced = loudness(&road, &NONE, [60.0, 55.0, 50.0]);
        assert!((balanced.n5_den_sone / balanced.n5_sone[0] - 1.0).abs() < 0.02);
        let quiet_night = loudness(&road, &NONE, [60.0, 50.0, 40.0]);
        assert!(quiet_night.n5_den_sone < quiet_night.n5_sone[0]);
        let day_only = loudness(&road, &NONE, [60.0, f64::NEG_INFINITY, f64::NEG_INFINITY]);
        let half_day = day_at(60.0 - 10.0 * 2f64.log10());
        assert!(
            (day_only.n5_den_sone / half_day - 1.0).abs() < 0.02,
            "{} vs {half_day}",
            day_only.n5_den_sone
        );
    }
}
