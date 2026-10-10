//! How the level of a click spreads over each period: the share of the time at each level, from
//! which come the levels exceeded 5, 10, 50 and 90 % of the time (L5, L10, L50, L90) and the mean
//! loudness ([`crate::loudness`]). Every contributor is a line of emitters of Kurze's statistics
//! (`physics::percentile`) at its own lambda = (emitters per metre) x (its distance): vehicles on
//! a road (the daily flow over the period's share and hours, over the speed), trains on a track
//! (the period's trains over its hours, over the speed), airport movements on an aeroway (at taxi
//! speed); the flights heard one more line at the boxes' energy-weighted lambda; events (church
//! bells) on for their duty, the share of the period they sound, at their mean over it; industry,
//! buildings, ships and the unlisted remainder steady at their mean. A road's flow follows the
//! hours of the day within each period (measured hourly profiles: a night's 3 am carries a fifth
//! of its mean), all roads the same hour. The spread is computed, not drawn: for every hour and
//! weather state the steady energy with each line's quantiles added (the lines independent, on a
//! grid of 0.1 dB), averaged over the hours and states. The same traffic tells how a contributor
//! is heard: its passes per hour, and whether at its distance they run together into a steady
//! sound.

use crate::distribution::{Distribution, line_values};
use crate::selection::LayerSelection;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::percentile::relative_intensity;
use rayon::prelude::*;
use tiles::sources::Layer;

/// Contributors under this share of their period's energy count as steady (their fluctuation does
/// not move a percentile of the sum).
const FLUCTUATING_SHARE_MIN: f64 = 1e-4;
/// The same for the list's last row, everything it leaves out: a coarser account of a row that
/// names nothing ([`rest_lines`]).
const REST_SHARE_MIN: f64 = 1e-2;
/// Day, evening and night shares of a road's daily flow where its fields carry none (sources built
/// before the shares by country): motorways, trunks and their links; other roads.
const MOTORWAY_PERIOD_SHARES: [f64; PERIODS] = [0.65, 0.20, 0.15];
const OTHER_PERIOD_SHARES: [f64; PERIODS] = [0.70, 0.18, 0.12];
/// Each hour's share of a road's daily vehicles (hour 0 is 00-01): medians of Baden-Wuerttemberg's
/// permanent counters over 2025, all days (121 Autobahn, 74 Bundesstrasse and 47 Landesstrasse
/// stations, mobidata-bw.de `stundenwerte_dauerzaehlstellen`): motorways and trunks, primary roads,
/// every other road.
const HOURLY_SHARES: [[f64; 24]; 3] = [
    [
        0.01309, 0.00918, 0.00785, 0.00838, 0.01246, 0.02353, 0.03973, 0.05108, 0.05364, 0.05517,
        0.05799, 0.05978, 0.06152, 0.06281, 0.06338, 0.06522, 0.06718, 0.06558, 0.05905, 0.04848,
        0.03909, 0.03131, 0.02568, 0.01881,
    ],
    [
        0.00684, 0.00423, 0.00330, 0.00331, 0.00691, 0.02047, 0.04139, 0.05610, 0.05161, 0.05350,
        0.05888, 0.06151, 0.06501, 0.06797, 0.07131, 0.07483, 0.08065, 0.07760, 0.06261, 0.04433,
        0.03251, 0.02441, 0.01922, 0.01150,
    ],
    [
        0.00545, 0.00336, 0.00242, 0.00236, 0.00555, 0.02012, 0.03996, 0.05863, 0.05247, 0.05348,
        0.05843, 0.06219, 0.06493, 0.06839, 0.07266, 0.07679, 0.08515, 0.08071, 0.06255, 0.04356,
        0.03107, 0.02300, 0.01719, 0.00958,
    ],
];
/// The first hour of each period (END: day 07-19, evening 19-23, night 23-07).
const PERIOD_FIRST_HOUR: [usize; PERIODS] = [7, 19, 23];

/// A road's flow in the `slot`th hour of `period` against its mean over the period, by its profile.
fn hour_factor(profile: usize, period: usize, slot: usize) -> f64 {
    let hours = PERIOD_HOURS[period] as usize;
    let share = |slot: usize| HOURLY_SHARES[profile][(PERIOD_FIRST_HOUR[period] + slot) % 24];
    let mean = (0..hours).map(share).sum::<f64>() / hours as f64;
    share(slot) / mean
}

