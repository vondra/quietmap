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
/// together. Pieces louder in calm air (tree crowns or the ground taking more from the arc) keep
/// bins of their own: summed with the others the states' difference cancels, and the share no
/// longer gives back the mean. A draw's weather is favourable in the bins whose share lies above
/// it.
#[derive(Clone, Default)]
pub struct Weather([[[f64; PERIODS]; 3]; 2 * SHARE_BINS]);

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
            let calm_louder = usize::from(favourable < homogeneous);
            let bin = &mut self.0[calm_louder * SHARE_BINS
                + ((share * SHARE_BINS as f64) as usize).min(SHARE_BINS - 1)];
            bin[0][period] += homogeneous;
            bin[1][period] += favourable;
            bin[2][period] += energy[period];
        }
    }

    /// Each bin's favourable share and its energy of `period` in either state: a draw's weather u
    /// in [0, 1) takes the favourable energy of the bins whose share is above it.
    fn states(&self, period: usize) -> [(f64, f64, f64); 2 * SHARE_BINS] {
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

/// One source whose level varies in time: its mean energy, its means in the weather states and
/// lambda per period (infinite for a steady source, whose level follows the weather alone).
#[derive(Clone)]
pub struct Line {
    /// The contributor's group: the lines are added in its order, so a click repeats exactly.
    pub(crate) key: u64,
    pub energy: [f64; PERIODS],
    /// Per period its mean in each of the [`WEATHER_STATES`] ([`Line::state_means`]).
    means: [[f64; WEATHER_STATES]; PERIODS],
    lambda: [f64; PERIODS],
    /// A road's hourly profile ([`HOURLY_SHARES`]); `None` keeps the period's mean every hour.
    profile: Option<usize>,
    /// An event source's share of each period it sounds: on, it is its mean over the duty; off,
    /// silent.
    duty: Option<[f64; PERIODS]>,
}

/// A source group heard at a place, what its line is made of: its group, layer, energy per period
/// and weather, its nearest piece's distance and where its display fields are read (a click's
/// contributor, or a painted pixel's exactly evaluated sources of one group).
#[derive(Clone, Copy)]
pub struct Group<'a> {
    pub key: u64,
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    pub weather: &'a Weather,
    pub distance_m: f64,
    pub display: DisplayRef,
}

impl crate::update::Contributor {
    pub fn group(&self) -> Group<'_> {
        Group {
            key: self.group_key,
            layer: self.layer,
            energy: self.energy,
            weather: &self.weather,
            distance_m: self.distance_m,
            display: self.display,
        }
    }
}

impl Line {
    /// A group's line: industry, buildings and ships steady but for their events, a road, railway
    /// or aeroway by its traffic, read from its display `fields`; none for one of those whose
    /// fields carry no traffic.
    pub fn of(group: &Group, fields: &serde_json::Value) -> Option<Self> {
        let Group {
            key,
            layer,
            energy,
            weather,
            distance_m,
            ..
        } = *group;
        let steady = matches!(layer, Layer::Industry | Layer::Building | Layer::Ship);
        let (lambda, profile, duty) = if steady {
            ([f64::INFINITY; PERIODS], None, duty(fields))
        } else {
            let (per_hour, speed, profile) = traffic(layer, fields, energy)?;
            (lambda(per_hour, speed, distance_m), profile, None)
        };
        Some(Line {
            key,
            energy,
            means: weather_means(weather, energy),
            lambda,
            profile,
            duty,
        })
    }

