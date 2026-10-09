//! The long-term weather of propagation, one global table on a 0.5 degree grid (ERA5 1991-2020)
//! sampled at the receiver: the probability of favourable propagation (CNOSSOS-EU 2.5.9) per
//! period and 16 direction sectors, and the air absorption of each octave band (ISO 9613-1 at
//! every 3-hourly state of the 30 years, its mean weighted by the periods' hours: CNOSSOS-EU 2.5.6
//! takes the yearly average atmosphere of the place). Sector s is centred on the bearing 22.5 * s
//! degrees, clockwise from north, of the direction the sound travels (source to receiver).

use crate::atmosphere::ALPHA_DB_PER_KM;
use crate::bands::{BANDS, PERIODS};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Direction sectors of 22.5 degrees.
pub const SECTORS: usize = 16;
/// Grid nodes per degree.
pub const NODES_PER_DEGREE: usize = 2;
/// Rows from 90 N to 90 S and columns from 0 E eastwards.
pub const ROWS: usize = 180 * NODES_PER_DEGREE + 1;
pub const COLUMNS: usize = 360 * NODES_PER_DEGREE;
/// A node: its percentages, then the absorption of each band in 0.01 dB/km (u16).
const NODE_BYTES: usize = PERIODS * SECTORS + 2 * BANDS;
const MAGIC: &[u8; 8] = b"qmwthr2\n";
/// The whole table's bytes (16.6 MB).
const TABLE_BYTES: usize = MAGIC.len() + ROWS * COLUMNS * NODE_BYTES;
/// The bytes [`read_place`] reads: the magic and four nodes.
pub const PLACE_BYTES: u64 = (MAGIC.len() + 4 * NODE_BYTES) as u64;

/// One node of the table: p per period and sector in percent, air absorption per band (dB/km).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherNode {
    pub percent: [[u8; SECTORS]; PERIODS],
    pub alpha_db_per_km: [f64; BANDS],
}

impl Default for WeatherNode {
    fn default() -> Self {
        WeatherNode {
            percent: [[0; SECTORS]; PERIODS],
            alpha_db_per_km: *ALPHA_DB_PER_KM,
        }
    }
}

/// The weather of one place: p per period and sector, and the air absorption per band (dB/km).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaceWeather {
    pub favourable: FavourableProbability,
    pub alpha_db_per_km: [f64; BANDS],
}

/// p per period and sector at one place, each in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FavourableProbability {
    pub by_sector: [[f64; SECTORS]; PERIODS],
}

impl FavourableProbability {
    /// p of `period` for sound travelling along `azimuth_rad` (atan2(north, east)), linear
    /// between the two nearest sector centres.
    pub fn at(&self, period: usize, azimuth_rad: f64) -> f64 {
        let bearing_deg = (90.0 - azimuth_rad.to_degrees()).rem_euclid(360.0);
        let position = bearing_deg / (360.0 / SECTORS as f64);
        let lower = position.floor() as usize % SECTORS;
        let upper = (lower + 1) % SECTORS;
        let fraction = position - position.floor();
        let row = &self.by_sector[period];
        row[lower] + fraction * (row[upper] - row[lower])
    }

    /// The largest p of each period: every ray's p is a convex mix of two sectors of this row,
    /// so the bound mixed at it stays an upper bound.
    pub fn maximum(&self) -> [f64; PERIODS] {
        self.by_sector
            .map(|row| row.iter().copied().fold(0.0, f64::max))
    }
}

/// The bytes of the global table: magic, then `ROWS x COLUMNS` nodes of `3 x 16` percentages and
/// 8 absorptions in 0.01 dB/km.
pub fn encode(nodes: &[WeatherNode]) -> Vec<u8> {
    assert_eq!(nodes.len(), ROWS * COLUMNS);
    let mut bytes = Vec::with_capacity(MAGIC.len() + nodes.len() * NODE_BYTES);
    bytes.extend_from_slice(MAGIC);
    for node in nodes {
        assert!(node.percent.iter().flatten().all(|&p| p <= 100));
        bytes.extend(node.percent.iter().flatten());
        for alpha in node.alpha_db_per_km {
            let code = (alpha * 100.0).round();
            assert!((0.0..=65_535.0).contains(&code), "absorption {alpha} dB/km");
            bytes.extend_from_slice(&(code as u16).to_le_bytes());
        }
    }
    bytes
}