/// Airport movements taxi and roll at about 20 kt on average (m/s).
const MOVEMENT_SPEED_M_S: f64 = 10.0;
/// Emitters closer than this are at this distance (m): a receiver on the line itself.
const DISTANCE_MIN_M: f64 = 1.0;

/// The levels (dB, `-inf` silent) exceeded 5, 10, 50 and 90 % of the time per period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub l5: [f64; PERIODS],
    pub l10: [f64; PERIODS],
    pub l50: [f64; PERIODS],
    pub l90: [f64; PERIODS],
}

/// How a contributor is heard: its passes per hour by day, evening and night (vehicles, trains,
/// airport movements), and whether at its distance they run together into a steady sound (on
/// average at least one within its distance by day: Kurze's lambda of at least 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    pub per_hour: [f64; PERIODS],
    pub steady: bool,
}

/// Kurze's lambda from which passes run together into a steady sound.
const STEADY_LAMBDA: f64 = 1.0;

/// Bins of the favourable share in which a contributor keeps its pieces' weather apart.
const SHARE_BINS: usize = 10;

/// A contributor's energy per period in each meteorological state (homogeneous, favourable) and
/// their mix, binned by its pieces' favourable share p (CNOSSOS-EU 2.5.9, by direction): the
/// pieces on either side of a click are favourable at different times (ERA5's sectors differ by
/// up to 50 points across), so one share for the whole would make them all favourable or all calm
/// together. A draw's weather is favourable in the bins whose share lies above it.
#[derive(Clone, Default)]
pub struct Weather([[[f64; PERIODS]; 3]; SHARE_BINS]);

impl Weather {
    /// Adds a piece of `energy` per period, the mix of its `states`; a piece without states
    /// (none computed) holds its mean in either.
    pub fn add(&mut self, energy: &[f64; PERIODS], states: &[[f64; PERIODS]; 2]) {
        for period in 0..PERIODS {
            let [homogeneous, favourable] = if states[0][period] > 0.0 || states[1][period] > 0.0 {
                [states[0][period], states[1][period]]
            } else {
                [energy[period]; 2]
            };
            let share = favourable_share(energy[period], homogeneous, favourable);
            let bin = &mut self.0[((share * SHARE_BINS as f64) as usize).min(SHARE_BINS - 1)];
            bin[0][period] += homogeneous;
            bin[1][period] += favourable;
            bin[2][period] += energy[period];
        }
    }

    /// Each bin's favourable share and its energy of `period` in either state: a draw's weather u
    /// in [0, 1) takes the favourable energy of the bins whose share is above it.
    fn states(&self, period: usize) -> [(f64, f64, f64); SHARE_BINS] {
        self.0.map(|[homogeneous, favourable, mix]| {
            let share = favourable_share(mix[period], homogeneous[period], favourable[period]);
            (share, homogeneous[period], favourable[period])
        })
    }
}

/// The share of the time a source is heard in its favourable state, from its mean being the mix
/// of the two.
pub(crate) fn favourable_share(mean: f64, homogeneous: f64, favourable: f64) -> f64 {
    if (favourable - homogeneous).abs() <= f64::EPSILON * favourable.abs() {
        return 0.0;
    }
    ((mean - homogeneous) / (favourable - homogeneous)).clamp(0.0, 1.0)
}

/// One source whose level varies in time: its mean energy, its weather and lambda per period
/// (infinite for a steady source, whose level follows the weather alone).
#[derive(Clone)]
pub struct Line<'w> {
    /// The contributor's group: the lines are added in its order, so a click repeats exactly.
    pub(crate) key: u64,
    pub energy: [f64; PERIODS],
    weather: &'w Weather,
    lambda: [f64; PERIODS],
    /// A road's hourly profile ([`HOURLY_SHARES`]); `None` keeps the period's mean every hour.
    profile: Option<usize>,
    /// An event source's share of each period it sounds: on, it is its mean over the duty; off,
    /// silent.
    duty: Option<[f64; PERIODS]>,
}

