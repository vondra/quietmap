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

use crate::selection::LayerSelection;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::percentile::relative_intensity;
use rayon::prelude::*;
use tiles::sources::Layer;

/// Contributors under this share of their period's energy count as steady (their fluctuation does
/// not move a percentile of the sum).
const FLUCTUATING_SHARE_MIN: f64 = 1e-4;
/// The same for the list's last row, everything it leaves out: a coarser account of a row that
/// names nothing.
pub const REST_SHARE_MIN: f64 = 1e-2;
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
fn favourable_share(mean: f64, homogeneous: f64, favourable: f64) -> f64 {
    if (favourable - homogeneous).abs() <= f64::EPSILON * favourable.abs() {
        return 0.0;
    }
    ((mean - homogeneous) / (favourable - homogeneous)).clamp(0.0, 1.0)
}

/// One source whose level varies in time: its mean energy, its weather and lambda per period
/// (infinite for a steady source, whose level follows the weather alone).
pub struct Line<'w> {
    /// The contributor's group: the lines are added in its order, so a click repeats exactly.
    key: u64,
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
    fn state_means(&self, period: usize) -> [f64; WEATHER_STATES] {
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

/// The grid of the level distributions: levels at multiples of [`BIN_DB`] from [`LEVEL_MIN_DB`]
/// (each bin holds the levels rounding to it); a level below the grid counts as silence.
pub const BIN_DB: f64 = 0.1;
const LEVEL_MIN_DB: f64 = -30.0;
const BINS: usize = 1_800;
/// Equally likely weather states of the place, one for every ray of it (a night's inversion bends
/// them alike): in state k a piece is favourable when its share exceeds (k + 0.5) / WEATHER_STATES.
const WEATHER_STATES: usize = 10;
/// A line at least this dense (Kurze's lambda) holds its mean: its level stays within about 0.5 dB.
const HOLDS_MEAN_LAMBDA: f64 = 10.0;
/// A line held at its mean: one whose level exceeded a thousandth of the time stays a hundredth of
/// the steady energy's (20 dB under it), so it moves no level by more than a bin but for moments
/// too rare to count.
const HELD_EXCEEDED: f64 = 1e-3;
const HELD_SHARE: f64 = 1e-2;

/// The probabilities at which a line's distribution is read, each with the share of the time it
/// stands for: every percent to 0.99 (the levels exceeded 5 to 90 % of the time within one), then
/// evenly in log(1 - p) to 1 - 1e-8, where a sparse line's rare passes lie (a car a day 10 m away
/// is loud for a few seconds of it).
fn nodes() -> &'static [(f64, f64)] {
    static NODES: std::sync::OnceLock<Vec<(f64, f64)>> = std::sync::OnceLock::new();
    NODES.get_or_init(|| {
        const EVEN: usize = 99;
        const TAIL_STEPS: usize = 24;
        let mut nodes: Vec<(f64, f64)> = (0..EVEN)
            .map(|k| ((k as f64 + 0.5) / 100.0, 0.01))
            .collect();
        // -log10(1 - p) from 2 to 8 in steps of a quarter, and the last 1e-8 in one.
        let at = |t: f64| 1.0 - 10f64.powf(-t);
        for k in 0..TAIL_STEPS {
            let (a, b) = (2.0 + k as f64 * 0.25, 2.25 + k as f64 * 0.25);
            nodes.push((at((a + b) / 2.0), at(b) - at(a)));
        }
        nodes.push((1.0 - 0.5e-8, 1e-8));
        nodes
    })
}

/// How the summed level is spread over a period: the share of the time in each bin of the level
/// grid with the mean intensity of its moments (so adding sounds loses no energy to the grid), and
/// the share in silence.
#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    pub silent: f64,
    /// Per bin its share of the time and that share times its moments' mean intensity.
    bins: Vec<[f64; 2]>,
}

impl Distribution {
    fn empty() -> Self {
        Distribution {
            silent: 0.0,
            bins: vec![[0.0; 2]; BINS],
        }
    }

    /// The bin of a level; a level under the grid is held in its lowest bin, at its own intensity.
    fn bin(level_db: f64) -> usize {
        (((level_db - LEVEL_MIN_DB) / BIN_DB).round().max(0.0) as usize).min(BINS - 1)
    }

    /// Each bin holding a share of the time: its moments' mean intensity and its share.
    fn intensities(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.bins
            .iter()
            .filter(|[share, _]| *share > 0.0)
            .map(|&[share, weighted]| (weighted / share, share))
    }

