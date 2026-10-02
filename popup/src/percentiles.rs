//! The levels exceeded a share of the time per period (L5, L10, L50, L90): every contributor a line of
//! emitters of Kurze's statistics (`physics::percentile`) at its own lambda = (emitters per metre)
//! x (its distance): vehicles on a road (the daily flow over the period's share and hours, over
//! the speed), trains on a track (the period's trains over its hours, over the speed), airport
//! movements on an aeroway (at taxi speed); the flights heard one more line at the boxes'
//! energy-weighted lambda; industry, buildings, ships and the unlisted remainder steady at their
//! mean. A road's flow follows the hours of the day within each period (measured hourly profiles:
//! a night's 3 am carries a fifth of its mean), all roads the same hour of a draw. The sum's
//! distribution is drawn by a simulation seeded by the click, each line's draws stratified (one in
//! each 1/DRAWS of its probability, in a shuffled order) and the lines taken in a fixed order, so
//! that a click gives the same levels every time and the loudest line's own quantiles come out
//! nearly exact. The same traffic tells how a contributor is heard: its passes per hour, and
//! whether at its distance they run together into a steady sound.

use crate::selection::LayerSelection;
use physics::bands::{PERIOD_HOURS, PERIODS};
use physics::percentile::{Random, relative_intensity};
use tiles::sources::Layer;

/// Draws of the sum's distribution.
const DRAWS: usize = 2_000;
/// Contributors under this share of their period's energy count as steady (their fluctuation does
/// not move a percentile of the sum).
const FLUCTUATING_SHARE_MIN: f64 = 1e-4;
/// Day, evening and night shares of a road's daily flow: motorways, trunks and their links; other
/// roads (the sources builder's).
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

/// One source whose level varies in time: its mean energy, its energy in each meteorological
/// state (homogeneous, favourable) and lambda per period (infinite for a steady source, whose
/// level follows the weather alone).
struct Line {
    key: u64,
    energy: [f64; PERIODS],
    states: [[f64; PERIODS]; 2],
    lambda: [f64; PERIODS],
    /// A road's hourly profile ([`HOURLY_SHARES`]); `None` keeps the period's mean every hour.
    profile: Option<usize>,
}

impl Line {
    /// The share of the time the source is heard in its favourable state, from its mean being
    /// the mix of the two.
    fn favourable_share(&self, period: usize) -> f64 {
        let [homogeneous, favourable] = [self.states[0][period], self.states[1][period]];
        if (favourable - homogeneous).abs() <= f64::EPSILON * favourable.abs() {
            return 0.0;
        }
        ((self.energy[period] - homogeneous) / (favourable - homogeneous)).clamp(0.0, 1.0)
    }
}

fn number(fields: &serde_json::Value, name: &str) -> Option<f64> {
    fields.get(name)?.as_f64()
}

/// A contributor's passes per hour by day, evening and night and their speed (m/s) from its
/// display fields: vehicles on a road (the daily flow over the period's share and hours), trains on
/// a track (the period's trains over its hours), airport movements on an aeroway (at taxi speed);
/// `None` for a steady source (or one whose fields say nothing).
fn traffic(layer: Layer, fields: &serde_json::Value) -> Option<([f64; PERIODS], f64)> {
    traffic_and_profile(layer, fields).map(|(per_hour, speed, _)| (per_hour, speed))
}