impl<'w> Line<'w> {
    /// A contributor's line: industry, buildings and ships steady but for their events, a road,
    /// railway or aeroway by its traffic; none for one of those whose fields carry no traffic.
    pub fn of(
        contributor: &'w crate::update::Contributor,
        fields: Option<&serde_json::Value>,
    ) -> Option<Self> {
        let steady = matches!(
            contributor.layer,
            Layer::Industry | Layer::Building | Layer::Ship
        );
        let (lambda, profile, duty) = if steady {
            ([f64::INFINITY; PERIODS], None, fields.and_then(duty))
        } else {
            let (per_hour, speed, profile) =
                traffic(contributor.layer, fields?, contributor.energy)?;
            (
                lambda(per_hour, speed, contributor.distance_m),
                profile,
                None,
            )
        };
        Some(Line {
            key: contributor.group_key,
            energy: contributor.energy,
            weather: &contributor.weather,
            lambda,
            profile,
            duty,
        })
    }
}

impl<'w> Line<'w> {
    /// The flights heard, one line at their energy-weighted lambda (`weather` theirs: the mean in
    /// either state).
    pub fn flights(
        weather: &'w Weather,
        (energy, energy_lambda): ([f64; PERIODS], [f64; PERIODS]),
    ) -> Self {
        Line {
            key: u64::MAX,
            energy,
            weather,
            lambda: std::array::from_fn(|p| {
                if energy[p] > 0.0 {
                    energy_lambda[p] / energy[p]
                } else {
                    f64::INFINITY
                }
            }),
            profile: None,
            duty: None,
        }
    }

    /// The line's mean in each weather state of `period`: a piece is favourable while the place's
    /// weather u lies under its favourable share, so over the state's span of u it is favourable
    /// for the part of the span under the share (no step where a share crosses a state's middle).
    pub(crate) fn state_means(&self, period: usize) -> [f64; WEATHER_STATES] {
        let states = self.weather.states(period);
        if states
            .iter()
            .all(|&(_, homogeneous, favourable)| homogeneous + favourable <= 0.0)
        {
            return [self.energy[period]; WEATHER_STATES];
        }
        std::array::from_fn(|state| {
            let span = 1.0 / WEATHER_STATES as f64;
            let low = state as f64 * span;
            states
                .iter()
                .map(|&(share, homogeneous, favourable)| {
                    let favoured = (share.clamp(low, low + span) - low) / span;
                    favoured * favourable + (1.0 - favoured) * homogeneous
                })
                .sum()
        })
    }

    /// The line's intensities in an hour of `period` with `factor` its flow then and the line at
    /// `mean` (its weather state's), each with its share of the time; a line that holds its mean
    /// one value, an event its duty.
    fn values(&self, period: usize, mean: f64, factor: f64) -> Vec<(f64, f64)> {
        let lambda = self.lambda[period] * factor;
        match self.duty.map(|duty| duty[period]) {
            Some(duty) if duty > 0.0 => line_values(mean * factor, 0.0, Some(duty)),
            Some(_) => Vec::new(),
            None if lambda >= HOLDS_MEAN_LAMBDA => vec![(mean * factor, 1.0)],
            None => line_values(mean * factor, lambda, None),
        }
    }

    /// The line alone over a steady `floor` in `period`: each of its levels (dB) with its share of
    /// the period, over every hour and weather state; silence apart.
    pub fn own_levels(&self, period: usize, floor: f64, visit: &mut dyn FnMut(f64, f64)) {
        if self.energy[period] <= 0.0 {
            if floor > 0.0 {
                visit(10.0 * floor.log10(), 1.0);
            }
            return;
        }
        let factors: Vec<f64> = match self.profile {
            Some(profile) => (0..PERIOD_HOURS[period] as usize)
                .map(|slot| hour_factor(profile, period, slot))
                .collect(),
            None => vec![1.0],
        };
        let weight = 1.0 / (factors.len() * WEATHER_STATES) as f64;
        for mean in self.state_means(period) {
            for &factor in &factors {
                let values = self.values(period, mean, factor);
                let silent = 1.0 - values.iter().map(|(_, share)| share).sum::<f64>();
                for (value, share) in values.into_iter().chain([(0.0, silent)]) {
                    if floor + value > 0.0 && share > 0.0 {
                        visit(10.0 * (floor + value).log10(), share * weight);
                    }
                }
            }
        }
    }
}

fn number(fields: &serde_json::Value, name: &str) -> Option<f64> {
    fields.get(name)?.as_f64()
}