    /// Each level (dB) holding a share of the time, with its share; silence apart.
    pub fn levels(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.intensities()
            .map(|(intensity, share)| (10.0 * intensity.log10(), share))
    }

    /// The level exceeded `exceeded` of the time (dB, `-inf` where silence is).
    pub fn exceeded_db(&self, exceeded: f64) -> f64 {
        let mut above = 0.0;
        for &[share, weighted] in self.bins.iter().rev() {
            above += share;
            if share > 0.0 && above > exceeded * (1.0 - 1e-12) {
                return 10.0 * (weighted / share).log10();
            }
        }
        f64::NEG_INFINITY
    }

    /// A distribution of given levels (dB, `-inf` silent) with their shares of the time.
    #[cfg(test)]
    pub fn of_levels(levels: &[(f64, f64)]) -> Self {
        let mut distribution = Distribution::empty();
        for &(level, share) in levels {
            distribution.add(10f64.powf(level / 10.0), share);
        }
        distribution
    }

    /// The distribution with independent `values` (intensities with their shares of the time)
    /// added at every moment. A sum's bin is found from the two levels (a table); its intensity
    /// is kept exact.
    fn with_line(&self, values: &[(f64, f64)]) -> Self {
        let mut sum = Distribution::empty();
        let levels: Vec<f64> = values
            .iter()
            .map(|&(value, _)| {
                if value > 0.0 {
                    10.0 * value.log10()
                } else {
                    f64::NEG_INFINITY
                }
            })
            .collect();
        for &(value, weight) in values {
            sum.add(value, self.silent * weight);
        }
        for (intensity, share) in self.intensities() {
            let level = 10.0 * intensity.log10();
            for (&(value, weight), &value_level) in values.iter().zip(&levels) {
                let bin = Self::bin(power_sum_db(level, value_level));
                sum.bins[bin][0] += share * weight;
                sum.bins[bin][1] += share * weight * (intensity + value);
            }
        }
        sum
    }

    /// The distribution with a steady `intensity` added at every moment.
    fn over(&self, intensity: f64) -> Self {
        self.with_line(&[(intensity, 1.0)])
    }

    /// Adds `other`'s shares times `weight`.
    fn add_scaled(&mut self, other: &Distribution, weight: f64) {
        self.silent += other.silent * weight;
        for (into, from) in self.bins.iter_mut().zip(&other.bins) {
            into[0] += from[0] * weight;
            into[1] += from[1] * weight;
        }
    }

    fn add(&mut self, intensity: f64, share: f64) {
        if intensity > 0.0 {
            let bin = Self::bin(10.0 * intensity.log10());
            self.bins[bin][0] += share;
            self.bins[bin][1] += share * intensity;
        } else {
            self.silent += share;
        }
    }
}

