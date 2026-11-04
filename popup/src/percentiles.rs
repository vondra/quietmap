//! How the level of a click spreads over each period: the share of the time at each level, from
//! which come the levels exceeded 5, 10, 50 and 90 % of the time (L5, L10, L50, L90) and the mean
//! loudness ([`crate::loudness`]). Every contributor is a line of emitters of Kurze's statistics
//! (`physics::percentile`) at its own lambda = (emitters per metre) x (its distance): vehicles on
//! a road (the daily flow over the period's share and hours, over the speed), trains on a track
//! (the period's trains over its hours, over the speed), airport movements on an aeroway (at taxi
//! speed); the flights heard one more line at the energy-weighted lambda of the boxes and the
//! runways' roll paths (`crate::aircraft::rolls`, a path's classes one stream); events (church
//! bells) on for their duty, the share of the period they sound, at their mean over it; industry,
//! buildings, ships and the unlisted remainder steady at their mean. A road's flow follows the
//! hours of the day within each period (measured hourly profiles: a night's 3 am carries a fifth
//! of its mean), all roads the same hour. The spread is computed, not drawn: for every hour and
//! weather state the steady energy with each line's quantiles added (the lines independent, on a
//! grid of 0.5 dB), averaged over the hours and states. The same traffic tells how a contributor
//! is heard: its passes per hour, and whether at its distance they run together into a steady
//! sound.

use crate::candidates::DisplayRef;
use crate::distribution::{Distribution, LineValues, line_values};
use crate::selection::LayerSelection;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::percentile::relative_intensity;
use rayon::prelude::*;
use tiles::sources::Layer;

mod lines;
mod traffic;
mod weather;
pub(crate) use lines::at_mean_flow;
pub use lines::{
    Group, HELD_EXCEEDED, HELD_SHARE, HOLDS_MEAN_LAMBDA, Line, LineParts, click_lines, place_lines,
};
use traffic::lambda;
pub use traffic::{Heard, Passes, hour_factor, hour_groups, passes_at};
use weather::weather_means;
pub(crate) use weather::{WEATHER_STATES, favourable_share};
pub use weather::{Weather, flight_weather, merged_states};

/// The levels (dB, `-inf` silent) exceeded 5, 10, 50 and 90 % of the time per period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub l5: [f64; PERIODS],
    pub l10: [f64; PERIODS],
    pub l50: [f64; PERIODS],
    pub l90: [f64; PERIODS],
}

/// How finely the level distributions are computed: the grid's step (dB), the equally likely
/// weather states (the native ten merged into as many) and the hours of a period with a profile
/// (`None` every hour apart; groups of hours by their flow, each at its hours' mean flow).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    pub bin_db: f64,
    pub weather_states: usize,
    pub hour_groups: Option<[usize; PERIODS]>,
}

impl Resolution {
    /// A click's: 0.5 dB, Nden within 0.09 % of 0.1 dB's at 919 points (measured 2026-10-12).
    pub const CLICK: Resolution = Resolution {
        bin_db: 0.5,
        weather_states: WEATHER_STATES,
        hour_groups: None,
    };
    /// The map's: 2 dB, two weather states, four, one and three groups of the day's, evening's
    /// and night's hours. On Dobris's 1,125 popup clicks the painted total is within 1 phon but at
    /// one cell (1.5), over 0.5 phon at 4.8 %, bias +0.23 phon (1 dB and three states: 1.8 %,
    /// +0.16 phon); against the sone etalon the drift the same; the painter a tenth faster
    /// (measured 2026-10-12).
    pub const MAP: Resolution = Resolution {
        bin_db: 2.0,
        weather_states: 2,
        hour_groups: Some([4, 1, 3]),
    };
}

/// The distribution of the summed level over each period at `resolution` of `lines` over the
/// `steady` energy: [`layered_distributions`] of one layer.
pub fn distributions_of(
    lines: &[Line],
    steady: [f64; PERIODS],
    resolution: Resolution,
) -> [Distribution; PERIODS] {
    layered_distributions(&[(lines, steady)], resolution).1
}