/// A road's day, evening and night shares of its daily flow, as the sources builder wrote them.
fn period_shares(fields: &serde_json::Value) -> Option<[f64; PERIODS]> {
    let values = fields.get("period_shares")?.as_array()?;
    let shares: Vec<f64> = values
        .iter()
        .filter_map(serde_json::Value::as_f64)
        .collect();
    (shares.len() == PERIODS && shares.iter().all(|share| *share >= 0.0))
        .then(|| std::array::from_fn(|p| shares[p]))
}

/// An event source's duty per period (the share of the period it sounds), from its display
/// fields; `None` for any other source.
fn duty(fields: &serde_json::Value) -> Option<[f64; PERIODS]> {
    let values = fields.get("duty")?.as_array()?;
    let duty: Vec<f64> = values
        .iter()
        .filter_map(serde_json::Value::as_f64)
        .collect();
    (duty.len() == PERIODS && duty.iter().all(|share| (0.0..=1.0).contains(share)))
        .then(|| std::array::from_fn(|p| duty[p]))
}

/// An event source's events per hour by day, evening and night, from its events a day per
/// period.
fn events_per_hour(fields: &serde_json::Value) -> Option<[f64; PERIODS]> {
    let values = fields.get("events_per_day")?.as_array()?;
    let events: Vec<f64> = values
        .iter()
        .filter_map(serde_json::Value::as_f64)
        .collect();
    (events.len() == PERIODS).then(|| std::array::from_fn(|p| events[p] / PERIOD_HOURS[p]))
}

/// A contributor's passes per hour by day, evening and night, their speed (m/s) and, for a road,
/// its hourly profile by class, from its display fields: vehicles on a road (the daily flow over
/// the period's share and hours), trains on a track (the period's trains over its hours), airport
/// movements and ground vehicles on an aeroway (at taxi speed, by the periods its `energy` fell
/// in); `None` for a steady source (or one whose fields say nothing).
fn traffic(
    layer: Layer,
    fields: &serde_json::Value,
    energy: [f64; PERIODS],
) -> Option<([f64; PERIODS], f64, Option<usize>)> {
    match layer {
        Layer::Road => {
            let daily: f64 = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
                .iter()
                .filter_map(|name| number(fields, name))
                .sum();
            let speed = number(fields, "speed_kmh")? / 3.6;
            let class = fields
                .get("road_class")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let motorway = matches!(class, "motorway" | "trunk" | "motorway_link" | "trunk_link");
            let shares = period_shares(fields).unwrap_or(if motorway {
                MOTORWAY_PERIOD_SHARES
            } else {
                OTHER_PERIOD_SHARES
            });
            let profile = if motorway {
                0
            } else if matches!(class, "primary" | "primary_link") {
                1
            } else {
                2
            };
            (speed > 0.0 && daily > 0.0).then(|| {
                (
                    std::array::from_fn(|p| daily * shares[p] / PERIOD_HOURS[p]),
                    speed,
                    Some(profile),
                )
            })
        }
        Layer::Railway => {
            let speed = number(fields, "speed_kmh")? / 3.6;
            let names = [
                ["trains_passenger_day", "trains_freight_day"],
                ["trains_passenger_evening", "trains_freight_evening"],
                ["trains_passenger_night", "trains_freight_night"],
            ];
            (speed > 0.0).then(|| {
                let per_hour = std::array::from_fn(|p| {
                    let trains: f64 = names[p]
                        .iter()
                        .filter_map(|name| number(fields, name))
                        .sum();
                    trains / PERIOD_HOURS[p]
                });
                (per_hour, speed, None)
            })
        }
        Layer::Aircraft => {
            let daily: f64 = [
                "arrivals_per_day",
                "departures_per_day",
                "ground_vehicles_per_day",
            ]
            .iter()
            .filter_map(|name| number(fields, name))
            .sum();
            let total: f64 = energy.iter().sum();
            (daily > 0.0 && total > 0.0).then(|| {
                (
                    std::array::from_fn(|p| daily * energy[p] / total / PERIOD_HOURS[p]),
                    MOVEMENT_SPEED_M_S,
                    None,
                )
            })
        }
        Layer::Industry | Layer::Building | Layer::Ship => None,
    }
}