/// [`traffic`] and, for a road, its hourly profile by class.
fn traffic_and_profile(
    layer: Layer,
    fields: &serde_json::Value,
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
            let shares = if motorway {
                MOTORWAY_PERIOD_SHARES
            } else {
                OTHER_PERIOD_SHARES
            };
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
            let daily: f64 = ["arrivals_per_day", "departures_per_day"]
                .iter()
                .filter_map(|name| number(fields, name))
                .sum();
            (daily > 0.0).then(|| {
                (
                    std::array::from_fn(|p| daily * OTHER_PERIOD_SHARES[p] / PERIOD_HOURS[p]),
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

/// How `contributor` is heard, from its display fields; `None` for a steady source.
pub fn heard(
    contributor: &crate::update::Contributor,
    fields: &serde_json::Value,
) -> Option<Heard> {
    let (per_hour, speed) = traffic(contributor.layer, fields)?;
    Some(Heard {
        per_hour,
        steady: lambda(per_hour, speed, contributor.distance_m)[0] >= STEADY_LAMBDA,
    })
}

/// Exceeded 5 % of the time: what the loud moments of a source are made of.
const LOUD_EXCEEDED: f64 = 0.05;

/// A source's energy exceeded 5 % of the time by itself, per period: its mean energy where it is
/// steady (industry, buildings, ships, or no traffic in its fields), else its line's L5 at its
/// lambda. A car every few hours by the window weighs little; the flights of an approach a lot.
pub fn loud_energy(
    contributor: &crate::update::Contributor,
    fields: Option<&serde_json::Value>,
) -> [f64; PERIODS] {
    let steady = matches!(
        contributor.layer,
        Layer::Industry | Layer::Building | Layer::Ship
    );
    match fields
        .filter(|_| !steady)
        .and_then(|fields| traffic(contributor.layer, fields))
    {
        Some((per_hour, speed)) => {
            let lambda = lambda(per_hour, speed, contributor.distance_m);
            std::array::from_fn(|p| {
                contributor.energy[p] * relative_intensity(lambda[p], 1.0 - LOUD_EXCEEDED)
            })
        }
        None => contributor.energy,
    }
}

/// The flights' energy exceeded 5 % of the time, from their energy and energy times lambda.
pub fn loud_flight_energy(
    (energy, energy_lambda): ([f64; PERIODS], [f64; PERIODS]),
) -> [f64; PERIODS] {
    std::array::from_fn(|p| {
        if energy[p] > 0.0 {
            energy[p] * relative_intensity(energy_lambda[p] / energy[p], 1.0 - LOUD_EXCEEDED)
        } else {
            0.0
        }
    })
}

/// The percentile levels of an answer: `selections` with their contributors, the flights' energy
/// and energy times lambda per period, `fields` a contributor's display fields, `seed` the click's.
pub fn percentiles(
    selections: &[LayerSelection],
    (flight_energy, flight_energy_lambda): ([f64; PERIODS], [f64; PERIODS]),
    fields: &dyn Fn(&crate::update::Contributor) -> Option<serde_json::Value>,
    seed: u64,
) -> Percentiles {
    let total: [f64; PERIODS] =
        std::array::from_fn(|p| selections.iter().map(|s| s.answer_energy()[p]).sum::<f64>());
    let mut lines = Vec::new();
    for selection in selections {
        for contributor in selection.contributors.values() {
            let significant =
                (0..PERIODS).any(|p| contributor.energy[p] > FLUCTUATING_SHARE_MIN * total[p]);
            if !significant {
                continue;
            }
            // Industry, buildings and ships are steady; their level still follows the weather.
            let (lambda, profile) = if matches!(
                selection.layer,
                Layer::Industry | Layer::Building | Layer::Ship
            ) {
                ([f64::INFINITY; PERIODS], None)
            } else {
                let Some((per_hour, speed, profile)) =
                    fields(contributor).and_then(|f| traffic_and_profile(contributor.layer, &f))
                else {
                    continue;
                };
                (lambda(per_hour, speed, contributor.distance_m), profile)
            };
            lines.push(Line {
                key: contributor.group_key,
                energy: contributor.energy,
                states: contributor.states,
                lambda,
                profile,
            });
        }
    }
    // The contributors come from hash maps: a fixed order keeps the draws of a click the same.
    lines.sort_by_key(|line| line.key);
    lines.push(Line {
        key: u64::MAX,
        energy: flight_energy,
        states: [flight_energy; 2],
        lambda: std::array::from_fn(|p| {
            if flight_energy[p] > 0.0 {
                flight_energy_lambda[p] / flight_energy[p]
            } else {
                f64::INFINITY
            }
        }),
        profile: None,
    });
    let mut random = Random::new(seed);
    let mut result = Percentiles {
        l5: [f64::NEG_INFINITY; PERIODS],
        l10: [f64::NEG_INFINITY; PERIODS],
        l50: [f64::NEG_INFINITY; PERIODS],
        l90: [f64::NEG_INFINITY; PERIODS],
    };
    for (p, period_total) in total.iter().enumerate() {
        let fluctuating: f64 = lines.iter().map(|line| line.energy[p]).sum();
        let steady = (period_total - fluctuating).max(0.0);
        let mut draws = vec![steady; DRAWS];
        let mut strata: Vec<usize> = (0..DRAWS).collect();
        // The weather of each draw, one for all sources (a night's inversion or a wind bends
        // every ray of the place alike): stratified too.
        let shuffle = |strata: &mut Vec<usize>, random: &mut Random| {
            for k in (1..DRAWS).rev() {
                let j = (random.uniform() * (k + 1) as f64) as usize;
                strata.swap(k, j.min(k));
            }
        };
        shuffle(&mut strata, &mut random);
        let weather: Vec<f64> = strata
            .iter()
            .map(|&stratum| (stratum as f64 + random.uniform()) / DRAWS as f64)
            .collect();
        // The hour of each draw, one for all roads (the day's rhythm moves them together).
        let hours = PERIOD_HOURS[p] as usize;
        shuffle(&mut strata, &mut random);
        let slots: Vec<usize> = strata
            .iter()
            .map(|&stratum| (stratum * hours / DRAWS).min(hours - 1))
            .collect();
        for line in lines.iter().filter(|line| line.energy[p] > 0.0) {
            // Fisher-Yates: which stratum of this line's probability each draw takes.
            shuffle(&mut strata, &mut random);
            let favourable = line.favourable_share(p);
            for (k, stratum) in strata.iter().enumerate() {
                let probability = (*stratum as f64 + random.uniform()) / DRAWS as f64;
                let state = usize::from(weather[k] < favourable);
                let mean = if line.states[state][p] > 0.0 || line.states[1 - state][p] > 0.0 {
                    line.states[state][p]
                } else {
                    line.energy[p]
                };
                let factor = line
                    .profile
                    .map_or(1.0, |profile| hour_factor(profile, p, slots[k]));
                draws[k] +=
                    mean * factor * relative_intensity(line.lambda[p] * factor, probability);
            }
        }
        draws.sort_by(f64::total_cmp);
        let level = |exceeded: f64| {
            let value = draws[((1.0 - exceeded) * (DRAWS - 1) as f64).round() as usize];
            if value > 0.0 {
                10.0 * value.log10()
            } else {
                f64::NEG_INFINITY
            }
        };
        result.l5[p] = level(0.05);
        result.l10[p] = level(0.1);
        result.l50[p] = level(0.5);
        result.l90[p] = level(0.9);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::DisplayRef;
    use crate::update::Contributor;
    use physics::bands::energy;
    use physics::percentile::{exceeded_level_db, relative_intensity};

    fn contributor(key: u64, layer: Layer, leq_db: f64, distance_m: f64) -> Contributor {
        Contributor {
            group_key: key,
            layer,
            energy: [energy(leq_db); PERIODS],
            states: [[energy(leq_db); PERIODS]; 2],
            distance_m,
            display: DisplayRef {
                ring: 0,
                tile: 0,
                attribute: key as u32,
            },
            pieces: Vec::new(),
            lines: Vec::new(),
            heard: None,
            loud: None,
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

    /// One sparse road alone: its levels are the quantiles of its line over the period's hours,
    /// each hour at its share of the day's vehicles (the stratified draws make them nearly
    /// exact), far below its Leq most of the time.
    #[test]
    fn a_lone_sparse_road_has_its_lines_own_levels_over_the_hours() {
        let road = selection(Layer::Road, vec![contributor(7, Layer::Road, 40.0, 4.0)]);
        let levels = percentiles(&[road], ([0.0; PERIODS], [0.0; PERIODS]), &quiet_road, 1);
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
        let levels = percentiles(&[road], ([0.0; PERIODS], [0.0; PERIODS]), &busy, 4);
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

    /// The loud moments of a source by itself: 33 cars a day 4 m away are there for under 5 % of
    /// the time and weigh a fifth of their mean; industry keeps its mean; passes that run together
    /// keep about theirs; flights a few times an hour weigh several times theirs.
    #[test]
    fn loud_moments_weigh_rare_passes_little_and_frequent_flights_much() {
        let lane = contributor(7, Layer::Road, 40.0, 4.0);
        let rare = loud_energy(&lane, quiet_road(&lane).as_ref())[0] / energy(40.0);
        assert!(rare < 0.4, "{rare}");
        let industry = contributor(9, Layer::Industry, 45.0, 30.0);
        assert_eq!(loud_energy(&industry, None), industry.energy);
        let motorway = contributor(8, Layer::Road, 45.0, 300.0);
        let busy = serde_json::json!({"aadt_light": 30_000.0, "speed_kmh": 100.0,
            "road_class": "motorway"});
        let steady = loud_energy(&motorway, Some(&busy))[0] / energy(45.0);
        assert!((1.0..1.5).contains(&steady), "{steady}");
        let flights = loud_flight_energy(([1.0; PERIODS], [0.05; PERIODS]))[0];
        assert!(flights > 3.0, "{flights}");
    }

    /// Industry is steady; the same click gives the same levels whatever order the hash maps
    /// hold the contributors in.
    #[test]
    fn steady_sources_hold_their_mean_and_a_click_repeats() {
        let industry = selection(
            Layer::Industry,
            vec![contributor(1, Layer::Industry, 45.0, 300.0)],
        );
        let levels = percentiles(
            &[industry],
            ([0.0; PERIODS], [0.0; PERIODS]),
            &quiet_road,
            2,
        );
        for level in [levels.l10[0], levels.l50[0], levels.l90[0]] {
            assert!((level - 45.0).abs() < 1e-9);
        }
        let roads = |order: &[u64]| {
            let contributors = order
                .iter()
                .map(|&key| contributor(key, Layer::Road, 30.0 + key as f64, 5.0 * key as f64))
                .collect();
            percentiles(
                &[selection(Layer::Road, contributors)],
                ([0.0; PERIODS], [0.0; PERIODS]),
                &quiet_road,
                3,
            )
        };
        let (first, second) = (roads(&[1, 2, 3, 4, 5]), roads(&[5, 3, 1, 4, 2]));
        assert_eq!(
            (first.l10, first.l50, first.l90),
            (second.l10, second.l50, second.l90)
        );
    }
}
