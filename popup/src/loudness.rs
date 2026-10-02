//! How loud the click sounds, on Zwicker's loudness scale (ISO 532-1, sone: twice the sone sound
//! twice as loud; a quiet room is about 1 sone, a busy street 20 to 40). N5 per period: the
//! loudness of the sound each period exceeds 5 % of its time, the percentile loudness of
//! psychoacoustic and soundscape research (ISO/TS 12913-3). The rated loudness: one number for the
//! whole year, the loudness of the day-evening-night energy spectrum weighted as Lden weighs its
//! periods (overall judgements of long sounds follow their energy, Kuwano & Namba 1978,
//! Schlittenlacher et al. 2017), each layer first adjusted by how people judge it at equal level
//! (aircraft, steady road traffic). The spectrum is the received one: every ground layer's octave
//! bands as its evaluated pieces arrive (A-weighted, unweighted again, each octave shared by its
//! thirds), the flights' as the loudest flight's spectral classes at its slant distance through
//! the place's air, each scaled to its layer's energy; N5's sum is set to the period's L5 and its
//! loudness taken as steady.

use crate::selection::LayerSelection;
use physics::bands::{
    A_WEIGHTING_DB as OCTAVE_A_DB, BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy,
};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START, THIRD_OCTAVES, zwicker_loudness_sone};
use tiles::sources::Layer;

/// Aircraft are judged more annoying than road traffic at equal level: ISO 1996-1:2016 Annex A
/// +3 to +6 dB; the WHO 2018 review's curves +5.5 to +14 dB and SiRENE's (Brink et al. 2019) +7
/// to +12 dB over road Lden 50-70.
const AIRCRAFT_ADJUSTMENT_DB: f64 = 6.0;
/// Steady road traffic is judged more annoying than intermittent at equal level: by more than 6 dB
/// from an intermittency ratio of 10 % to 90 % (SiRENE, Brink et al. 2019); motorway residents are
/// as annoyed as urban-road residents at 6-13 dB lower Lden (Danish Road Directorate 2016, report
/// 565). The adjustment runs from this at a ratio of 10 % down to 0 at 90 %.
const STEADY_ROAD_ADJUSTMENT_DB: f64 = 6.0;

/// Per period the loudness of the sound exceeded 5 % (N5) and 50 % (N50, the typical sound) of
/// the time, and the rated loudness of the year (sone; 0 for silence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub n5_sone: [f64; PERIODS],
    pub n50_sone: [f64; PERIODS],
    pub rated_sone: f64,
}

/// The heard flights: their A-weighted energy per period and their spectrum's shape (unweighted
/// third-octave levels from 50 Hz, dB up to a constant).
pub struct FlightSound {
    pub energy: [f64; PERIODS],
    pub spectrum_db: Option<[f64; CLASS_THIRDS]>,
}

