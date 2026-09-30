//! The levels exceeded a share of the time per period (L10, L50, L90): every contributor a line of
//! emitters of Kurze's statistics (`physics::percentile`) at its own lambda = (emitters per metre)
//! x (its distance): vehicles on a road (the daily flow over the period's share and hours, over
//! the speed), trains on a track (the period's trains over its hours, over the speed), airport
//! movements on an aeroway (at taxi speed); the flights heard one more line at the boxes'
//! energy-weighted lambda; industry, buildings, ships and the unlisted remainder steady at their
//! mean. The sum's distribution is drawn by a simulation seeded by the click.

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
                energy: contributor.energy,
                lambda: density.map(|rho| rho * distance),
            });
        }
    }
    lines.push(Line {
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
        let mut draws: Vec<f64> = (0..DRAWS)
            .map(|_| {
                steady
                    + lines
                        .iter()
                        .filter(|line| line.energy[p] > 0.0)
                        .map(|line| {
                            line.energy[p] * relative_intensity(line.lambda[p], random.uniform())
                        })
                        .sum::<f64>()
            })
            .collect();
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