/// Kurze's lambda per period: the emitters within the contributor's distance of a line of them.
fn lambda(per_hour: [f64; PERIODS], speed_m_s: f64, distance_m: f64) -> [f64; PERIODS] {
    per_hour.map(|passes| passes / 3_600.0 / speed_m_s * distance_m.max(DISTANCE_MIN_M))
}

/// How `contributor` is heard, from its display fields: its passes, or its events (church bells
/// ringing), per hour; `None` for a steady source.
pub fn heard(
    contributor: &crate::update::Contributor,
    fields: &serde_json::Value,
) -> Option<Heard> {
    if let Some(per_hour) = events_per_hour(fields) {
        return Some(Heard {
            per_hour,
            steady: false,
        });
    }
    let (per_hour, speed, _) = traffic(contributor.layer, fields, contributor.energy)?;
    Some(Heard {
        per_hour,
        steady: lambda(per_hour, speed, contributor.distance_m)[0] >= STEADY_LAMBDA,
    })
}

/// Equally likely weather states of the place, one for every ray of it (a night's inversion bends
/// them alike): in state k a piece is favourable when its share exceeds (k + 0.5) / WEATHER_STATES.
pub(crate) const WEATHER_STATES: usize = 10;
/// A line at least this dense (Kurze's lambda) holds its mean: its level stays within about 0.5 dB.
const HOLDS_MEAN_LAMBDA: f64 = 10.0;
/// A line held at its mean: one whose level exceeded a thousandth of the time stays a hundredth of
/// the steady energy's (20 dB under it), so it moves no level by more than a bin but for moments
/// too rare to count.
const HELD_EXCEEDED: f64 = 1e-3;
const HELD_SHARE: f64 = 1e-2;

/// The lines of what the list leaves out, summing to `total` per period, for its last row's Nden:
/// those over [`REST_SHARE_MIN`] of it, each at its period's mean flow every hour (the hours'
/// profile moved that Nden by 1.7 % at most at ten places and took it 3-4 times as long,
/// 2026-10-09); and what is left, steady.
pub fn rest_lines<'a>(
    contributors: impl Iterator<Item = &'a crate::update::Contributor>,
    total: [f64; PERIODS],
    fields: &dyn Fn(&crate::update::Contributor) -> Option<serde_json::Value>,
) -> (Vec<Line<'a>>, [f64; PERIODS]) {
    let (mut lines, steady) = lines(contributors, total, REST_SHARE_MIN, fields);
    at_mean_flow(&mut lines);
    (lines, steady)
}

/// Every line at its period's mean flow every hour.
pub(crate) fn at_mean_flow(lines: &mut [Line]) {
    for line in lines {
        line.profile = None;
    }
}

/// The lines of `contributors` summing to `total` per period: every one with traffic in its fields
/// (or steady but for its events) over `share_min` of its period, in a fixed order; and what is
/// left of each period's energy, steady.
fn lines<'a>(
    contributors: impl Iterator<Item = &'a crate::update::Contributor>,
    total: [f64; PERIODS],
    share_min: f64,
    fields: &dyn Fn(&crate::update::Contributor) -> Option<serde_json::Value>,
) -> (Vec<Line<'a>>, [f64; PERIODS]) {
    let mut lines: Vec<Line<'a>> = contributors
        .filter(|contributor| (0..PERIODS).any(|p| contributor.energy[p] > share_min * total[p]))
        .filter_map(|contributor| Line::of(contributor, fields(contributor).as_ref()))
        .collect();
    // The contributors come from hash maps: a fixed order keeps a click's rounding the same.
    lines.sort_by_key(|line| line.key);
    let steady = std::array::from_fn(|p| {
        (total[p] - lines.iter().map(|line| line.energy[p]).sum::<f64>()).max(0.0)
    });
    (lines, steady)
}

/// The flights' weather: their mean in either state.
pub fn flight_weather(flights: &([f64; PERIODS], [f64; PERIODS])) -> Weather {
    let mut weather = Weather::default();
    weather.add(&flights.0, &[flights.0; 2]);
    weather
}