/// The parsed global table.
pub struct WeatherTable {
    nodes: Vec<u8>,
}

impl WeatherTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != TABLE_BYTES || &bytes[..MAGIC.len()] != MAGIC {
            return Err("weather table: bad magic or length".into());
        }
        let nodes = bytes[MAGIC.len()..].to_vec();
        if !nodes.chunks_exact(NODE_BYTES).all(percentages_valid) {
            return Err("weather table: a percentage above 100".into());
        }
        Ok(WeatherTable { nodes })
    }

    /// The whole table from its file, for many places (the builders', the painter's).
    pub fn read(path: &Path) -> Result<Self, String> {
        let failed = |error: String| format!("{}: {error}", path.display());
        let bytes = std::fs::read(path).map_err(|error| failed(error.to_string()))?;
        Self::parse(&bytes).map_err(failed)
    }

    /// The weather of a place: p and the air absorption, bilinear between the nodes.
    pub fn place(&self, lat: f64, lon: f64) -> PlaceWeather {
        place_of(corners(lat, lon).map(|(index, weight)| {
            (
                &self.nodes[index * NODE_BYTES..(index + 1) * NODE_BYTES],
                weight,
            )
        }))
    }
}

/// The weather of one place read from the table's file: its four nodes alone, by positioned
/// reads, as [`WeatherTable::place`] gives it (a click needs one place, and reading and checking
/// the whole table cost it 6-10 ms warm, 13-23 ms cold on 2026-10-09).
pub fn read_place(path: &Path, lat: f64, lon: f64) -> Result<PlaceWeather, String> {
    let failed = |what: String| format!("{}: {what}", path.display());
    let file = File::open(path).map_err(|error| failed(error.to_string()))?;
    let length = file
        .metadata()
        .map_err(|error| failed(error.to_string()))?
        .len();
    let malformed = || failed("weather table: bad magic or length".into());
    if length != TABLE_BYTES as u64 {
        return Err(malformed());
    }
    let mut magic = [0u8; MAGIC.len()];
    file.read_exact_at(&mut magic, 0)
        .map_err(|error| failed(error.to_string()))?;
    if &magic != MAGIC {
        return Err(malformed());
    }
    let corners = corners(lat, lon);
    let mut nodes = [[0u8; NODE_BYTES]; 4];
    for (node, (index, _)) in nodes.iter_mut().zip(corners) {
        file.read_exact_at(node, (MAGIC.len() + index * NODE_BYTES) as u64)
            .map_err(|error| failed(error.to_string()))?;
        if !percentages_valid(node) {
            return Err(failed("weather table: a percentage above 100".into()));
        }
    }
    Ok(place_of(std::array::from_fn(|corner| {
        (&nodes[corner][..], corners[corner].1)
    })))
}

fn percentages_valid(node: &[u8]) -> bool {
    node[..PERIODS * SECTORS].iter().all(|&p| p <= 100)
}

/// The four nodes around a place, as their index in the table, and their bilinear weights,
/// longitude wrapping.
fn corners(lat: f64, lon: f64) -> [(usize, f64); 4] {
    let nodes = NODES_PER_DEGREE as f64;
    let y = ((90.0 - lat) * nodes).clamp(0.0, (ROWS - 1) as f64);
    let x = lon.rem_euclid(360.0) * nodes;
    let (row, column) = ((y.floor() as usize).min(ROWS - 2), x.floor() as usize);
    let (fy, fx) = (y - row as f64, x - column as f64);
    let index = |row: usize, column: usize| row * COLUMNS + column % COLUMNS;
    [
        (index(row, column), (1.0 - fy) * (1.0 - fx)),
        (index(row, column + 1), (1.0 - fy) * fx),
        (index(row + 1, column), fy * (1.0 - fx)),
        (index(row + 1, column + 1), fy * fx),
    ]
}

