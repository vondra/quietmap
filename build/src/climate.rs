//! The climate of a place (WorldClim 2.1, 1970-2000, 10 arc-minutes; `fetch/worldclim.sh` writes
//! each variable as raw little-endian f32, 2,160 x 1,080 cells from 90 N and 180 W): the yearly
//! mean air temperature (`bio_1`) for the road converter's CNOSSOS-EU temperature correction
//! (2.2.2, Eq. 2.2.10), and with the mean temperatures of the warmest and the coldest quarter
//! (`bio_10`, `bio_11`) the degree days that run the homes' heat pumps and air conditioners.

use physics::emission::road::REFERENCE_AIR_TEMPERATURE_C;
use std::path::Path;

const COLUMNS: usize = 2_160;
const ROWS: usize = 1_080;
const CELLS_PER_DEGREE: f64 = 6.0;

/// The global grid of yearly mean air temperature (C); the sea has none.
pub struct Temperature {
    cells: Vec<f32>,
}

impl Temperature {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if bytes.len() != 4 * COLUMNS * ROWS {
            return Err(format!(
                "{}: {} bytes, not a 10' grid",
                path.display(),
                bytes.len()
            ));
        }
        let cells = bytes
            .chunks_exact(4)
            .map(|cell| f32::from_le_bytes(cell.try_into().expect("four bytes")))
            .collect();
        Ok(Temperature { cells })
    }

    fn cell(&self, row: usize, column: usize) -> Option<f64> {
        let value = self.cells[row * COLUMNS + column % COLUMNS];
        (value > -100.0 && value < 100.0).then_some(f64::from(value))
    }

    /// The temperature at a place, bilinear between the four nearest cell centres; where some of
    /// them are sea, the mean of the land ones; with none, the surfaces' reference 20 C (no
    /// correction).
    pub fn at(&self, lat: f64, lon: f64) -> f64 {
        let y = ((90.0 - lat) * CELLS_PER_DEGREE - 0.5).clamp(0.0, (ROWS - 1) as f64);
        let x = (lon + 180.0).rem_euclid(360.0) * CELLS_PER_DEGREE - 0.5;
        let (row, column) = ((y.floor() as usize).min(ROWS - 2), x.floor());
        let (fy, fx) = (y - row as f64, x - column);
        let column = column.rem_euclid(COLUMNS as f64) as usize;
        let corners = [
            (self.cell(row, column), (1.0 - fy) * (1.0 - fx)),
            (self.cell(row, column + 1), (1.0 - fy) * fx),
            (self.cell(row + 1, column), fy * (1.0 - fx)),
            (self.cell(row + 1, column + 1), fy * fx),
        ];
        let land: Vec<(f64, f64)> = corners
            .iter()
            .filter_map(|&(value, weight)| value.map(|value| (value, weight)))
            .collect();
        if land.len() == 4 {
            land.iter().map(|(value, weight)| value * weight).sum()
        } else if land.is_empty() {
            REFERENCE_AIR_TEMPERATURE_C
        } else {
            land.iter().map(|(value, _)| value).sum::<f64>() / land.len() as f64
        }
    }
}

/// The yearly mean, warmest-quarter and coldest-quarter temperature grids of a WorldClim
/// directory (`bio1.f32`, `bio10.f32`, `bio11.f32`).
pub struct Climate {
    pub mean: Temperature,
    warmest_quarter: Temperature,
    coldest_quarter: Temperature,
}

/// Heating degree days below 16 C and cooling degree days above 18 C of a year (K days).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DegreeDays {
    pub heating: f64,
    pub cooling: f64,
}

/// The base temperatures of heating (EN 14825's heat pumps run below 16 C) and cooling (the
/// cooling degree days of the air-conditioning adoption studies).
const HEATING_BASE_C: f64 = 16.0;
const COOLING_BASE_C: f64 = 18.0;
/// A sinusoid's mean over its warmest quarter in units of its amplitude: sin(pi/4) / (pi/4).
const QUARTER_MEAN_OF_AMPLITUDE: f64 = 0.900_316_316_157_106;

impl Climate {
    pub fn load(directory: &Path) -> Result<Self, String> {
        let grid = |name: &str| Temperature::load(&directory.join(name));
        Ok(Climate {
            mean: grid("bio1.f32")?,
            warmest_quarter: grid("bio10.f32")?,
            coldest_quarter: grid("bio11.f32")?,
        })
    }

    /// The year as a sinusoid through the yearly mean whose warmest and coldest quarters average
    /// the place's (monthly means: the days' own swings add degree days at the margins).
    pub fn degree_days(&self, lat: f64, lon: f64) -> DegreeDays {
        let mean = self.mean.at(lat, lon);
        let amplitude = ((self.warmest_quarter.at(lat, lon) - self.coldest_quarter.at(lat, lon))
            / (2.0 * QUARTER_MEAN_OF_AMPLITUDE))
            .max(0.0);
        let above = |base: f64| 365.0 * mean_excess(mean, amplitude, base);
        DegreeDays {
            heating: above(HEATING_BASE_C) - 365.0 * (mean - HEATING_BASE_C),
            cooling: above(COOLING_BASE_C),
        }
    }

