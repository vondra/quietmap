//! How loud the click sounds: Zwicker's loudness (ISO 532-1, sone) of the sound each period
//! exceeds 5 % of its time, N5, the percentile loudness of psychoacoustic and soundscape research
//! (ISO/TS 12913-3). Twice the sone sound twice as loud; a quiet room is about 1 sone, a busy
//! street 20 to 40. The spectrum is the received one: every ground layer's octave bands as its
//! evaluated pieces arrive (A-weighted, unweighted again, each octave shared by its thirds), the
//! flights' as the loudest flight's spectral classes at its slant distance through the place's
//! air, each scaled to its layer's energy; the sum is set to the period's L5 and its loudness
//! taken as steady.

use crate::selection::LayerSelection;
use physics::bands::{A_WEIGHTING_DB as OCTAVE_A_DB, BANDS, PERIODS, energy};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START, THIRD_OCTAVES, zwicker_loudness_sone};
use tiles::sources::Layer;

/// N5 per period (sone; 0 for silence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub n5_sone: [f64; PERIODS],
}

/// The heard flights: their A-weighted energy per period and their spectrum's shape (unweighted
/// third-octave levels from 50 Hz, dB up to a constant).
pub struct FlightSound {
    pub energy: [f64; PERIODS],
    pub spectrum_db: Option<[f64; CLASS_THIRDS]>,
}

/// N5 of the click from its layers, its flights and the levels exceeded 5 % of each period.
pub fn loudness(
    selections: &[LayerSelection],
    flights: &FlightSound,
    l5_db: [f64; PERIODS],
) -> Loudness {
    Loudness {
        n5_sone: std::array::from_fn(|period| {
            let thirds = unweighted_thirds(selections, flights, period);
            let a_weighted: f64 = thirds
                .iter()
                .zip(A_WEIGHTING_DB)
                .map(|(value, weight)| value * energy(weight))
                .sum();
            if a_weighted <= 0.0 || !l5_db[period].is_finite() {
                return 0.0;
            }
            let scale = energy(l5_db[period]) / a_weighted;
            let levels = thirds.map(|value| {
                if value > 0.0 {
                    10.0 * (value * scale).log10()
                } else {
                    f64::NEG_INFINITY
                }
            });
            zwicker_loudness_sone(&levels)
        }),
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
        if selection.layer == Layer::Aircraft {
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

    /// A road's N5 grows by about 2x per 10 dB of L5 and follows L5, not the mean; silence and a
    /// shape-less layer give 0; an aircraft's distant rumble sounds less loud than a road at the
    /// same L5 (its energy sits low, where the ear is deaf).
    #[test]
    fn loudness_follows_the_level_exceeded_five_percent_of_the_time() {
        let none = FlightSound {
            energy: [0.0; PERIODS],
            spectrum_db: None,
        };
        // A road's received A-weighted octave energies at 50 km/h (relative).
        let road_shape = [0.002, 0.03, 0.09, 0.28, 0.42, 0.18, 0.03, 0.004];
        let road = [layer(Layer::Road, 60.0, road_shape)];
        let at = |l5: f64| loudness(&road, &none, [l5; PERIODS]).n5_sone[0];
        let ratio = at(70.0) / at(60.0);
        assert!((1.8..2.3).contains(&ratio), "{ratio}");
        assert!((10.0..30.0).contains(&at(65.0)), "{}", at(65.0));
        assert_eq!(
            loudness(&road, &none, [f64::NEG_INFINITY; PERIODS]).n5_sone[0],
            0.0
        );
        assert_eq!(
            loudness(
                &[layer(Layer::Road, 60.0, [0.0; BANDS])],
                &none,
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
    }
}
