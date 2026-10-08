//! How loud the click sounds, on Zwicker's loudness scale (ISO 532-1, sone: twice the sone sound
//! twice as loud; a quiet room is about 1 sone, a busy street 20 to 40): Nden, the mean over the
//! day of how loud every moment is (each period's level distribution, `crate::percentiles`), so a
//! sound counts by how loud it is and how long it lasts, and a rare loud pass weighs little where
//! Lden's energy lets it lead (owner 2026-10-08); the evening's 5 dB and the night's 10 dB are
//! added to the level first, as Lden adds them, and the periods weigh their hours. A moment's
//! spectrum is the period's received one: every ground layer's octave bands as its evaluated
//! pieces arrive (A-weighted, unweighted again, each octave shared by its thirds), the flights' as
//! the loudest flight's spectral classes at its slant distance through the place's air, each scaled
//! to its layer's energy, set to the moment's level and taken as steady. A source alone is
//! measured the same way: its own Nden ranks it in the list.

use crate::distribution::Distribution;
use crate::percentiles::Line;
use crate::selection::LayerSelection;
use physics::bands::{
    A_WEIGHTING_DB as OCTAVE_A_DB, BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy,
};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START, THIRD_OCTAVES, zwicker_loudness_sone};
use tiles::sources::Layer;

/// The mean loudness of each period as it sounds and Nden, the day's with the evening and night
/// penalties (sone; 0 for silence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub mean_sone: [f64; PERIODS],
    pub nden_sone: f64,
}

/// The heard flights: their A-weighted energy per period and their spectrum's shape (unweighted
/// third-octave levels from 50 Hz, dB up to a constant).
pub struct FlightSound {
    pub energy: [f64; PERIODS],
    pub spectrum_db: Option<[f64; CLASS_THIRDS]>,
}

/// No flights heard: a ground layer's own sound.
pub const NO_FLIGHTS: FlightSound = FlightSound {
    energy: [0.0; PERIODS],
    spectrum_db: None,
};

/// Loudness is tabled at whole decibels over this span of A-weighted levels (dB).
const CURVE_LOW_DB: f64 = -30.0;
const CURVE_HIGH_DB: f64 = 160.0;

/// One period's loudness by A-weighted level: its received spectrum set to each whole decibel,
/// interpolated between.
pub struct Curve(Vec<f64>);

impl Curve {
    fn new(thirds: &[f64; THIRD_OCTAVES]) -> Self {
        let a_weighted: f64 = thirds
            .iter()
            .zip(A_WEIGHTING_DB)
            .map(|(value, weight)| value * energy(weight))
            .sum();
        let steps = (CURVE_HIGH_DB - CURVE_LOW_DB) as usize;
        Curve(
            (0..=steps)
                .map(|step| {
                    if a_weighted <= 0.0 {
                        return 0.0;
                    }
                    let scale = energy(CURVE_LOW_DB + step as f64) / a_weighted;
                    zwicker_loudness_sone(&thirds.map(|value| {
                        let scaled = value * scale;
                        if scaled > 0.0 {
                            10.0 * scaled.log10()
                        } else {
                            f64::NEG_INFINITY
                        }
                    }))
                })
                .collect(),
        )
    }

    /// The loudness (sone) at `level_db`.
    pub fn at(&self, level_db: f64) -> f64 {
        let position = (level_db - CURVE_LOW_DB).clamp(0.0, (self.0.len() - 1) as f64);
        let below = (position.floor() as usize).min(self.0.len() - 2);
        self.0[below] + (self.0[below + 1] - self.0[below]) * (position - below as f64)
    }
}

/// Each period's loudness curve of the click's received spectrum.
pub fn curves(selections: &[LayerSelection], flights: &FlightSound) -> [Curve; PERIODS] {
    std::array::from_fn(|period| Curve::new(&unweighted_thirds(selections, flights, period)))
}

/// Each layer's own curves, its received spectrum alone (the flights' for the aircraft layer): a
/// source alone sounds as its layer does, not as the place.
pub fn layer_curves(
    selections: &[LayerSelection],
    flights: &FlightSound,
) -> Vec<(Layer, [Curve; PERIODS])> {
    selections
        .iter()
        .map(|selection| {
            let own = if selection.layer == Layer::Aircraft {
                flights
            } else {
                &NO_FLIGHTS
            };
            (
                selection.layer,
                curves(std::slice::from_ref(selection), own),
            )
        })
        .collect()
}