/// The adjustment of road traffic (dB) for its intermittency ratio (0-1; NaN without road).
fn steady_road_adjustment_db(intermittency: f64) -> f64 {
    if intermittency.is_nan() {
        return 0.0;
    }
    (STEADY_ROAD_ADJUSTMENT_DB * (0.9 - intermittency) / 0.8).clamp(0.0, STEADY_ROAD_ADJUSTMENT_DB)
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

/// The click's loudness from its layers, its flights, the levels exceeded 5 % and 50 % of each
/// period and road traffic's intermittency ratio per period.
pub fn loudness(
    selections: &[LayerSelection],
    flights: &FlightSound,
    (l5_db, l50_db): ([f64; PERIODS], [f64; PERIODS]),
    road_intermittency: [f64; PERIODS],
) -> Loudness {
    // The period's spectrum set to a level the period exceeds part of its time.
    let at_level = |level_db: [f64; PERIODS]| -> [f64; PERIODS] {
        std::array::from_fn(|period| {
            let thirds = unweighted_thirds(selections, flights, period, &|_| 1.0);
            let a_weighted: f64 = thirds
                .iter()
                .zip(A_WEIGHTING_DB)
                .map(|(value, weight)| value * energy(weight))
                .sum();
            if a_weighted <= 0.0 || !level_db[period].is_finite() {
                return 0.0;
            }
            let scale = energy(level_db[period]) / a_weighted;
            zwicker_loudness_sone(&levels_db(&thirds.map(|value| value * scale)))
        })
    };
    let (n5_sone, n50_sone) = (at_level(l5_db), at_level(l50_db));
    let mut year = [0.0; THIRD_OCTAVES];
    for period in 0..PERIODS {
        let road = energy(steady_road_adjustment_db(road_intermittency[period]));
        let gain = |layer: Layer| match layer {
            Layer::Aircraft => energy(AIRCRAFT_ADJUSTMENT_DB),
            Layer::Road => road,
            _ => 1.0,
        };
        let weight = PERIOD_HOURS[period] / 24.0 * energy(PERIOD_PENALTY_DB[period]);
        for (total, value) in year
            .iter_mut()
            .zip(unweighted_thirds(selections, flights, period, &gain))
        {
            *total += weight * value;
        }
    }
    let rated_sone = if year.iter().any(|&value| value > 0.0) {
        zwicker_loudness_sone(&levels_db(&year))
    } else {
        0.0
    };
    Loudness {
        n5_sone,
        n50_sone,
        rated_sone,
    }
}

/// The click's unweighted third-octave energies (linear, 25 Hz to 12.5 kHz) of one period, at the
/// layers' answer energies, each layer's times its `gain` (the flights' at the aircraft layer's).
fn unweighted_thirds(
    selections: &[LayerSelection],
    flights: &FlightSound,
    period: usize,
    gain: &dyn Fn(Layer) -> f64,
) -> [f64; THIRD_OCTAVES] {
    let mut thirds = [0.0; THIRD_OCTAVES];
    for selection in selections {
        let mut layer = selection.answer_energy()[period] * gain(selection.layer);
        if selection.layer == Layer::Aircraft {
            layer -= flights.energy[period] * gain(Layer::Aircraft);
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
                flights.energy[period] * gain(Layer::Aircraft) * energy(spectrum[n]) / a_weighted;
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
        let at = |l5: f64| {
            loudness(
                &road,
                &none,
                ([l5; PERIODS], [l5 - 5.0; PERIODS]),
                [0.5; PERIODS],
            )
            .n5_sone[0]
        };
        let ratio = at(70.0) / at(60.0);
        assert!((1.8..2.3).contains(&ratio), "{ratio}");
        assert!((10.0..30.0).contains(&at(65.0)), "{}", at(65.0));
        assert_eq!(
            loudness(
                &road,
                &none,
                ([f64::NEG_INFINITY; PERIODS], [f64::NEG_INFINITY; PERIODS]),
                [0.5; PERIODS]
            )
            .n5_sone[0],
            0.0
        );
        assert_eq!(
            loudness(
                &[layer(Layer::Road, 60.0, [0.0; BANDS])],
                &none,
                ([65.0; PERIODS], [60.0; PERIODS]),
                [0.5; PERIODS]
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
        let heard = loudness(
            &aircraft,
            &flights,
            ([65.0; PERIODS], [60.0; PERIODS]),
            [f64::NAN; PERIODS],
        )
        .n5_sone[0];
        assert!(heard > 0.0 && heard < at(65.0), "{heard} {}", at(65.0));
    }

    /// The rated loudness doubles with 10 dB more energy in every period; a steady road (a
    /// motorway's hum) rates as 6 dB more than an intermittent one of equal energy, about 1.5
    /// times as loud; a quiet night counts like Lden's.
    #[test]
    fn rated_loudness_doubles_per_ten_decibels_and_weighs_a_steady_road_more() {
        let none = FlightSound {
            energy: [0.0; PERIODS],
            spectrum_db: None,
        };
        let road_shape = [0.002, 0.03, 0.09, 0.28, 0.42, 0.18, 0.03, 0.004];
        let rated = |db: f64, intermittency: f64| {
            loudness(
                &[layer(Layer::Road, db, road_shape)],
                &none,
                ([db + 3.0; PERIODS], [db - 2.0; PERIODS]),
                [intermittency; PERIODS],
            )
            .rated_sone
        };
        let doubling = rated(65.0, 0.9) / rated(55.0, 0.9);
        assert!((1.8..2.3).contains(&doubling), "{doubling}");
        let steady = rated(55.0, 0.1) / rated(55.0, 0.9);
        assert!((1.35..1.7).contains(&steady), "{steady}");
        assert_eq!(rated(55.0, 0.5), rated(55.0, 0.5));
        assert_eq!(
            loudness(
                &[],
                &none,
                ([f64::NEG_INFINITY; PERIODS], [f64::NEG_INFINITY; PERIODS]),
                [f64::NAN; PERIODS]
            )
            .rated_sone,
            0.0
        );
    }
}
