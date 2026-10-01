//! The yearly mean air temperature of a place (WorldClim 2.1 `bio_1`, 1970-2000, 10 arc-minutes;
//! `fetch/worldclim.sh` writes it as raw little-endian f32, 2,160 x 1,080 cells from 90 N and
//! 180 W): the road converter's CNOSSOS-EU temperature correction (2.2.2, Eq. 2.2.10).

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

#[cfg(test)]
mod tests {
    use super::*;

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
