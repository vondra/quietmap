//! The dev4 r260924 z9 tree as a builder input until the builders read the sources themselves:
//! square paths, Arrow tables, the z30 grid and the one-arc-second raster windows.

use arrow_array::types::ArrowPrimitiveType;
use arrow_array::{Array, Float32Array, PrimitiveArray, RecordBatch, StringArray};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiles::geo::{GlobalSteps, MAX_LATITUDE_DEG, WGS84_A_M};
use tiles::terrain::{NODES_PER_DEGREE, Window};

/// z9 squares per axis.
pub const Z9_PER_AXIS: u32 = 512;

/// A z9 square in the XYZ numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Square {
    pub x: u32,
    pub y: u32,
}

impl Square {
    /// The square of a z30 cell (x east from 180 W, y north from the equator at 2^29).
    pub fn of_z30(gx: i32, gy: i32) -> Square {
        let per_square = (1i64 << 30) / i64::from(Z9_PER_AXIS);
        let last = i64::from(Z9_PER_AXIS) - 1;
        Square {
            x: (i64::from(gx) / per_square).clamp(0, last) as u32,
            y: ((((1i64 << 30) - 1) - i64::from(gy)) / per_square).clamp(0, last) as u32,
        }
    }

    /// The square and its existing neighbours (x wraps, y stops at the poles).
    pub fn with_neighbours(self) -> Vec<Square> {
        let mut squares = Vec::new();
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let y = i64::from(self.y) + dy;
                if !(0..i64::from(Z9_PER_AXIS)).contains(&y) {
                    continue;
                }
                let x = (i64::from(self.x) + dx).rem_euclid(i64::from(Z9_PER_AXIS));
                squares.push(Square {
                    x: x as u32,
                    y: y as u32,
                });
            }
        }
        squares
    }
}

/// The dev4 prepared tree (`z9/<x>/<y>/*.arrow`) and its raster tree (`z9/<x>/<y>/*.u8|u16le`).
pub struct Dev4 {
    pub prepared: PathBuf,
    pub rasters: PathBuf,
}

/// An Arrow IPC file read whole: its schema metadata and record batches.
pub struct Table {
    pub metadata: HashMap<String, String>,
    pub batches: Vec<RecordBatch>,
}

impl Dev4 {
    pub fn prepared_file(&self, square: Square, name: &str) -> PathBuf {
        self.prepared
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string())
            .join(name)
    }

    pub fn raster_file(&self, square: Square, name: &str) -> PathBuf {
        self.rasters
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string())
            .join(name)
    }

    /// A prepared Arrow table, or `None` when the square has no such file.
    pub fn table(&self, square: Square, name: &str) -> Result<Option<Table>, String> {
        read_table(&self.prepared_file(square, name))
    }

    /// The square's country (ISO 3166 alpha-2, little-endian) from dev4's 13-byte
    /// `square-country-city.bin` (Morton id u64, continent u8, country u16, city u16): the country
    /// at its centre, else the most of its interior; 0 when unknown or the square has no record.
    pub fn square_country(&self, square: Square) -> Result<u16, String> {
        let path = self.prepared_file(square, "square-country-city.bin");
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == 13 => Ok(u16::from_le_bytes([bytes[9], bytes[10]])),
            Ok(bytes) => Err(format!("{}: {} bytes, not 13", path.display(), bytes.len())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }
}

pub fn read_table(path: &Path) -> Result<Option<Table>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let reader = arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = reader.schema().metadata().clone().into();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Some(Table { metadata, batches }))
}

/// Require a schema stamp, so a converter never reads a table of another contract.
pub fn require_stamp(table: &Table, key: &str, value: &str) -> Result<(), String> {
    match table.metadata.get(key) {
        Some(found) if found == value => Ok(()),
        found => Err(format!("stamp {key}: expected {value:?}, found {found:?}")),
    }
}

/// A typed column of a batch.
pub fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("column {name} missing or of another type"))
}

/// A nullable text cell; null reads as empty.
pub fn text(values: &StringArray, row: usize) -> &str {
    if values.is_valid(row) {
        values.value(row)
    } else {
        ""
    }
}

/// A nullable cell of a primitive column.
pub fn cell<T: ArrowPrimitiveType>(values: &PrimitiveArray<T>, row: usize) -> Option<T::Native> {
    values.is_valid(row).then(|| values.value(row))
}

/// A nullable float cell when it is positive.
pub fn positive(values: &Float32Array, row: usize) -> Option<f64> {
    cell(values, row)
        .filter(|value| *value > 0.0)
        .map(f64::from)
}

/// The nearest global int16 step to the centre of a dev4 z30 cell. dev4's z30 grid counts
/// 2^30 cells from the south-west corner with y growing north; one step is exactly 8 cells.
pub fn z30_to_global(gx: i32, gy: i32) -> GlobalSteps {
    let x = (2 * i64::from(gx) + 9).div_euclid(16);
    let y = ((1i64 << 31) - 2 * i64::from(gy) + 7).div_euclid(16);
    GlobalSteps { x, y }
}