/// The lines of a click, every contributor of `selections` and the flights heard (`weather` from
/// [`flight_weather`]), and what is left of each period's energy, steady.
pub fn click_lines<'a>(
    selections: &'a [LayerSelection],
    (weather, flights): (&'a Weather, ([f64; PERIODS], [f64; PERIODS])),
    fields: &dyn Fn(&crate::update::Contributor) -> Option<serde_json::Value>,
) -> (Vec<Line<'a>>, [f64; PERIODS]) {
    let total: [f64; PERIODS] =
        std::array::from_fn(|p| selections.iter().map(|s| s.answer_energy()[p]).sum::<f64>());
    let (mut lines, mut steady) = lines(
        selections
            .iter()
            .flat_map(|selection| selection.contributors.values()),
        total,
        FLUCTUATING_SHARE_MIN,
        fields,
    );
    let flights_line = Line::flights(weather, flights);
    for (steady, energy) in steady.iter_mut().zip(flights_line.energy) {
        *steady = (*steady - energy).max(0.0);
    }
    lines.push(flights_line);
    (lines, steady)
}

/// The distribution of the summed level over each period: for every hour of it (the roads' flows
/// follow the hour together) and every weather state (one for every ray of the place), the lines
/// that move the level added, independent, then the steady energy; the hours and states averaged.
pub fn distributions_of(lines: &[Line], steady: [f64; PERIODS]) -> [Distribution; PERIODS] {
    std::array::from_fn(|p| {
        let hours = PERIOD_HOURS[p] as usize;
        let means: Vec<[f64; WEATHER_STATES]> =
            lines.iter().map(|line| line.state_means(p)).collect();
        // The hours differ only where a road follows its hourly profile.
        let hours = if lines.iter().any(|line| line.profile.is_some()) {
            hours
        } else {
            1
        };
        // Every hour and weather state apart, in parallel.
        let weight = 1.0 / (hours * WEATHER_STATES) as f64;
        let parts: Vec<Distribution> = (0..hours * WEATHER_STATES)
            .into_par_iter()
            .map(|task| {
                let (slot, state) = (task / WEATHER_STATES, task % WEATHER_STATES);
                let moment = moment(lines, &means, steady[p], (p, slot, state));
                let mut moving = Distribution::empty();
                moving.silent = 1.0;
                for (_, values) in &moment.moving {
                    moving = moving.with_line(values);
                }
                moving.over(moment.floor)
            })
            .collect();
        let mut mixture = Distribution::empty();
        for part in &parts {
            mixture.add_scaled(part, weight);
        }
        mixture
    })
}

/// One hour and weather state of a period: the energy that holds steady (`steady` and the lines
/// that hold their mean, each such line's part by its index) and the lines that move the level,
/// with their values, the events first.
pub(crate) struct Moment {
    pub floor: f64,
    pub holding: Vec<(usize, f64)>,
    pub moving: Vec<(usize, Vec<(f64, f64)>)>,
}

/// The moment of `lines` (`means` their state means) in the `slot`th hour of `period` and weather
/// `state`.
pub(crate) fn moment(
    lines: &[Line],
    means: &[[f64; WEATHER_STATES]],
    steady: f64,
    (period, slot, state): (usize, usize, usize),
) -> Moment {
    let mut moment = Moment {
        floor: steady,
        holding: Vec::new(),
        moving: Vec::new(),
    };
    let mut held = Vec::new();
    for (index, (line, means)) in lines.iter().zip(means).enumerate() {
        if line.energy[period] <= 0.0 {
            continue;
        }
        let factor = line
            .profile
            .map_or(1.0, |profile| hour_factor(profile, period, slot));
        let mean = means[state] * factor;
        let lambda = line.lambda[period] * factor;
        match line.duty.map(|duty| duty[period]) {
            Some(duty) if duty > 0.0 => moment
                .moving
                .push((index, line_values(mean, 0.0, Some(duty)))),
            Some(_) => {}
            None if lambda >= HOLDS_MEAN_LAMBDA => {
                moment.floor += mean;
                moment.holding.push((index, mean));
            }
            None => held.push((index, mean, lambda)),
        }
    }
    // A line moves the level unless its level exceeded a thousandth of the time stays a hundredth
    // of the steady energy: then it holds its mean in it.
    let steady_energy = moment.floor + held.iter().map(|(_, mean, _)| mean).sum::<f64>();
    for (index, mean, lambda) in held {
        if mean * relative_intensity(lambda, 1.0 - HELD_EXCEEDED) < HELD_SHARE * steady_energy {
            moment.floor += mean;
            moment.holding.push((index, mean));
        } else {
            moment.moving.push((index, line_values(mean, lambda, None)));
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