    /// The flights heard, one line at their energy-weighted lambda (`weather` theirs: the mean in
    /// either state).
    pub fn flights(
        weather: &Weather,
        (energy, energy_lambda): ([f64; PERIODS], [f64; PERIODS]),
    ) -> Self {
        Line {
            key: u64::MAX,
            energy,
            means: weather_means(weather, energy),
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

    /// The line's mean in each weather state of `period`.
    pub(crate) fn state_means(&self, period: usize) -> [f64; WEATHER_STATES] {
        self.means[period]
    }

    /// The line's intensities in an hour of `period` with `factor` its flow then and the line at
    /// `mean` (its weather state's), each with its share of the time; a line that holds its mean
    /// one value, an event its duty.
    fn values(&self, period: usize, mean: f64, factor: f64) -> LineValues {
        let lambda = self.lambda[period] * factor;
        match self.duty.map(|duty| duty[period]) {
            Some(duty) if duty > 0.0 => line_values(mean * factor, 0.0, Some(duty)),
            Some(_) => LineValues::of(Vec::new()),
            None if lambda >= HOLDS_MEAN_LAMBDA => LineValues::of(vec![(mean * factor, 1.0)]),
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
                let silent = 1.0 - values.pairs().iter().map(|(_, share)| share).sum::<f64>();
                for (value, share) in values.pairs().iter().copied().chain([(0.0, silent)]) {
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

/// Per period the mean of `energy` in `weather` in each weather state: a piece is favourable while
/// the place's weather u lies under its favourable share, so over the state's span of u it is
/// favourable for the part of the span under the share (no step where a share crosses a state's
/// middle).
fn weather_means(weather: &Weather, energy: [f64; PERIODS]) -> [[f64; WEATHER_STATES]; PERIODS] {
    std::array::from_fn(|period| {
        let states = weather.states(period);
        if states
            .iter()
            .all(|&(_, homogeneous, favourable)| homogeneous + favourable <= 0.0)
        {
            return [energy[period]; WEATHER_STATES];
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
    })
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
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let groups = contributors.map(|contributor| contributor.group());
    let (mut lines, steady) = lines(groups, total, REST_SHARE_MIN, fields)?;
    at_mean_flow(&mut lines);
    Ok((lines, steady))
}

/// Every line at its period's mean flow every hour.
pub(crate) fn at_mean_flow(lines: &mut [Line]) {
    for line in lines {
        line.profile = None;
    }
}

/// The lines of `groups` summing to `total` per period: every one with traffic in its fields (or
/// steady but for its events) over `share_min` of its period, in a fixed order; and what is left
/// of each period's energy, steady.
fn lines<'a>(
    groups: impl Iterator<Item = Group<'a>>,
    total: [f64; PERIODS],
    share_min: f64,
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let mut lines = groups
        .filter(|group| (0..PERIODS).any(|p| group.energy[p] > share_min * total[p]))
        .filter_map(|group| (fields(&group).map(|fields| Line::of(&group, &fields))).transpose())
        .collect::<Result<Vec<Line>, String>>()?;
    // The contributors come from hash maps: a fixed order keeps a click's rounding the same.
    lines.sort_by_key(|line| line.key);
    let steady = std::array::from_fn(|p| {
        (total[p] - lines.iter().map(|line| line.energy[p]).sum::<f64>()).max(0.0)
    });
    Ok((lines, steady))
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
    flights: (&'a Weather, ([f64; PERIODS], [f64; PERIODS])),
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let total: [f64; PERIODS] =
        std::array::from_fn(|p| selections.iter().map(|s| s.answer_energy()[p]).sum::<f64>());
    let groups = (selections.iter())
        .flat_map(|selection| selection.contributors.values())
        .map(|contributor| contributor.group());
    place_lines(groups, total, flights, fields)
}

/// The lines of a place heard with `total` energy per period: its `groups` that move the level and
/// the flights heard (`weather` from [`flight_weather`]), and what is left of each period's
/// energy, steady.
pub fn place_lines<'a>(
    groups: impl Iterator<Item = Group<'a>>,
    total: [f64; PERIODS],
    (weather, flights): (&Weather, ([f64; PERIODS], [f64; PERIODS])),
    fields: &dyn Fn(&Group) -> Result<serde_json::Value, String>,
) -> Result<(Vec<Line>, [f64; PERIODS]), String> {
    let (mut lines, mut steady) = lines(groups, total, FLUCTUATING_SHARE_MIN, fields)?;
    let flights_line = Line::flights(weather, flights);
    for (steady, energy) in steady.iter_mut().zip(flights_line.energy) {
        *steady = (*steady - energy).max(0.0);
    }
    lines.push(flights_line);
    Ok((lines, steady))
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
    /// A click's: 0.5 dB, Nden within 0.09 % of 0.1 dB's at 919 points
    /// (`evidence/2026-10-12/sone-map-4`).
    pub const CLICK: Resolution = Resolution {
        bin_db: 0.5,
        weather_states: WEATHER_STATES,
        hour_groups: None,
    };
    /// The map's: 1 dB, three weather states, four, one and three groups of the day's, evening's
    /// and night's hours: Nden within 2.5 % at p99 and 3.9 % at most of the click's 0.1 dB at the
    /// same 919 points, 2.8 ms a point on one core against 787.
    pub const MAP: Resolution = Resolution {
        bin_db: 1.0,
        weather_states: 3,
        hour_groups: Some([4, 1, 3]),
    };
}

/// The distribution of the summed level over each period at `resolution`: for every group of hours
/// (the roads' flows follow the hour together) and every weather state (one for every ray of the
/// place), the lines that move the level added, independent, then the steady energy; the groups
/// and states averaged by their shares of the time.
pub fn distributions_of(
    lines: &[Line],
    steady: [f64; PERIODS],
    resolution: Resolution,
) -> [Distribution; PERIODS] {
    std::array::from_fn(|p| {
        let (groups, hours) = hour_groups(lines, p, resolution.hour_groups);
        let states = resolution.weather_states;
        let means: Vec<Vec<f64>> = (lines.iter())
            .map(|line| merged_states(&line.means[p], states))
            .collect();
        // Every group of hours and weather state apart, in parallel.
        let parts: Vec<(f64, Distribution)> = (0..groups.len() * states)
            .into_par_iter()
            .map(|task| {
                let (slots, state) = (&groups[task / states], task % states);
                let factors: Vec<f64> = (lines.iter())
                    .map(|line| {
                        line.profile.map_or(1.0, |profile| {
                            slots
                                .iter()
                                .map(|&slot| hour_factor(profile, p, slot))
                                .sum::<f64>()
                                / slots.len() as f64
                        })
                    })
                    .collect();
                let state_means: Vec<f64> = means.iter().map(|means| means[state]).collect();
                let moment = moment(lines, (&state_means, &factors), steady[p], p);
                let mut moving = Distribution::empty(resolution.bin_db);
                moving.silent = 1.0;
                for (_, values) in &moment.moving {
                    moving = moving.with_line(values);
                }
                let weight = slots.len() as f64 / (hours * states) as f64;
                (weight, moving.over(moment.floor))
            })
            .collect();
        let mut mixture = Distribution::empty(resolution.bin_db);
        for (weight, part) in &parts {
            mixture.add_scaled(part, *weight);
        }
        mixture
    })
}

/// A period's hours in groups of `groups` (`None`: every hour apart) and how many there are: one
/// where no line follows a profile; else the hours in order of their summed profiles' flow, cut
/// into that many runs.
fn hour_groups(
    lines: &[Line],
    period: usize,
    groups: Option<[usize; PERIODS]>,
) -> (Vec<Vec<usize>>, usize) {
    if lines.iter().all(|line| line.profile.is_none()) {
        return (vec![vec![0]], 1);
    }
    let hours = PERIOD_HOURS[period] as usize;
    let Some(groups) = groups else {
        return ((0..hours).map(|slot| vec![slot]).collect(), hours);
    };
    let flow = |slot: usize| -> f64 {
        (0..HOURLY_SHARES.len())
            .map(|profile| hour_factor(profile, period, slot))
            .sum()
    };
    let mut slots: Vec<usize> = (0..hours).collect();
    slots.sort_by(|&a, &b| flow(a).total_cmp(&flow(b)).then(a.cmp(&b)));
    let runs = groups[period];
    let cut = (0..runs)
        .map(|run| {
            let mut run_slots = slots[run * hours / runs..(run + 1) * hours / runs].to_vec();
            run_slots.sort_unstable();
            run_slots
        })
        .collect();
    (cut, hours)
}

/// The native weather states' means merged into `states` equally likely ones, each the mean over
/// its span of the native ones it covers.
fn merged_states(means: &[f64; WEATHER_STATES], states: usize) -> Vec<f64> {
    if states == WEATHER_STATES {
        return means.to_vec();
    }
    (0..states)
        .map(|state| {
            let (low, high) = (
                state as f64 / states as f64,
                (state + 1) as f64 / states as f64,
            );
            (0..WEATHER_STATES)
                .map(|native| {
                    let from = native as f64 / WEATHER_STATES as f64;
                    let to = (native + 1) as f64 / WEATHER_STATES as f64;
                    means[native] * (high.min(to) - low.max(from)).max(0.0) * states as f64
                })
                .sum()
        })
        .collect()
}

/// One hour and weather state of a period: the energy that holds steady (`steady` and the lines
/// that hold their mean, each such line's part by its index) and the lines that move the level,
/// with their values, the events first.
pub(crate) struct Moment {
    pub floor: f64,
    pub holding: Vec<(usize, f64)>,
    pub moving: Vec<(usize, LineValues)>,
}

/// The moment of `lines` in `period`: each line's mean in the moment's weather state and its flow
/// then against the period's mean (`state_means`, `factors`).
pub(crate) fn moment(
    lines: &[Line],
    (state_means, factors): (&[f64], &[f64]),
    steady: f64,
    period: usize,
) -> Moment {
    let mut moment = Moment {
        floor: steady,
        holding: Vec::new(),
        moving: Vec::new(),
    };
    let mut held = Vec::new();
    for (index, (line, (&state_mean, &factor))) in lines
        .iter()
        .zip(state_means.iter().zip(factors))
        .enumerate()
    {
        if line.energy[period] <= 0.0 {
            continue;
        }
        let mean = state_mean * factor;
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
