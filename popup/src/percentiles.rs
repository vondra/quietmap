//! The levels exceeded a share of the time per period (L5, L10, L50, L90): every contributor a line of
//! emitters of Kurze's statistics (`physics::percentile`) at its own lambda = (emitters per metre)
//! x (its distance): vehicles on a road (the daily flow over the period's share and hours, over
//! the speed), trains on a track (the period's trains over its hours, over the speed), airport
//! movements on an aeroway (at taxi speed); the flights heard one more line at the boxes'
//! energy-weighted lambda; industry, buildings, ships and the unlisted remainder steady at their
//! mean. The sum's distribution is drawn by a simulation seeded by the click, each line's draws
//! stratified (one in each 1/DRAWS of its probability, in a shuffled order) and the lines taken in
//! a fixed order, so that a click gives the same levels every time and the loudest line's own
//! quantiles come out nearly exact. The same traffic tells how a contributor is heard: its passes
//! per hour, and whether at its distance they run together into a steady sound.

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
            let shares = if matches!(class, "motorway" | "trunk" | "motorway_link" | "trunk_link") {
                MOTORWAY_PERIOD_SHARES
            } else {
                OTHER_PERIOD_SHARES
            };
            (speed > 0.0 && daily > 0.0).then(|| {
                (
                    std::array::from_fn(|p| daily * shares[p] / PERIOD_HOURS[p]),
                    speed,
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
                (per_hour, speed)
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
            let lambda = if matches!(
                selection.layer,
                Layer::Industry | Layer::Building | Layer::Ship
            ) {
                [f64::INFINITY; PERIODS]
            } else {
                let Some((per_hour, speed)) =
                    fields(contributor).and_then(|f| traffic(contributor.layer, &f))
                else {
                    continue;
                };
                lambda(per_hour, speed, contributor.distance_m)
            };
            lines.push(Line {
                key: contributor.group_key,
                energy: contributor.energy,
                states: contributor.states,
                lambda,
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
                draws[k] += mean * relative_intensity(line.lambda[p], probability);
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
    use physics::percentile::exceeded_level_db;

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

    /// One sparse road alone: its levels are the line's own quantiles (the stratified draws make
    /// them nearly exact), far below its Leq most of the time.
    #[test]
    fn a_lone_sparse_road_has_its_lines_own_levels() {
        let road = selection(Layer::Road, vec![contributor(7, Layer::Road, 40.0, 4.0)]);
        let levels = percentiles(&[road], ([0.0; PERIODS], [0.0; PERIODS]), &quiet_road, 1);
        let lambda =
            33.0 * OTHER_PERIOD_SHARES[0] / (PERIOD_HOURS[0] * 3_600.0) / (20.0 / 3.6) * 4.0;
        for (exceeded, level) in [
            (0.1, levels.l10[0]),
            (0.5, levels.l50[0]),
            (0.9, levels.l90[0]),
        ] {
            let expected = exceeded_level_db(40.0, lambda, exceeded);
            assert!(
                (level - expected).abs() < 0.3,
                "{exceeded}: {level} vs {expected}"
            );
        }
        assert!(levels.l50[0] < 25.0 && levels.l10[0] > levels.l50[0]);
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
