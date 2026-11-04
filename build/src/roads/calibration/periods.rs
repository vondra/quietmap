//! Complete station-days determine period fractions, with measured global fallback for unsupported country cells.

use super::{Parameters, country_row, weighted_median, write};
use crate::roads::observation::Observation;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

struct Station {
    country: u16,
    class: u8,
    days: u32,
    all: [f64; 24],
    heavy: Option<[f64; 24]>,
    intervals: Vec<(String, String)>,
    weekday_days: u32,
    weekday_vehicles: f64,
}

#[derive(Debug)]
pub struct PeriodModel {
    pub sample_counts: Value,
    pub weekday_factor: Option<f64>,
}

pub fn fit(rows: &[Observation], parameters: &Parameters, directory: &Path) -> Result<PeriodModel> {
    let mut stations = BTreeMap::<(u16, u16, String), Station>::new();
    for row in rows.iter().filter(|r| r.measured) {
        let (Some(class), Some(hours)) = (row.sample.road_class, &row.sample.hourly) else {
            continue;
        };
        let station = stations
            .entry((row.country, row.source_id, row.identity.clone()))
            .or_insert(Station {
                country: row.country,
                class,
                days: 0,
                all: [0.0; 24],
                heavy: hours.heavy_vehicles.map(|_| [0.0; 24]),
                intervals: Vec::new(),
                weekday_days: 0,
                weekday_vehicles: 0.0,
            });
        ensure!(
            station.class == class && station.heavy.is_some() == hours.heavy_vehicles.is_some(),
            "inconsistent hourly station classes"
        );
        ensure!(
            station
                .intervals
                .iter()
                .all(|(first, last)| row.observed_end < *first || row.observed_start > *last),
            "overlapping hourly station intervals"
        );
        station
            .intervals
            .push((row.observed_start.clone(), row.observed_end.clone()));
        station.days += hours.complete_days;
        station.weekday_days += hours.weekday_days;
        station.weekday_vehicles += hours.weekday_vehicles;
        for hour in 0..24 {
            station.all[hour] += hours.all_vehicles[hour];
            if let (Some(total), Some(heavy)) = (&mut station.heavy, &hours.heavy_vehicles) {
                total[hour] += heavy[hour];
            }
        }
    }
    let mut samples = BTreeMap::<(u16, usize, usize), Vec<[f64; 3]>>::new();
    let mut weekday_ratios = Vec::new();
    let mut countries = BTreeSet::from([0]);
    for station in stations
        .values()
        .filter(|s| s.days >= parameters.minimum_hourly_days)
    {
        if station.weekday_days > 0
            && station.weekday_days < station.days
            && station.weekday_vehicles > 0.0
        {
            weekday_ratios.push((
                station.all.iter().sum::<f64>()
                    / f64::from(station.days)
                    / (station.weekday_vehicles / f64::from(station.weekday_days)),
                1.0,
            ));
        }
        let group = match station.class {
            0 | 10 => 0,
            1 | 2 | 11 | 12 => 1,
            _ => 2,
        };
        countries.insert(station.country);
        for vehicle in 0..2 {
            let hourly = if vehicle == 1 {
                station.heavy
            } else {
                Some(std::array::from_fn(|h| {
                    station.all[h] - station.heavy.map_or(0.0, |v| v[h])
                }))
            };
            let Some(hourly) = hourly else {
                continue;
            };
            let total = hourly.iter().sum::<f64>();
            if total <= 0.0 {
                continue;
            }
            let mut shares = [0.0; 3];
            for (hour, count) in hourly.into_iter().enumerate() {
                shares[parameters.period_by_hour[hour]] += count / total;
            }
            for country in [0, station.country] {
                samples
                    .entry((country, group, vehicle))
                    .or_default()
                    .push(shares);
            }
        }
    }
    let counts = samples
        .iter()
        .map(|(k, v)| (format!("{}/{}/{}", k.0, k.1, k.2), v.len()))
        .collect::<BTreeMap<_, _>>();
    let mut medians = BTreeMap::new();
    for (key, values) in &samples {
        if values.len() < parameters.minimum_hourly_stations {
            continue;
        }
        let mut shares: [f64; 3] = std::array::from_fn(|period| {
            weighted_median(&mut values.iter().map(|v| (v[period], 1.0)).collect::<Vec<_>>())
                .unwrap()
        });
        let total = shares.iter().sum::<f64>();
        shares.iter_mut().for_each(|s| *s /= total);
        medians.insert(*key, shares);
    }
    let mut output = Vec::new();
    for country in countries {
        let mut values = [[[0.0; 3]; 2]; 3];
        for (group, vehicles) in values.iter_mut().enumerate() {
            for (vehicle, shares) in vehicles.iter_mut().enumerate() {
                *shares = *medians
                    .get(&(country, group, vehicle))
                    .or_else(|| medians.get(&(0, group, vehicle)))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no observed global period model for group {group}, vehicle {vehicle}"
                        )
                    })?;
            }
        }
        output.push(country_row(country, json!(values)));
    }
    write(directory, "road-period-shares", &json!({"rows":output}))?;
    let weekday_factor = if weekday_ratios.len() >= parameters.minimum_hourly_stations {
        let stations = weekday_ratios.len();
        let factor = weighted_median(&mut weekday_ratios);
        write(
            directory,
            "road-weekday-factor",
            &json!({"factor":factor,"stations":stations}),
        )?;
        factor
    } else {
        None
    };
    Ok(PeriodModel {
        sample_counts: json!(counts),
        weekday_factor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roads::{observation_sample::Hourly, tests::observation};

    #[test]
    fn complete_days_fit_the_weekday_conversion_and_overlapping_station_intervals_fail() {
        let parameters = Parameters {
            minimum_class_samples: 1,
            minimum_hourly_days: 60,
            minimum_hourly_stations: 1,
            period_by_hour: std::array::from_fn(|h| {
                if (7..19).contains(&h) {
                    0
                } else if (19..23).contains(&h) {
                    1
                } else {
                    2
                }
            }),
            preparation: None,
        };
        let mut rows = Vec::new();
        for class in [0, 1, 3] {
            let mut row = observation();
            row.country = u16::from_le_bytes(*b"US");
            row.identity = class.to_string();
            row.sample.road_class = Some(class);
            row.sample.hourly = Some(Hourly {
                complete_days: 70,
                all_vehicles: [100.0; 24],
                heavy_vehicles: Some([10.0; 24]),
                weekday_days: 50,
                weekday_vehicles: 2000.0,
            });
            rows.push(row);
        }
        let directory = tempfile::tempdir().unwrap();
        fit(&rows, &parameters, directory.path()).unwrap();
        let result: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("road-weekday-factor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            result["factor"].as_f64().unwrap(),
            (2400.0 / 70.0) / (2000.0 / 50.0)
        );
        assert_eq!(result["stations"], 3);
        rows.push(rows[0].clone());
        assert!(
            fit(&rows, &parameters, directory.path())
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
    }
}