    /// The share of the year whose daily mean reaches `base_c`, on the same sinusoidal year.
    pub fn share_of_year_above(&self, lat: f64, lon: f64, base_c: f64) -> f64 {
        let mean = self.mean.at(lat, lon);
        let amplitude = (self.warmest_quarter.at(lat, lon) - self.coldest_quarter.at(lat, lon))
            / (2.0 * QUARTER_MEAN_OF_AMPLITUDE);
        share_above(mean, amplitude.max(0.0), base_c)
    }
}

/// The share of a year T = mean + amplitude cos(t) at or above `base`.
fn share_above(mean: f64, amplitude: f64, base: f64) -> f64 {
    if amplitude <= 0.0 {
        return if mean >= base { 1.0 } else { 0.0 };
    }
    ((base - mean) / amplitude).clamp(-1.0, 1.0).acos() / std::f64::consts::PI
}

/// The mean of max(0, T - base) over a year T = mean + amplitude cos(t).
fn mean_excess(mean: f64, amplitude: f64, base: f64) -> f64 {
    if amplitude <= 0.0 {
        return (mean - base).max(0.0);
    }
    let u = (base - mean) / amplitude;
    if u >= 1.0 {
        0.0
    } else if u <= -1.0 {
        mean - base
    } else {
        ((mean - base) * u.acos() + amplitude * (1.0 - u * u).sqrt()) / std::f64::consts::PI
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prague (9.3, 18.2, 0.6 C) heats 2,640 K days and hardly cools; Bangkok (28.5, 29.9, 26.8)
    /// cools 3,830 and never heats; Oslo (5.8, 15.5, -3.2) heats 3,730.
    #[test]
    fn degree_days_follow_the_quarters() {
        let place = |mean: f64, warm: f64, cold: f64| {
            let amplitude = (warm - cold) / (2.0 * QUARTER_MEAN_OF_AMPLITUDE);
            let above = |base: f64| 365.0 * mean_excess(mean, amplitude, base);
            (
                above(HEATING_BASE_C) - 365.0 * (mean - HEATING_BASE_C),
                above(COOLING_BASE_C),
            )
        };
        let (heating, cooling) = place(9.3, 18.2, 0.6);
        assert!(
            (heating - 2_640.0).abs() < 15.0 && cooling < 50.0,
            "{heating} {cooling}"
        );
        let (heating, cooling) = place(28.5, 29.9, 26.8);
        assert!(
            heating == 0.0 && (cooling - 3_832.5).abs() < 1.0,
            "{heating} {cooling}"
        );
        let (heating, cooling) = place(5.8, 15.5, -3.2);
        assert!(
            (heating - 3_727.0).abs() < 15.0 && cooling == 0.0,
            "{heating} {cooling}"
        );
    }

    /// Paris (12.4, 19.5, 5.5 C) reaches 12.5 C half the year, Barcelona (16.5, 23.5, 10.5) seven
    /// tenths of it, Bangkok always, Tromsø never.
    #[test]
    fn shares_of_the_year_follow_the_quarters() {
        let share = |mean: f64, warm: f64, cold: f64| {
            share_above(
                mean,
                (warm - cold) / (2.0 * QUARTER_MEAN_OF_AMPLITUDE),
                12.5,
            )
        };
        assert!((share(12.4, 19.5, 5.5) - 0.50).abs() < 0.01);
        assert!((share(16.5, 23.5, 10.5) - 0.69).abs() < 0.01);
        assert_eq!(share(28.5, 30.0, 27.0), 1.0);
        assert_eq!(share(3.0, 10.0, -3.0), 0.0);
    }

    /// Bilinear between cell centres, the mean of the land corners on a coast, 20 C at sea.
    #[test]
    fn the_grid_reads_between_cell_centres_and_skips_the_sea() {
        let mut cells = vec![-3.4e38f32; COLUMNS * ROWS];
        let (row, column) = (240, 1_164);
        for (dr, dc, value) in [(0, 0, 9.0), (0, 1, 11.0), (1, 0, 9.0), (1, 1, 11.0)] {
            cells[(row + dr) * COLUMNS + column + dc] = value;
        }
        let grid = Temperature { cells };
        let centre = |r: usize, c: usize| {
            (
                90.0 - (r as f64 + 0.5) / 6.0,
                -180.0 + (c as f64 + 0.5) / 6.0,
            )
        };
        let (lat, lon) = centre(row, column);
        assert!((grid.at(lat, lon) - 9.0).abs() < 1e-6);
        assert!((grid.at(lat, lon + 1.0 / 12.0) - 10.0).abs() < 1e-6);
        let (_, east) = centre(row, column + 1);
        assert!(
            (grid.at(lat, east + 1.0 / 12.0) - 11.0).abs() < 1e-6,
            "coast"
        );
        assert_eq!(grid.at(0.0, -30.0), REFERENCE_AIR_TEMPERATURE_C);
    }
}
