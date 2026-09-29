//! Builds the global weather table: p per period and direction sector on the 0.5 degree grid, cut
//! from dev4's per-square ERA5 files (every 0.5 degree node is a 0.25 degree ERA5 node).

use crate::dev4::{Dev4, Square};
use physics::bands::PERIODS;
use physics::weather::{COLUMNS, NODES_PER_DEGREE, ROWS, SECTORS, encode};
use std::collections::BTreeMap;
use std::path::Path;
use tiles::geo::{Mercator, TileId};

const ERA5_NODES_PER_DEGREE: i64 = 4;
const HEADER_BYTES: usize = 16;
const RECORD_BYTES: usize = 240;

/// One dev4 `meteorology.bin`: its window (west and north node, columns, rows) and records.
struct SquareWeather {
    west: i64,
    north: i64,
    columns: i64,
    rows: i64,
    bytes: Vec<u8>,
}

impl SquareWeather {
    fn load(dev4: &Dev4, square: Square) -> Result<Self, String> {
        let path = dev4.raster_file(square, "meteorology.bin");
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if bytes.len() < HEADER_BYTES || &bytes[..7] != b"qm-met1" {
            return Err(format!("{}: not a meteorology.bin", path.display()));
        }
        let i16_at = |at: usize| i64::from(i16::from_le_bytes([bytes[at], bytes[at + 1]]));
        let u16_at = |at: usize| i64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
        let (west, north, columns, rows) = (i16_at(8), i16_at(10), u16_at(12), u16_at(14));
        if bytes.len() != HEADER_BYTES + RECORD_BYTES * (columns * rows) as usize {
            return Err(format!(
                "{}: length does not match its window",
                path.display()
            ));
        }
        Ok(SquareWeather {
            west,
            north,
            columns,
            rows,
            bytes,
        })
    }

    /// The 48 percentages of the ERA5 node at global indices (column east of 0 E, row south of 90 N).
    fn percent(&self, column: i64, row: i64) -> Option<[[u8; SECTORS]; PERIODS]> {
        let wrap = 360 * ERA5_NODES_PER_DEGREE;
        let local_column = (column - self.west).rem_euclid(wrap);
        let local_row = row - (90 * ERA5_NODES_PER_DEGREE - self.north);
        if local_column >= self.columns || local_row < 0 || local_row >= self.rows {
            return None;
        }
        let at = HEADER_BYTES + RECORD_BYTES * (local_row * self.columns + local_column) as usize;
        Some(std::array::from_fn(|period| {
            std::array::from_fn(|sector| self.bytes[at + period * SECTORS + sector])
        }))
    }
}

/// Writes the table to `out`; every node is read from the dev4 square that owns its position.
pub fn build(dev4: &Dev4, out: &Path) -> Result<(), String> {
    let step = ERA5_NODES_PER_DEGREE / NODES_PER_DEGREE as i64;
    let mut by_square: BTreeMap<Square, Vec<(usize, i64, i64)>> = BTreeMap::new();
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let lat = 90.0 - row as f64 / NODES_PER_DEGREE as f64;
            let lon = column as f64 / NODES_PER_DEGREE as f64;
            let tile = TileId::containing(Mercator::from_degrees(lat, lon));
            let square = Square {
                x: tile.x >> 3,
                y: tile.y >> 3,
            };
            by_square.entry(square).or_default().push((
                row * COLUMNS + column,
                column as i64 * step,
                row as i64 * step,
            ));
        }
    }
    let mut table = vec![[[0u8; SECTORS]; PERIODS]; ROWS * COLUMNS];
    for (square, nodes) in by_square {
        let weather = SquareWeather::load(dev4, square)?;
        for (index, era5_column, era5_row) in nodes {
            table[index] = weather.percent(era5_column, era5_row).ok_or_else(|| {
                format!("square {square:?} lacks ERA5 node {era5_column}/{era5_row}")
            })?;
        }
    }
    std::fs::write(out, encode(&table)).map_err(|error| format!("{}: {error}", out.display()))
}