/// (latitude, longitude) in degrees of a dev4 z30 cell's south-west corner, as dev4 decodes
/// centroids (`grid_cell_lonlat`): the quantum is the Web Mercator circumference over 2^30.
pub fn z30_corner_degrees(gx: i32, gy: i32) -> (f64, f64) {
    let [x, y] = z30_corner_mercator_m(gx, gy);
    let lat = 2.0 * (y / WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2;
    (lat.to_degrees(), (x / WGS84_A_M).to_degrees())
}

/// The side of a dev4 z30 cell in Web Mercator metres: the circumference over 2^30.
pub(crate) const Z30_QUANTUM_M: f64 = 0.037_322_767_717_044_72;

/// Web Mercator metres (EPSG:3857) of a dev4 z30 cell's south-west corner (`grid_to_meters`).
pub fn z30_corner_mercator_m(gx: i32, gy: i32) -> [f64; 2] {
    [gx, gy].map(|g| (i64::from(g) - (1 << 29)) as f64 * Z30_QUANTUM_M)
}

/// The dev4 z30 cell holding a position (`lonlat_to_grid`), latitude clamped to the Mercator limit.
pub fn degrees_to_z30(lat: f64, lon: f64) -> (i32, i32) {
    let phi = lat.clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG).to_radians();
    let x = WGS84_A_M * lon.to_radians();
    let y = WGS84_A_M * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln();
    let cell = |m: f64| ((m / Z30_QUANTUM_M).floor() as i64 + (1 << 29)) as i32;
    (cell(x), cell(y))
}

/// dev4's node window of a z9 square: floor/ceil node edges, the polar rows reaching +-90 deg.
pub fn z9_raster_window(square: Square) -> Window {
    let axis = i64::from(Z9_PER_AXIS);
    let longitude_nodes = 360 * i64::from(NODES_PER_DEGREE);
    let west = i64::from(square.x) * longitude_nodes / axis - longitude_nodes / 2;
    let east =
        ((i64::from(square.x) + 1) * longitude_nodes + axis - 1) / axis - longitude_nodes / 2;
    let edge_latitude_nodes = |y: u32| {
        let mercator = std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / f64::from(Z9_PER_AXIS));
        mercator.sinh().atan().to_degrees() * f64::from(NODES_PER_DEGREE)
    };
    let pole = 90 * NODES_PER_DEGREE;
    let north = if square.y == 0 {
        pole
    } else {
        edge_latitude_nodes(square.y).ceil() as i32
    };
    let south = if square.y == Z9_PER_AXIS - 1 {
        -pole
    } else {
        edge_latitude_nodes(square.y + 1).floor() as i32
    };
    Window {
        north_node: north,
        west_node: west as i32,
        rows: (north - south + 1) as u32,
        columns: (east - west + 1) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::geo::{Mercator, TileId};

    #[test]
    fn z30_cells_land_on_their_tile() {
        // Vinohrady in dev4's grid (z30, y north) against its z12 tile 2212/1387.
        let global = z30_to_global(579_939_889, 709_936_176);
        let tile = TileId::containing(global.to_mercator());
        assert_eq!(tile, TileId { x: 2212, y: 1387 });
        let (lat, lon) = global.to_mercator().to_degrees();
        assert!((lat - 50.075).abs() < 0.001 && (lon - 14.44).abs() < 0.001);
        assert_eq!(z30_to_global(0, (1 << 30) - 1), GlobalSteps { x: 0, y: 0 });
        assert_eq!(z30_to_global(7, (1 << 30) - 8), GlobalSteps { x: 1, y: 1 });
        let _ = Mercator::from_degrees(0.0, 0.0);
    }

    #[test]
    fn degrees_snap_to_the_z30_cell_whose_corner_they_decode_to() {
        for cell in [
            (579_939_889, 709_936_176),
            (1 << 29, 1 << 29),
            (12, 1_000_000_000),
        ] {
            let (lat, lon) = z30_corner_degrees(cell.0, cell.1);
            let [x, y] = z30_corner_mercator_m(cell.0, cell.1);
            let inside = |m: f64| m + 0.5 * Z30_QUANTUM_M;
            let (lat_inside, lon_inside) = (
                (2.0 * (inside(y) / WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2)
                    .to_degrees(),
                (inside(x) / WGS84_A_M).to_degrees(),
            );
            assert_eq!(degrees_to_z30(lat_inside, lon_inside), cell, "{lat} {lon}");
        }
    }

    #[test]
    fn prague_square_window_matches_the_file_sizes() {
        // dem.u16le of z9/276/173 holds 1,627 x 2,533 nodes (8,242,382 bytes).
        let window = z9_raster_window(Square { x: 276, y: 173 });
        assert_eq!((window.rows, window.columns), (1_627, 2_533));
        assert_eq!((window.north_node, window.west_node), (181_042, 50_625));
    }
}