/// The level of two levels' summed energy (dB): the louder one and the step the quieter adds,
/// tabled every 0.01 dB of their difference (a silent one adds nothing).
fn power_sum_db(a: f64, b: f64) -> f64 {
    static STEPS: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    const PER_DB: f64 = 100.0;
    const SPAN_DB: f64 = 60.0;
    let steps = STEPS.get_or_init(|| {
        (0..=(SPAN_DB * PER_DB) as usize)
            .map(|k| 10.0 * (1.0 + 10f64.powf(-(k as f64 / PER_DB) / 10.0)).log10())
            .collect()
    });
    let (high, difference) = if a >= b { (a, a - b) } else { (b, b - a) };
    if !difference.is_finite() {
        return high;
    }
    high + steps
        .get((difference * PER_DB).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

/// A line's intensities at the nodes, each with its share of the time: Kurze's quantiles of
/// `mean` at `lambda`, or an event's mean over its duty while it sounds and silence otherwise.
fn line_values(mean: f64, lambda: f64, duty: Option<f64>) -> Vec<(f64, f64)> {
    match duty {
        Some(duty) => vec![(0.0, 1.0 - duty), (mean / duty, duty)],
        None => nodes()
            .iter()
            .map(|&(p, weight)| (mean * relative_intensity(lambda, p), weight))
            .collect(),
    }
}

/// The lines of `contributors` summing to `total` per period: every one with traffic in its fields
/// (or steady but for its events) over `share_min` of its period, in a fixed order; and what is
/// left of each period's energy, steady.
pub fn lines<'a>(
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

/// The distribution of the summed level over each period of a click: every contributor of
/// `selections` and the flights heard ([`distributions_of`]).
pub fn distributions(
    selections: &[LayerSelection],
    flights: ([f64; PERIODS], [f64; PERIODS]),
    fields: &dyn Fn(&crate::update::Contributor) -> Option<serde_json::Value>,
) -> [Distribution; PERIODS] {
    let mut flight_weather = Weather::default();
    flight_weather.add(&flights.0, &[flights.0; 2]);
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
    let flights_line = Line::flights(&flight_weather, flights);
    for (steady, energy) in steady.iter_mut().zip(flights_line.energy) {
        *steady = (*steady - energy).max(0.0);
    }
    lines.push(flights_line);
    distributions_of(&lines, steady)
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
                let mut floor = steady[p];
                let mut moving = Distribution::empty();
                moving.silent = 1.0;
                let mut held = Vec::new();
                for (line, means) in lines.iter().zip(&means) {
                    if line.energy[p] <= 0.0 {
                        continue;
                    }
                    let factor = line
                        .profile
                        .map_or(1.0, |profile| hour_factor(profile, p, slot));
                    let mean = means[state] * factor;
                    let lambda = line.lambda[p] * factor;
                    match line.duty.map(|duty| duty[p]) {
                        Some(duty) if duty > 0.0 => {
                            moving = moving.with_line(&line_values(mean, 0.0, Some(duty)))
                        }
                        Some(_) => {}
                        None if lambda >= HOLDS_MEAN_LAMBDA => floor += mean,
                        None => held.push((mean, lambda)),
                    }
                }
                // A line moves the level unless its level exceeded a thousandth of the time stays
                // a hundredth of the steady energy: then it holds its mean in it.
                let steady_energy = floor + held.iter().map(|(mean, _)| mean).sum::<f64>();
                for (mean, lambda) in held {
                    if mean * relative_intensity(lambda, 1.0 - HELD_EXCEEDED)
                        < HELD_SHARE * steady_energy
                    {
                        floor += mean;
                    } else {
                        moving = moving.with_line(&line_values(mean, lambda, None));
                    }
                }
                moving.over(floor)
            })
            .collect();
        let mut mixture = Distribution::empty();
        for part in &parts {
            mixture.add_scaled(part, weight);
        }
        mixture
    })
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
mod tests {
    use super::*;
    use crate::candidates::DisplayRef;
    use crate::update::Contributor;
    use physics::bands::energy;
    use physics::percentile::{exceeded_level_db, relative_intensity};

    /// The time levels of `selections` with no flights and `fields` the contributors' displays.
    fn levels(
        selections: &[LayerSelection],
        fields: &dyn Fn(&Contributor) -> Option<serde_json::Value>,
    ) -> Percentiles {
        Percentiles::of(&distributions(
            selections,
            ([0.0; PERIODS], [0.0; PERIODS]),
            fields,
        ))
    }

    fn contributor(key: u64, layer: Layer, leq_db: f64, distance_m: f64) -> Contributor {
        Contributor {
            group_key: key,
            layer,
            energy: [energy(leq_db); PERIODS],
            weather: {
                let mut weather = Weather::default();
                weather.add(&[energy(leq_db); PERIODS], &[[energy(leq_db); PERIODS]; 2]);
                weather
            },
            distance_m,
            display: DisplayRef {
                ring: 0,
                tile: 0,
                attribute: key as u32,
            },
            pieces: Vec::new(),
            lines: Vec::new(),
            heard: None,
            nden_sone: None,
        }
    }

    fn selection(layer: Layer, contributors: Vec<Contributor>) -> LayerSelection {
        let mut selection = LayerSelection::new(layer);
        for contributor in contributors {
            for p in 0..PERIODS {
                selection.energy[p] += contributor.energy[p];
            }
            selection
                .contributors
                .insert(contributor.group_key, contributor);
        }
        selection
    }

    /// 33 vehicles a day at 20 km/h, as the service road by the owner's hotel.
    fn quiet_road(_: &Contributor) -> Option<serde_json::Value> {
        Some(
            serde_json::json!({"aadt_light": 31.0, "aadt_heavy": 1.0, "aadt_moto": 1.0,
            "speed_kmh": 20.0, "road_class": "service"}),
        )
    }

