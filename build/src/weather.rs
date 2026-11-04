//! Build the native weather table directly from the manifest's completed climatology statistics.

use crate::{era5, year::YearInputs};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::weather::{COLUMNS, NODES_PER_DEGREE, ROWS, SECTORS, WeatherNode, encode};
use std::path::Path;

pub struct Node {
    pub percent: [[u8; SECTORS]; PERIODS],
    pub means: [[f32; BANDS]; PERIODS],
}

impl Node {
    fn from_statistics(statistics: &era5::Statistics, index: usize) -> Result<Self, String> {
        let mut node = Self {
            percent: [[0; SECTORS]; PERIODS],
            means: [[0.0; BANDS]; PERIODS],
        };
        for period in 0..PERIODS {
            let count = statistics.counts.value(index * PERIODS + period);
            if count == 0.0 {
                return Err(format!(
                    "ERA5 node {index}, period {period}: no observations"
                ));
            }
            for sector in 0..SECTORS {
                let favourable = statistics
                    .favourable
                    .value((index * PERIODS + period) * SECTORS + sector);
                if favourable > count {
                    return Err(format!(
                        "ERA5 node {index}: favourable states exceed observations"
                    ));
                }
                node.percent[period][sector] = (100.0 * favourable / count).round_ties_even() as u8;
            }
            for band in 0..BANDS {
                let mean = statistics
                    .means
                    .value((index * PERIODS + period) * BANDS + band)
                    as f32;
                if !mean.is_finite() || mean < 0.0 {
                    return Err(format!("ERA5 node {index}: invalid absorption mean"));
                }
                node.means[period][band] = mean;
            }
        }
        Ok(node)
    }

    fn native(&self) -> WeatherNode {
        WeatherNode {
            percent: self.percent,
            // Round to the established climatology's f32 means before combining the periods.
            alpha_db_per_km: std::array::from_fn(|band| {
                (0..PERIODS)
                    .map(|period| PERIOD_HOURS[period] * f64::from(self.means[period][band]))
                    .sum::<f64>()
                    / PERIOD_HOURS.iter().sum::<f64>()
            }),
        }
    }
}

pub fn build(inputs: &YearInputs, out: &Path) -> Result<(), String> {
    let statistics = era5::Statistics::read(inputs)?;
    statistics
        .counts
        .require("<u4", &[era5::ROWS * era5::COLUMNS, PERIODS])?;
    let step = era5::DENSITY / NODES_PER_DEGREE;
    let mut nodes = Vec::with_capacity(ROWS * COLUMNS);
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            nodes.push(
                Node::from_statistics(&statistics, row * step * era5::COLUMNS + column * step)?
                    .native(),
            );
        }
    }
    std::fs::write(out, encode(&nodes)).map_err(|error| format!("{}: {error}", out.display()))
}

#[cfg(test)]
#[path = "weather_tests.rs"]
mod tests;
