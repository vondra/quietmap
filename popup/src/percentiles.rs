//! The levels exceeded a share of the time per period (L10, L50, L90): every contributor a line of
//! emitters of Kurze's statistics (`physics::percentile`) at its own lambda = (emitters per metre)
//! x (its distance): vehicles on a road (the daily flow over the period's share and hours, over
//! the speed), trains on a track (the period's trains over its hours, over the speed), airport
//! movements on an aeroway (at taxi speed); the flights heard one more line at the boxes'
//! energy-weighted lambda; industry, buildings, ships and the unlisted remainder steady at their
//! mean. The sum's distribution is drawn by a simulation seeded by the click, each line's draws
//! stratified (one in each 1/DRAWS of its probability, in a shuffled order) and the lines taken in
//! a fixed order, so that a click gives the same levels every time and the loudest line's own
//! quantiles come out nearly exact.

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

/// The levels (dB, `-inf` silent) exceeded 10, 50 and 90 % of the time per period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub l10: [f64; PERIODS],
    pub l50: [f64; PERIODS],
    pub l90: [f64; PERIODS],
}

/// One fluctuating source: its mean energy and lambda per period.
struct Line {
    key: u64,
    energy: [f64; PERIODS],
    lambda: [f64; PERIODS],
}

fn number(fields: &serde_json::Value, name: &str) -> Option<f64> {
    fields.get(name)?.as_f64()
}

/// Emitters per metre of a contributor per period from its display fields, or `None` for a steady
/// source (or one whose fields say nothing).
fn emitters_per_metre(layer: Layer, fields: &serde_json::Value) -> Option<[f64; PERIODS]> {
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
                std::array::from_fn(|p| daily * shares[p] / (PERIOD_HOURS[p] * 3_600.0) / speed)
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
                std::array::from_fn(|p| {
                    let trains: f64 = names[p]
                        .iter()
                        .filter_map(|name| number(fields, name))
                        .sum();
                    trains / (PERIOD_HOURS[p] * 3_600.0) / speed
                })
            })
        }
        Layer::Aircraft => {
            let daily: f64 = ["arrivals_per_day", "departures_per_day"]
                .iter()
                .filter_map(|name| number(fields, name))
                .sum();
            (daily > 0.0).then(|| {
                std::array::from_fn(|p| {
                    daily * OTHER_PERIOD_SHARES[p]
                        / (PERIOD_HOURS[p] * 3_600.0)
                        / MOVEMENT_SPEED_M_S
                })
            })
        }
        Layer::Industry | Layer::Building | Layer::Ship => None,
    }
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
            if !significant
                || matches!(
                    selection.layer,
                    Layer::Industry | Layer::Building | Layer::Ship
                )
            {
                continue;
            }
            let Some(density) =
                fields(contributor).and_then(|f| emitters_per_metre(contributor.layer, &f))
            else {
                continue;
            };
            let distance = contributor.distance_m.max(DISTANCE_MIN_M);
            lines.push(Line {
                key: contributor.group_key,
                energy: contributor.energy,
                lambda: density.map(|rho| rho * distance),
            });
        }
    }
    // The contributors come from hash maps: a fixed order keeps the draws of a click the same.
    lines.sort_by_key(|line| line.key);
    lines.push(Line {
        key: u64::MAX,
        energy: flight_energy,
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
        l10: [f64::NEG_INFINITY; PERIODS],
        l50: [f64::NEG_INFINITY; PERIODS],
        l90: [f64::NEG_INFINITY; PERIODS],
    };
    for (p, period_total) in total.iter().enumerate() {
        let fluctuating: f64 = lines.iter().map(|line| line.energy[p]).sum();
        let steady = (period_total - fluctuating).max(0.0);
        let mut draws = vec![steady; DRAWS];
        let mut strata: Vec<usize> = (0..DRAWS).collect();
        for line in lines.iter().filter(|line| line.energy[p] > 0.0) {
            // Fisher-Yates: which stratum of this line's probability each draw takes.
            for k in (1..DRAWS).rev() {
                let j = (random.uniform() * (k + 1) as f64) as usize;
                strata.swap(k, j.min(k));
            }
            for (draw, stratum) in draws.iter_mut().zip(&strata) {
                let probability = (*stratum as f64 + random.uniform()) / DRAWS as f64;
                *draw += line.energy[p] * relative_intensity(line.lambda[p], probability);
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
            distance_m,
            display: DisplayRef {
                ring: 0,
                tile: 0,
                attribute: key as u32,
            },
            pieces: Vec::new(),
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
        assert_eq!(roads(&[1, 2, 3, 4, 5]), roads(&[5, 3, 1, 4, 2]));
    }
}
