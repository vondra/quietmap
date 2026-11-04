//! Traffic and event rates, period shares and measured hourly profiles.

use super::*;

/// Day, evening and night shares of a road's daily flow where its fields carry none (sources built
/// before the shares by country): motorways, trunks and their links; other roads.
const MOTORWAY_PERIOD_SHARES: [f64; PERIODS] = [0.65, 0.20, 0.15];
pub(super) const OTHER_PERIOD_SHARES: [f64; PERIODS] = [0.70, 0.18, 0.12];
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
pub fn hour_factor(profile: usize, period: usize, slot: usize) -> f64 {
    let hours = PERIOD_HOURS[period] as usize;
    let share = |slot: usize| HOURLY_SHARES[profile][(PERIOD_FIRST_HOUR[period] + slot) % 24];
    let mean = (0..hours).map(share).sum::<f64>() / hours as f64;
    share(slot) / mean
}

/// Airport movements on the aeroway lines taxi (their rolls are flights, `crate::aircraft::rolls`)
/// at the taxi emission's reference speed (m/s).
const MOVEMENT_SPEED_M_S: f64 =
    physics::emission::airport::TAXI_REFERENCE_SPEED_KT * 1_852.0 / 3_600.0;
/// Emitters closer than this are at this distance (m): a receiver on the line itself.
const DISTANCE_MIN_M: f64 = 1.0;

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

/// How a source group sounds in time, read once from its display fields: steady but for its events
/// (industry, buildings and ships: the share of each period it sounds), passes (a road's vehicles,
/// a track's trains: per hour by period, their speed in m/s and a road's hourly profile), an
/// aeroway's movements and ground vehicles a day (spread over the periods by where its energy
/// fell), or unknown (a road, railway or aeroway whose fields carry no traffic: no line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Passes {
    Steady(Option<[f64; PERIODS]>),
    Traffic([f64; PERIODS], f64, Option<usize>),
    Movements(f64),
    Unknown,
}

impl Passes {
    /// A `layer` source's passes from its display `fields`: vehicles on a road (the daily flow over
    /// the period's share and hours, its profile by class), trains on a track (the period's trains
    /// over its hours), airport movements and ground vehicles on an aeroway.
    pub fn read(layer: Layer, fields: &serde_json::Value) -> Self {
        let traffic = match layer {
            Layer::Industry | Layer::Building | Layer::Ship => return Passes::Steady(duty(fields)),
            Layer::Road => road_traffic(fields),
            Layer::Railway => railway_traffic(fields),
            Layer::Aircraft => {
                let daily: f64 = [
                    "arrivals_per_day",
                    "departures_per_day",
                    "ground_vehicles_per_day",
                ]
                .iter()
                .filter_map(|name| number(fields, name))
                .sum();
                return if daily > 0.0 {
                    Passes::Movements(daily)
                } else {
                    Passes::Unknown
                };
            }
        };
        traffic.map_or(Passes::Unknown, |(per_hour, speed, profile)| {
            Passes::Traffic(per_hour, speed, profile)
        })
    }

    /// Passes per hour by period, their speed (m/s) and hourly profile for a source of `energy` per
    /// period (an aeroway's movements shared over the periods as its sound exposure, each period's
    /// mean power times its hours, at taxi speed); `None` for a steady source or unknown traffic.
    pub(super) fn traffic(
        &self,
        energy: [f64; PERIODS],
    ) -> Option<([f64; PERIODS], f64, Option<usize>)> {
        match *self {
            Passes::Traffic(per_hour, speed, profile) => Some((per_hour, speed, profile)),
            Passes::Movements(daily) => {
                let exposure: f64 = (0..PERIODS).map(|p| energy[p] * PERIOD_HOURS[p]).sum();
                (exposure > 0.0).then(|| {
                    let per_hour = std::array::from_fn(|p| daily * energy[p] / exposure);
                    (per_hour, MOVEMENT_SPEED_M_S, None)
                })
            }
            Passes::Steady(_) | Passes::Unknown => None,
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

/// A road's vehicles per hour by day, evening and night (its daily flow over the period's share and
/// hours), their speed (m/s) and its hourly profile by class, from its display fields.
fn road_traffic(fields: &serde_json::Value) -> Option<([f64; PERIODS], f64, Option<usize>)> {
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

/// A track's trains per hour by day, evening and night and their speed (m/s), from its display
/// fields.
fn railway_traffic(fields: &serde_json::Value) -> Option<([f64; PERIODS], f64, Option<usize>)> {
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

/// Kurze's lambda per period: the emitters within the contributor's distance of a line of them.
pub(super) fn lambda(per_hour: [f64; PERIODS], speed_m_s: f64, distance_m: f64) -> [f64; PERIODS] {
    per_hour.map(|passes| passes / 3_600.0 / speed_m_s * distance_m.max(DISTANCE_MIN_M))
}

/// How `contributor` is heard, from its display fields: its passes, or its events (church bells
/// ringing), per hour; `None` for a steady source.
pub fn passes_at(
    contributor: &crate::update::Contributor,
    fields: &serde_json::Value,
) -> Option<Heard> {
    if let Some(per_hour) = events_per_hour(fields) {
        return Some(Heard {
            per_hour,
            steady: false,
        });
    }
    let (per_hour, speed, _) =
        Passes::read(contributor.layer, fields).traffic(contributor.energy)?;
    Some(Heard {
        per_hour,
        steady: lambda(per_hour, speed, contributor.distance_m)[0] >= STEADY_LAMBDA,
    })
}

/// A period's hours in groups of `groups` (`None`: every hour apart) and how many there are: one
/// where no line follows a profile (not `profiled`); else the hours in order of their summed
/// profiles' flow, cut into that many runs.
pub fn hour_groups(
    profiled: bool,
    period: usize,
    groups: Option<[usize; PERIODS]>,
) -> (Vec<Vec<usize>>, usize) {
    if !profiled {
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