/// The distribution of the summed level over each period at `resolution` of each of `layers` (its
/// lines over its steady energy) and of all of them together: for every group of hours (the roads'
/// flows follow the hour together) and every weather state (one for every ray of the place), each
/// layer's steady energy with its moving lines added, independent; the place's the layers' added,
/// independent at that moment too (each line once, not again for the place); the groups and states
/// averaged by their shares of the time.
pub fn layered_distributions(
    layers: &[(&[Line], [f64; PERIODS])],
    resolution: Resolution,
) -> (Vec<[Distribution; PERIODS]>, [Distribution; PERIODS]) {
    let follows = |lines: &[Line]| lines.iter().any(|line| line.profile.is_some());
    let profiled = layers.iter().any(|(lines, _)| follows(lines));
    let states = resolution.weather_states;
    let periods: [(Vec<Distribution>, Distribution); PERIODS] = std::array::from_fn(|p| {
        let (groups, hours) = hour_groups(profiled, p, resolution.hour_groups);
        // A layer none of whose lines follows a profile sounds alike in every group of hours.
        let alike: Vec<Option<States>> = (layers.iter())
            .map(|&(lines, steady)| {
                (!follows(lines))
                    .then(|| layer_states(lines, steady[p], (p, &groups[0]), resolution))
            })
            .collect();
        // Every group of hours apart, in parallel: each layer's distribution in each weather state,
        // and the place's, the layers' added.
        let parts: Vec<GroupPart> = (groups.par_iter())
            .map(|slots| {
                let own: Vec<Option<States>> = (layers.iter().zip(&alike))
                    .map(|(&(lines, steady), alike)| {
                        alike
                            .is_none()
                            .then(|| layer_states(lines, steady[p], (p, slots), resolution))
                    })
                    .collect();
                let wholes = (0..states)
                    .map(|state| {
                        // What holds steady in every layer at once, then the layers that move.
                        let each = (own.iter().zip(&alike))
                            .map(|(own, alike)| &own.as_ref().or(alike.as_ref()).unwrap()[state]);
                        let (steady, moving): (Vec<_>, Vec<_>) =
                            each.partition(|(_, floor)| floor.is_some());
                        let floor = steady.iter().filter_map(|(_, floor)| *floor).sum::<f64>();
                        let mut whole = Distribution::empty(resolution.bin_db);
                        whole.silent = 1.0;
                        (moving.iter()).fold(whole.over(floor), |whole, (layer, _)| {
                            whole.with_distribution(layer)
                        })
                    })
                    .collect();
                (slots.len() as f64 / (hours * states) as f64, own, wholes)
            })
            .collect();
        let mut each: Vec<Distribution> = (alike.iter())
            .map(|alike| {
                let mut mixture = Distribution::empty(resolution.bin_db);
                for (part, _) in alike.iter().flatten() {
                    mixture.add_scaled(part, 1.0 / states as f64);
                }
                mixture
            })
            .collect();
        let mut whole = Distribution::empty(resolution.bin_db);
        for (weight, own, wholes) in &parts {
            for (mixture, own) in each.iter_mut().zip(own) {
                for (part, _) in own.iter().flatten() {
                    mixture.add_scaled(part, *weight);
                }
            }
            for part in wholes {
                whole.add_scaled(part, *weight);
            }
        }
        (each, whole)
    });
    let [day, evening, night] = periods;
    let whole = [day.1, evening.1, night.1];
    let each = (day.0.into_iter().zip(evening.0).zip(night.0))
        .map(|((day, evening), night)| [day, evening, night])
        .collect();
    (each, whole)
}

/// Per weather state a distribution, and its intensity where it holds steady (no line moves).
type States = Vec<(Distribution, Option<f64>)>;

/// A group of hours' share of its period, each layer's states there where they differ from the
/// period's, and the place's distribution in each state.
type GroupPart = (f64, Vec<Option<States>>, Vec<Distribution>);

