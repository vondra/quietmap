//! The long-term weather of propagation, one global table on a 0.5 degree grid (ERA5 1991-2020)
//! sampled at the receiver: the probability of favourable propagation (CNOSSOS-EU 2.5.9) per
//! period and 16 direction sectors, and the air absorption of each octave band (ISO 9613-1 at
//! every 3-hourly state of the 30 years, its mean weighted by the periods' hours: CNOSSOS-EU 2.5.6
//! takes the yearly average atmosphere of the place). Sector s is centred on the bearing 22.5 * s
//! degrees, clockwise from north, of the direction the sound travels (source to receiver).

use crate::atmosphere::ALPHA_DB_PER_KM;
use crate::bands::{BANDS, PERIODS};

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
        if bytes.len() != MAGIC.len() + ROWS * COLUMNS * NODE_BYTES
            || &bytes[..MAGIC.len()] != MAGIC
        {
            return Err("weather table: bad magic or length".into());
        }
        let nodes = bytes[MAGIC.len()..].to_vec();
        if nodes
            .chunks_exact(NODE_BYTES)
            .any(|node| node[..PERIODS * SECTORS].iter().any(|&p| p > 100))
        {
            return Err("weather table: a percentage above 100".into());
        }
        Ok(WeatherTable { nodes })
    }

    fn node(&self, row: usize, column: usize) -> &[u8] {
        let at = (row * COLUMNS + column % COLUMNS) * NODE_BYTES;
        &self.nodes[at..at + NODE_BYTES]
    }

    /// The four nodes around a place and their bilinear weights, longitude wrapping.
    fn corners(&self, lat: f64, lon: f64) -> [(&[u8], f64); 4] {
        let nodes = NODES_PER_DEGREE as f64;
        let y = ((90.0 - lat) * nodes).clamp(0.0, (ROWS - 1) as f64);
        let x = lon.rem_euclid(360.0) * nodes;
        let (row, column) = ((y.floor() as usize).min(ROWS - 2), x.floor() as usize);
        let (fy, fx) = (y - row as f64, x - column as f64);
        [
            (self.node(row, column), (1.0 - fy) * (1.0 - fx)),
            (self.node(row, column + 1), (1.0 - fy) * fx),
            (self.node(row + 1, column), fy * (1.0 - fx)),
            (self.node(row + 1, column + 1), fy * fx),
        ]
    }

    /// The air absorption of each band at a place (dB/km), bilinear between the nodes.
    pub fn alpha_at(&self, lat: f64, lon: f64) -> [f64; BANDS] {
        let corners = self.corners(lat, lon);
        std::array::from_fn(|band| {
            let at = PERIODS * SECTORS + 2 * band;
            corners
                .iter()
                .map(|(node, w)| w * f64::from(u16::from_le_bytes([node[at], node[at + 1]])))
                .sum::<f64>()
                / 100.0
        })
    }

    /// The weather of a place: p and the air absorption, bilinear between the nodes.
    pub fn place(&self, lat: f64, lon: f64) -> PlaceWeather {
        PlaceWeather {
            favourable: self.at(lat, lon),
            alpha_db_per_km: self.alpha_at(lat, lon),
        }
    }

    /// p at a place: bilinear between the four surrounding nodes, longitude wrapping.
    pub fn at(&self, lat: f64, lon: f64) -> FavourableProbability {
        let corners = self.corners(lat, lon);
        FavourableProbability {
            by_sector: std::array::from_fn(|period| {
                std::array::from_fn(|sector| {
                    corners
                        .iter()
                        .map(|(node, w)| w * f64::from(node[period * SECTORS + sector]))
                        .sum::<f64>()
                        / 100.0
                })
            }),
        }
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
        let table = WeatherTable::parse(&encode(&nodes)).unwrap();
        assert!((table.at(50.0, -0.25).by_sector[1][3] - 0.6).abs() < 1e-12);
        assert!((table.at(50.0, 359.75).by_sector[1][3] - 0.6).abs() < 1e-12);
        assert!((table.at(50.25, 0.0).by_sector[1][3] - 0.2).abs() < 1e-12);
        assert!((table.alpha_at(50.0, -0.25)[7] - 110.0).abs() < 1e-9);
        let default = table.alpha_at(10.0, 10.0);
        for band in 0..BANDS {
            assert!((default[band] - ALPHA_DB_PER_KM[band]).abs() < 0.005);
        }
        assert!(WeatherTable::parse(&encode(&nodes)[..100]).is_err());
    }
}