/// The mean loudness of levels with their shares of the time, `penalty_db` added.
fn mean_over(levels: impl Iterator<Item = (f64, f64)>, curve: &Curve, penalty_db: f64) -> f64 {
    levels
        .map(|(level, share)| share * curve.at(level + penalty_db))
        .sum()
}

/// Nden of per-period means taken with the penalties: the periods over their hours.
fn day(penalised: [f64; PERIODS]) -> f64 {
    (0..PERIODS)
        .map(|period| PERIOD_HOURS[period] * penalised[period])
        .sum::<f64>()
        / 24.0
}

/// The click's loudness from its level distributions.
pub fn loudness(distributions: &[Distribution; PERIODS], curves: &[Curve; PERIODS]) -> Loudness {
    let mean = |penalty: &dyn Fn(usize) -> f64| -> [f64; PERIODS] {
        std::array::from_fn(|period| {
            mean_over(
                distributions[period].levels(),
                &curves[period],
                penalty(period),
            )
        })
    };
    Loudness {
        mean_sone: mean(&|_| 0.0),
        nden_sone: day(mean(&|period| PERIOD_PENALTY_DB[period])),
    }
}

/// A source alone, its `line` over a steady `floor` per period (the floor alone without a line):
/// its Nden.
pub fn own_nden(line: Option<&Line>, floor: [f64; PERIODS], curves: &[Curve; PERIODS]) -> f64 {
    day(std::array::from_fn(|period| {
        let mut sum = 0.0;
        let mut visit = |level: f64, share: f64| {
            sum += share * curves[period].at(level + PERIOD_PENALTY_DB[period]);
        };
        match line {
            Some(line) => line.own_levels(period, floor[period], &mut visit),
            None if floor[period] > 0.0 => visit(10.0 * floor[period].log10(), 1.0),
            None => {}
        }
        sum
    }))
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
    use crate::percentiles::Weather;

    fn layer(kind: Layer, energy_db: f64, shape: [f64; BANDS]) -> LayerSelection {
        let mut selection = LayerSelection::new(kind);
        selection.energy = [energy(energy_db); PERIODS];
        selection.spectrum = [shape; PERIODS];
        selection
    }

    const NONE: FlightSound = NO_FLIGHTS;
    /// A road's received A-weighted octave energies at 50 km/h (relative).
    const ROAD_SHAPE: [f64; BANDS] = [0.002, 0.03, 0.09, 0.28, 0.42, 0.18, 0.03, 0.004];

    /// Every period at `levels` (dB with their shares of the time).
    fn spread(levels: &[(f64, f64)]) -> [Distribution; PERIODS] {
        std::array::from_fn(|_| Distribution::of_levels(levels))
    }

    fn road_curves() -> [Curve; PERIODS] {
        curves(&[layer(Layer::Road, 60.0, ROAD_SHAPE)], &NONE)
    }

    /// A steady road's loudness grows by about 2x per 10 dB; silence and a shape-less layer give 0;
    /// an aircraft's distant rumble sounds less loud than a road at the same level (its energy sits
    /// low, where the ear is deaf); flights without a spectral class keep their layer's shape.
    #[test]
    fn loudness_doubles_per_ten_decibels_of_a_steady_sound() {
        let road = road_curves();
        let at = |db: f64| loudness(&spread(&[(db, 1.0)]), &road).mean_sone[0];
        let ratio = at(70.0) / at(60.0);
        assert!((1.8..2.3).contains(&ratio), "{ratio}");
        assert!((10.0..30.0).contains(&at(65.0)), "{}", at(65.0));
        assert_eq!(
            loudness(&spread(&[(f64::NEG_INFINITY, 1.0)]), &road).mean_sone[0],
            0.0
        );
        let shapeless = curves(&[layer(Layer::Road, 60.0, [0.0; BANDS])], &NONE);
        assert_eq!(
            loudness(&spread(&[(65.0, 1.0)]), &shapeless).mean_sone[0],
            0.0
        );
        let mut rumble = [0.0; CLASS_THIRDS];
        for (n, level) in rumble.iter_mut().enumerate() {
            *level = 80.0 - 4.0 * n as f64;
        }
        let flights = FlightSound {
            energy: [energy(60.0); PERIODS],
            spectrum_db: Some(rumble),
        };
        let aircraft = curves(&[layer(Layer::Aircraft, 60.0, [0.0; BANDS])], &flights);
        let heard = loudness(&spread(&[(65.0, 1.0)]), &aircraft).mean_sone[0];
        assert!(heard > 0.0 && heard < at(65.0), "{heard} {}", at(65.0));
        // A helipad: its loudest flight has no spectral class, so the layer keeps its own shape.
        let unclassed = FlightSound {
            energy: [energy(60.0); PERIODS],
            spectrum_db: None,
        };
        let helipad = [layer(Layer::Aircraft, 60.0, ROAD_SHAPE)];
        let with_flights = loudness(&spread(&[(65.0, 1.0)]), &curves(&helipad, &unclassed));
        assert!(with_flights.mean_sone[0] > 0.0);
        assert_eq!(
            with_flights,
            loudness(&spread(&[(65.0, 1.0)]), &curves(&helipad, &NONE))
        );
    }

    /// A sound counts by how long it lasts: heard half of each period it reads half its steady
    /// loudness, where its energy average sits only 3 dB down; a pass loud for a hundredth of the
    /// time adds a hundredth of its loudness. Nden weighs the periods over their hours with the
    /// evening's 5 dB and the night's 10 dB added first: a sound in the day alone reads half its
    /// day.
    #[test]
    fn loudness_counts_how_long_a_sound_lasts() {
        let road = road_curves();
        let steady = loudness(&spread(&[(60.0, 1.0)]), &road);
        let half = loudness(&spread(&[(60.0, 0.5), (f64::NEG_INFINITY, 0.5)]), &road);
        assert!((half.mean_sone[0] / steady.mean_sone[0] - 0.5).abs() < 1e-9);
        let rare = loudness(&spread(&[(40.0, 0.99), (80.0, 0.01)]), &road).mean_sone[0];
        let expected = 0.99 * road[0].at(40.0) + 0.01 * road[0].at(80.0);
        assert!((rare - expected).abs() < 1e-9 * expected);
        let day_only = [
            Distribution::of_levels(&[(60.0, 1.0)]),
            Distribution::of_levels(&[(f64::NEG_INFINITY, 1.0)]),
            Distribution::of_levels(&[(f64::NEG_INFINITY, 1.0)]),
        ];
        let day = loudness(&day_only, &road);
        assert!((day.nden_sone - day.mean_sone[0] / 2.0).abs() < 1e-9);
        let night_only = [
            Distribution::of_levels(&[(f64::NEG_INFINITY, 1.0)]),
            Distribution::of_levels(&[(f64::NEG_INFINITY, 1.0)]),
            Distribution::of_levels(&[(60.0, 1.0)]),
        ];
        let night = loudness(&night_only, &road).nden_sone;
        assert!((night - road[2].at(70.0) / 3.0).abs() < 1e-9);
    }

    /// A source alone: steady, its loudness at its level all day; a car every half hour 4 m away
    /// far under its steady self though its energy is the same; over a steady floor, the floor's
    /// loudness at least.
    #[test]
    fn a_source_alone_reads_its_own_nden() {
        let road = road_curves();
        let steady = own_nden(None, [energy(50.0); PERIODS], &road);
        let expected =
            (12.0 * road[0].at(50.0) + 4.0 * road[1].at(55.0) + 8.0 * road[2].at(60.0)) / 24.0;
        assert!((steady - expected).abs() < 1e-9);
        let weather = Weather::default();
        let lane = crate::percentiles::Line::flights(
            &weather,
            ([energy(50.0); PERIODS], [energy(50.0) * 1e-3; PERIODS]),
        );
        let rare = own_nden(Some(&lane), [0.0; PERIODS], &road);
        assert!(rare > 0.0 && rare < 0.5 * steady, "{rare} {steady}");
        let over_floor = own_nden(Some(&lane), [energy(45.0); PERIODS], &road);
        assert!(over_floor > own_nden(None, [energy(45.0); PERIODS], &road));
    }
}