/// A place's weather from its four nodes' bytes and their weights.
fn place_of(corners: [(&[u8], f64); 4]) -> PlaceWeather {
    let mix = |value: &dyn Fn(&[u8]) -> f64| {
        corners
            .iter()
            .map(|(node, weight)| weight * value(node))
            .sum::<f64>()
            / 100.0
    };
    PlaceWeather {
        favourable: FavourableProbability {
            by_sector: std::array::from_fn(|period| {
                std::array::from_fn(|sector| {
                    mix(&|node| f64::from(node[period * SECTORS + sector]))
                })
            }),
        },
        alpha_db_per_km: std::array::from_fn(|band| {
            let at = PERIODS * SECTORS + 2 * band;
            mix(&|node| f64::from(u16::from_le_bytes([node[at], node[at + 1]])))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sectors_interpolate_and_wrap_through_north() {
        let mut by_sector = [[0.0; SECTORS]; PERIODS];
        by_sector[0][0] = 0.8;
        by_sector[0][15] = 0.4;
        by_sector[0][4] = 0.6;
        let p = FavourableProbability { by_sector };
        let north = std::f64::consts::FRAC_PI_2;
        assert!((p.at(0, north) - 0.8).abs() < 1e-12);
        assert!((p.at(0, 0.0) - 0.6).abs() < 1e-12, "east is sector 4");
        let halfway_to_sector_15 = north + (11.25f64).to_radians();
        assert!((p.at(0, halfway_to_sector_15) - 0.6).abs() < 1e-12);
        assert_eq!(p.maximum()[0], 0.8);
    }

    #[test]
    fn the_table_interpolates_between_nodes_and_across_the_antimeridian() {
        let mut nodes = vec![WeatherNode::default(); ROWS * COLUMNS];
        let at = |lat: f64, lon: f64| {
            ((90.0 - lat) * 2.0) as usize * COLUMNS + ((lon * 2.0) as usize) % COLUMNS
        };
        nodes[at(50.0, 0.0)].percent[1][3] = 40;
        nodes[at(50.0, 359.5)].percent[1][3] = 80;
        nodes[at(50.0, 0.0)].alpha_db_per_km[7] = 100.0;
        nodes[at(50.0, 359.5)].alpha_db_per_km[7] = 120.0;
        let bytes = encode(&nodes);
        let table = WeatherTable::parse(&bytes).unwrap();
        let p = |lat: f64, lon: f64| table.place(lat, lon).favourable.by_sector[1][3];
        assert!((p(50.0, -0.25) - 0.6).abs() < 1e-12);
        assert!((p(50.0, 359.75) - 0.6).abs() < 1e-12);
        assert!((p(50.25, 0.0) - 0.2).abs() < 1e-12);
        assert!((table.place(50.0, -0.25).alpha_db_per_km[7] - 110.0).abs() < 1e-9);
        let default = table.place(10.0, 10.0).alpha_db_per_km;
        for band in 0..BANDS {
            assert!((default[band] - ALPHA_DB_PER_KM[band]).abs() < 0.005);
        }
        assert!(WeatherTable::parse(&bytes[..100]).is_err());

        // A click reads its four nodes from the file and gets the table's answer.
        let path = std::env::temp_dir().join(format!("qm-weather-{}", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        for (lat, lon) in [
            (50.0, -0.25),
            (50.2, 359.9),
            (50.25, 0.0),
            (90.0, 10.0),
            (-90.0, 0.0),
        ] {
            assert_eq!(read_place(&path, lat, lon).unwrap(), table.place(lat, lon));
        }
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(read_place(&path, 50.0, 0.0).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