/// One layer's distributions in the hours `slots` of `period`, one per weather state of
/// `resolution`: its `steady` energy with its moving lines added, their shapes shared by the
/// states; where none moves, that energy.
fn layer_states(
    lines: &[Line],
    steady: f64,
    (period, slots): (usize, &[usize]),
    resolution: Resolution,
) -> States {
    let factors: Vec<f64> = (lines.iter())
        .map(|line| {
            line.profile.map_or(1.0, |profile| {
                (slots.iter())
                    .map(|&slot| hour_factor(profile, period, slot))
                    .sum::<f64>()
                    / slots.len() as f64
            })
        })
        .collect();
    let shapes: Vec<Option<Shape>> = (lines.iter().zip(&factors))
        .map(|(line, &factor)| line.shape(period, factor))
        .collect();
    let means: Vec<Vec<f64>> = (lines.iter())
        .map(|line| merged_states(&line.means[period], resolution.weather_states))
        .collect();
    (0..resolution.weather_states)
        .map(|state| {
            let state_means: Vec<f64> = means.iter().map(|means| means[state]).collect();
            let moment = moment(lines, &shapes, (&state_means, &factors), steady, period);
            // What holds steady first: a moving line's quiet moments fall into its bin.
            let mut sum = Distribution::empty(resolution.bin_db);
            sum.silent = 1.0;
            sum = sum.over(moment.floor);
            for &(_, values, mean) in &moment.moving {
                sum = sum.with_line((values, mean));
            }
            (sum, moment.moving.is_empty().then_some(moment.floor))
        })
        .collect()
}

/// A line's moving values in hours of a period, relative to its mean then (each weather state
/// scales them by its own): an event's silence and sound, or a sparse line's Kurze quantiles at its
/// lambda then, with its intensity exceeded a thousandth of the time.
pub(crate) struct Shape {
    values: LineValues,
    /// `None` for an event, which moves the level whenever it sounds.
    exceeded: Option<f64>,
}

/// One hour and weather state of a period: the energy that holds steady (`steady` and the lines
/// that hold their mean, each such line's part by its index) and the lines that move the level,
/// with their values relative to their mean and that mean, the events first.
pub(crate) struct Moment<'a> {
    pub floor: f64,
    pub holding: Vec<(usize, f64)>,
    pub moving: Vec<(usize, &'a LineValues, f64)>,
}

/// The moment of `lines` in `period`: their `shapes` then, each line's mean in the moment's
/// weather state and its flow then against the period's mean (`state_means`, `factors`).
pub(crate) fn moment<'a>(
    lines: &[Line],
    shapes: &'a [Option<Shape>],
    (state_means, factors): (&[f64], &[f64]),
    steady: f64,
    period: usize,
) -> Moment<'a> {
    let mut moment = Moment {
        floor: steady,
        holding: Vec::new(),
        moving: Vec::new(),
    };
    let mut held = Vec::new();
    for (index, (line, shape)) in lines.iter().zip(shapes).enumerate() {
        let mean = state_means[index] * factors[index];
        if line.energy[period] <= 0.0 || mean <= 0.0 {
            continue;
        }
        match shape {
            Some(Shape {
                values,
                exceeded: None,
            }) => moment.moving.push((index, values, mean)),
            Some(Shape {
                values,
                exceeded: Some(exceeded),
            }) => held.push((index, values, mean, *exceeded)),
            None if line.duty.is_some() => {}
            None => {
                moment.floor += mean;
                moment.holding.push((index, mean));
            }
        }
    }
    // A line moves the level unless its level exceeded a thousandth of the time stays a hundredth
    // of the steady energy: then it holds its mean in it.
    let steady_energy = moment.floor + held.iter().map(|&(_, _, mean, _)| mean).sum::<f64>();
    for (index, values, mean, exceeded) in held {
        if mean * exceeded < HELD_SHARE * steady_energy {
            moment.floor += mean;
            moment.holding.push((index, mean));
        } else {
            moment.moving.push((index, values, mean));
        }
    }
    moment
}

impl Percentiles {
    /// The levels exceeded 5, 10, 50 and 90 % of each period's time.
    pub fn of(distributions: &[Distribution; PERIODS]) -> Self {
        let at = |exceeded: f64| std::array::from_fn(|p| distributions[p].exceeded_db(exceeded));
        Percentiles {
            l5: at(0.05),
            l10: at(0.1),
            l50: at(0.5),
            l90: at(0.9),
        }
    }
}

#[cfg(test)]
#[path = "percentiles_tests.rs"]
pub(crate) mod tests;