    /// A road runs at the day, evening and night shares its fields carry (its country's), and at
    /// the fallback where sources built before them carry none.
    #[test]
    fn a_road_runs_at_the_period_shares_its_fields_carry() {
        let thai = serde_json::json!({"aadt_light": 800.0, "speed_kmh": 50.0,
            "road_class": "primary", "period_shares": [0.632, 0.182, 0.186]});
        let (per_hour, _, _) = traffic(Layer::Road, &thai, [1.0; PERIODS]).unwrap();
        assert!((per_hour[2] - 800.0 * 0.186 / PERIOD_HOURS[2]).abs() < 1e-9);
        let older = serde_json::json!({"aadt_light": 800.0, "speed_kmh": 50.0,
            "road_class": "primary"});
        let (per_hour, _, _) = traffic(Layer::Road, &older, [1.0; PERIODS]).unwrap();
        assert!((per_hour[2] - 800.0 * OTHER_PERIOD_SHARES[2] / PERIOD_HOURS[2]).abs() < 1e-9);
    }

    /// A local road alone at night is loudest in its morning hour, its flow then well over its
    /// night mean; airport movements run in the periods their energy fell in, ground vehicles
    /// counted.
    #[test]
    fn a_road_follows_its_hours_and_movements_their_periods() {
        let steady = Line {
            key: 1,
            energy: [1.0; PERIODS],
            weather: &Weather::default(),
            lambda: [1e6; PERIODS],
            profile: Some(2),
            duty: None,
        };
        let mut loudest = f64::NEG_INFINITY;
        let mut shares = 0.0;
        steady.own_levels(2, 0.0, &mut |level, share| {
            loudest = loudest.max(level);
            shares += share;
        });
        let peak = (0..8)
            .map(|slot| hour_factor(2, 2, slot))
            .fold(0.0, f64::max);
        assert!(
            peak > 2.0 && (loudest - 10.0 * peak.log10()).abs() < 1e-9,
            "{loudest}"
        );
        assert!((shares - 1.0).abs() < 1e-9);
        let cargo = serde_json::json!({"arrivals_per_day": 4.0, "departures_per_day": 4.0,
            "ground_vehicles_per_day": 2.0});
        let (per_hour, _, _) = traffic(Layer::Aircraft, &cargo, [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(per_hour[0], 0.0);
        assert!((per_hour[2] - 10.0 / 8.0).abs() < 1e-12);
    }

    /// One sparse road alone: its levels are the quantiles of its line over the period's hours,
    /// each hour at its share of the day's vehicles, far below its Leq most of the time.
    #[test]
    fn a_lone_sparse_road_has_its_lines_own_levels_over_the_hours() {
        let road = selection(Layer::Road, vec![contributor(7, Layer::Road, 40.0, 4.0)]);
        let levels = levels(&[road], &quiet_road);
        let lambda =
            33.0 * OTHER_PERIOD_SHARES[0] / (PERIOD_HOURS[0] * 3_600.0) / (20.0 / 3.6) * 4.0;
        let mut pooled: Vec<f64> = (0..12)
            .flat_map(|slot| {
                let factor = hour_factor(2, 0, slot);
                (0..2_000).map(move |k| {
                    energy(40.0)
                        * factor
                        * relative_intensity(lambda * factor, (k as f64 + 0.5) / 2_000.0)
                })
            })
            .collect();
        pooled.sort_by(f64::total_cmp);
        for (exceeded, level) in [
            (0.1, levels.l10[0]),
            (0.5, levels.l50[0]),
            (0.9, levels.l90[0]),
        ] {
            let at = ((1.0 - exceeded) * (pooled.len() - 1) as f64).round() as usize;
            let expected = 10.0 * pooled[at].log10();
            assert!(
                (level - expected).abs() < 0.5,
                "{exceeded}: {level} vs {expected}"
            );
        }
        assert!(levels.l50[0] < 25.0 && levels.l10[0] > levels.l50[0]);
    }

    /// A busy road's passes run together (lambda about 12), yet its 3 am carries a fifth of the
    /// night's mean flow: its night L90 sits several dB under its Leq, where one rate for the
    /// whole night kept it within 1 dB (Madrid's stations read L90 4.5 dB under the model's
    /// beyond its level offset, evidence 2026-10-02).
    #[test]
    fn a_busy_roads_quiet_hours_lower_its_night_l90() {
        let road = selection(
            Layer::Road,
            vec![contributor(7, Layer::Road, 60.0, 2_000.0)],
        );
        let busy = |_: &Contributor| {
            Some(
                serde_json::json!({"aadt_light": 20_000.0, "speed_kmh": 50.0,
                "road_class": "secondary"}),
            )
        };
        let levels = levels(&[road], &busy);
        let constant_l90 = exceeded_level_db(60.0, 12.0, 0.9);
        assert!(constant_l90 > 59.0, "{constant_l90}");
        assert!(levels.l90[2] < 56.0, "{}", levels.l90[2]);
        assert!(levels.l10[2] > 60.5, "{}", levels.l10[2]);
    }

    /// A car every half hour 4 m away is heard as passes about twice an hour by day; a
    /// motorway's 30,000 vehicles 300 m away run together into a steady sound; industry has no
    /// passes.
    #[test]
    fn a_contributor_is_heard_as_its_passes_or_as_a_steady_sound() {
        let lane = contributor(7, Layer::Road, 40.0, 4.0);
        let heard_lane = heard(&lane, &quiet_road(&lane).unwrap()).unwrap();
        assert!((heard_lane.per_hour[0] - 33.0 * 0.70 / 12.0).abs() < 1e-9);
        assert!(!heard_lane.steady);
        let motorway = contributor(8, Layer::Road, 45.0, 300.0);
        let busy = serde_json::json!({"aadt_light": 30_000.0, "speed_kmh": 100.0,
            "road_class": "motorway"});
        let heard_motorway = heard(&motorway, &busy).unwrap();
        assert!((heard_motorway.per_hour[0] - 30_000.0 * 0.65 / 12.0).abs() < 1e-9);
        assert!(heard_motorway.steady);
        let industry = contributor(9, Layer::Industry, 45.0, 30.0);
        assert_eq!(heard(&industry, &busy), None);
    }

    /// Industry is steady; the same click gives the same levels whatever order the hash maps
    /// hold the contributors in.
    #[test]
    fn steady_sources_hold_their_mean_and_a_click_repeats() {
        let industry = selection(
            Layer::Industry,
            vec![contributor(1, Layer::Industry, 45.0, 300.0)],
        );
        let industry_levels = levels(&[industry], &quiet_road);
        for level in [
            industry_levels.l10[0],
            industry_levels.l50[0],
            industry_levels.l90[0],
        ] {
            assert!((level - 45.0).abs() < 1e-9, "{level}");
        }
        let roads = |order: &[u64]| {
            let contributors = order
                .iter()
                .map(|&key| contributor(key, Layer::Road, 30.0 + key as f64, 5.0 * key as f64))
                .collect();
            levels(&[selection(Layer::Road, contributors)], &quiet_road)
        };
        let (first, second) = (roads(&[1, 2, 3, 4, 5]), roads(&[5, 3, 1, 4, 2]));
        assert_eq!(
            (first.l10, first.l50, first.l90),
            (second.l10, second.l50, second.l90)
        );
    }

    /// Adding many lines loses no energy to the grid: 500 identical lines, each soon a small part
    /// of a 0.1 dB step of their sum, keep their summed mean (rounding to the grid lost 1.2 dB).
    #[test]
    fn many_quiet_lines_keep_their_energy() {
        let mut sum = Distribution::empty();
        sum.silent = 1.0;
        let values = line_values(1.0, 0.5, None);
        let mean: f64 = values.iter().map(|(value, weight)| value * weight).sum();
        for _ in 0..500 {
            sum = sum.with_line(&values);
        }
        let total: f64 = sum
            .intensities()
            .map(|(intensity, share)| intensity * share)
            .sum();
        assert!((total / (500.0 * mean) - 1.0).abs() < 1e-9, "{total}");
    }

    /// A site's two pieces on either side of the click, one always downwind (favourable, 10 dB
    /// over calm) and the other never: its level holds at their sum, never all favourable or all
    /// calm together (one share for the whole put L90 7 dB low and L10 3 dB high).
    #[test]
    fn pieces_on_either_side_keep_their_own_weather() {
        let mut site = contributor(1, Layer::Industry, 0.0, 300.0);
        let (calm, favourable) = (1.0, 10.0);
        site.weather = Weather::default();
        site.weather.add(
            &[favourable; PERIODS],
            &[[calm; PERIODS], [favourable; PERIODS]],
        );
        site.weather
            .add(&[calm; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
        site.energy = [favourable + calm; PERIODS];
        let levels = levels(&[selection(Layer::Industry, vec![site])], &|_| None);
        for p in 0..PERIODS {
            for level in [levels.l5[p], levels.l50[p], levels.l90[p]] {
                assert!(
                    (level - 11f64.log10() * 10.0).abs() <= BIN_DB / 2.0,
                    "{level}"
                );
            }
        }
    }

    /// A steady source alone keeps its weather: favourable (50 dB) half the time and calm (40 dB)
    /// the other half, it is at each for half the period, not at their mean.
    #[test]
    fn a_source_alone_keeps_its_weather() {
        let mut plant = contributor(1, Layer::Industry, 0.0, 300.0);
        let (calm, favourable) = (energy(40.0), energy(50.0));
        let mean = 0.5 * (calm + favourable);
        plant.weather = Weather::default();
        plant
            .weather
            .add(&[mean; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
        plant.energy = [mean; PERIODS];
        let line = Line::of(&plant, None).unwrap();
        let mut levels: Vec<(f64, f64)> = Vec::new();
        line.own_levels(0, 0.0, &mut |level, share| levels.push((level, share)));
        let at = |db: f64| -> f64 {
            levels
                .iter()
                .filter(|(level, _)| (level - db).abs() < 1e-9)
                .map(|(_, share)| share)
                .sum()
        };
        assert!(
            (at(40.0) - 0.5).abs() < 1e-9 && (at(50.0) - 0.5).abs() < 1e-9,
            "{levels:?}"
        );
    }

    /// A source's favourable share moves its levels smoothly: 24.9 % and 25.1 % of the time
    /// favourable (10 dB over calm) give nearly the same spread (ten states read at their middles
    /// jumped there by a whole state).
    #[test]
    fn the_weather_moves_the_levels_smoothly() {
        let spread = |share: f64| {
            let mut site = contributor(1, Layer::Industry, 0.0, 300.0);
            let (calm, favourable) = (1.0, 10.0);
            let mean = share * favourable + (1.0 - share) * calm;
            site.weather = Weather::default();
            site.weather
                .add(&[mean; PERIODS], &[[calm; PERIODS], [favourable; PERIODS]]);
            site.energy = [mean; PERIODS];
            distributions(
                &[selection(Layer::Industry, vec![site])],
                ([0.0; PERIODS], [0.0; PERIODS]),
                &|_| None,
            )
        };
        let (below, above) = (spread(0.249), spread(0.251));
        let mean = |distribution: &Distribution| -> f64 {
            distribution
                .levels()
                .map(|(level, share)| share * 10f64.powf(level / 10.0))
                .sum()
        };
        assert!((mean(&above[0]) / mean(&below[0]) - 1.0).abs() < 0.01);
        for exceeded in [0.1, 0.5, 0.9] {
            let step = above[0].exceeded_db(exceeded) - below[0].exceeded_db(exceeded);
            assert!(step.abs() < 0.2, "{exceeded}: {step}");
        }
    }

    /// Church bells sounding 2 % of the day at a mean of 40 dB beside a steady 40 dB: the
    /// percentiles keep the steady 40 (a steady source of the bells' energy would put L50 at 43)
    /// but for the bells' own 2 %, and the bells are heard as their rings a day.
    #[test]
    fn events_sound_for_their_duty_and_are_silent_otherwise() {
        let bells_fields = |c: &Contributor| {
            (c.group_key == 2).then(|| {
                serde_json::json!({"events_per_day": [36.0, 0.0, 3.0], "duty": [0.02, 0.0, 0.01]})
            })
        };
        let plant = contributor(1, Layer::Industry, 40.0, 50.0);
        let bells = contributor(2, Layer::Building, 40.0, 80.0);
        let levels = levels(
            &[
                selection(Layer::Industry, vec![plant]),
                selection(Layer::Building, vec![bells.clone()]),
            ],
            &bells_fields,
        );
        for level in [levels.l5[0], levels.l50[0], levels.l90[0]] {
            assert!((level - 40.0).abs() < 1e-9, "{level}");
        }
        let distribution = &distributions(
            &[
                selection(
                    Layer::Industry,
                    vec![contributor(1, Layer::Industry, 40.0, 50.0)],
                ),
                selection(Layer::Building, vec![bells.clone()]),
            ],
            ([0.0; PERIODS], [0.0; PERIODS]),
            &bells_fields,
        )[0];
        // Ringing, 40 dB over the duty's 2 %: 40 + 10 log10(1 + 50) for 2 % of the day.
        let ringing = 10.0 * 51f64.log10() + 40.0;
        assert!((distribution.exceeded_db(0.019) - ringing).abs() <= BIN_DB / 2.0);
        let fields = bells_fields(&bells).unwrap();
        let heard_bells = heard(&bells, &fields).unwrap();
        assert!(!heard_bells.steady && (heard_bells.per_hour[0] - 3.0).abs() < 1e-9);
    }
}
