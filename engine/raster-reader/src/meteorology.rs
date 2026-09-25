//! One square's ERA5 climatology window and continuous receiver/direction sampling.
use grid::{raster::RasterWindow, square_name, Square};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const ROWS: usize = 721;
pub const COLUMNS: usize = 1440;
pub const SECTORS: usize = 16;
/// ERA5 lattice density: 0.25° nodes on the same z9 edge rule as the 1″ rasters.
pub const ERA5_NODES_PER_DEGREE: i32 = 4;
const CONTRACT: &str = include_str!("../../noise-compute/meteorology-contract.json");
const HEADER_LEN: usize = 16;
const NODE_BYTES: usize = 240;

/// CUDA transfer layout: 48 probability bytes, then two 24-float moment arrays.
/// The per-square file stores these records row-major, floats little-endian.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct MeteorologyNode {
    pub p_percent: [[u8; SECTORS]; 3],
    pub alpha_mean: [[f32; 8]; 3],
    pub alpha_variance: [[f32; 8]; 3],
}

#[derive(Clone, Copy, Default, Debug)]
pub struct MeteorologySample {
    pub p: [[f32; SECTORS]; 3],
    pub alpha_mean: [[f32; 8]; 3],
    pub alpha_variance: [[f32; 8]; 3],
}

impl MeteorologySample {
    /// Linear circular interpolation; azimuth is source→receiver clockwise from north.
    pub fn probability(&self, period: usize, azimuth_degrees: f64) -> Result<f32, String> {
        if period >= 3 || !azimuth_degrees.is_finite() {
            return Err("invalid meteorology period or azimuth".into());
        }
        let sector = azimuth_degrees.rem_euclid(360.0) / (360.0 / SECTORS as f64);
        let first = sector.floor() as usize % SECTORS;
        let fraction = (sector - sector.floor()) as f32;
        Ok(self.p[period][first] * (1.0 - fraction)
            + self.p[period][(first + 1) % SECTORS] * fraction)
    }
}

pub struct Meteorology {
    window: RasterWindow,
    nodes: Vec<MeteorologyNode>,
    maximum_probability: [f32; 3],
}

impl Meteorology {
    pub fn path(root: &Path, square: Square) -> PathBuf {
        root.join(square_name(square)).join("meteorology.bin")
    }

    /// Load one square's window: a 16-byte header (contract magic, window, dims) then row-major
    /// node records. Rejects wrong magic, dims, lengths and nonphysical values.
    pub fn load(path: &Path, square: Square) -> Result<Self, String> {
        Self::read(path, square).map_err(|error| format!("meteorology {}: {error}", path.display()))
    }

    fn read(path: &Path, square: Square) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|error| error.to_string())?
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() < HEADER_LEN {
            return Err("truncated meteorology header".into());
        }
        if bytes[0..8] != expected_magic()? {
            return Err("wrong meteorology contract magic".into());
        }
        let window = RasterWindow::for_square_with_density(square, ERA5_NODES_PER_DEGREE);
        let west = i16::from_le_bytes([bytes[8], bytes[9]]) as i32;
        let north = i16::from_le_bytes([bytes[10], bytes[11]]) as i32;
        let columns = u16::from_le_bytes([bytes[12], bytes[13]]) as u32;
        let rows = u16::from_le_bytes([bytes[14], bytes[15]]) as u32;
        if west != window.west_node
            || north != window.north_node
            || columns != window.columns
            || rows != window.rows
        {
            return Err("meteorology window does not match the square".into());
        }
        if bytes.len() != HEADER_LEN + window.cell_count() * NODE_BYTES {
            return Err("wrong meteorology window byte length".into());
        }
        let mut nodes = Vec::with_capacity(window.cell_count());
        let mut maxima = [0u8; 3];
        for cell in 0..window.cell_count() {
            let base = HEADER_LEN + cell * NODE_BYTES;
            let mut node = MeteorologyNode::default();
            for period in 0..3 {
                for sector in 0..SECTORS {
                    let value = bytes[base + period * SECTORS + sector];
                    if value > 100 {
                        return Err("p outside 0..100 percent".into());
                    }
                    node.p_percent[period][sector] = value;
                }
                maxima[period] = maxima[period].max(*node.p_percent[period].iter().max().unwrap());
            }
            for period in 0..3 {
                for band in 0..8 {
                    let mean = read_f32(&bytes, base + 48 + period * 32 + band * 4);
                    let variance = read_f32(&bytes, base + 144 + period * 32 + band * 4);
                    if !mean.is_finite() || mean < 0.0 || !variance.is_finite() || variance < 0.0 {
                        return Err("nonfinite or negative absorption moment".into());
                    }
                    node.alpha_mean[period][band] = mean;
                    node.alpha_variance[period][band] = variance;
                }
            }
            nodes.push(node);
        }
        Ok(Self {
            window,
            nodes,
            maximum_probability: maxima.map(|p| f32::from(p) / 100.0),
        })
    }

    fn local_index(&self, x: usize, y: usize) -> Option<usize> {
        let west = self.window.west_node.rem_euclid(COLUMNS as i32) as usize;
        let column = (x + COLUMNS - west) % COLUMNS;
        let row = y.checked_sub((90 * ERA5_NODES_PER_DEGREE - self.window.north_node) as usize)?;
        (column < self.window.columns as usize && row < self.window.rows as usize)
            .then(|| row * self.window.columns as usize + column)
    }

    /// The window this file covers; painters sample receivers through it alone.
    pub fn window(&self) -> RasterWindow {
        self.window
    }

    /// Bilinear receiver interpolation inside this square's window.
    pub fn at(&self, latitude: f64, longitude: f64) -> Result<MeteorologySample, String> {
        sample_at(latitude, longitude, |x, y| {
            self.local_index(x, y).map(|index| self.nodes[index])
        })
    }

    /// Window maxima: the largest stored sector value per period, the conservative bound for
    /// every interpolation inside this window.
    pub fn maximum_probability(&self) -> [f32; 3] {
        self.maximum_probability
    }
}

/// The file magic names the contract version, so a method change refuses old files.
fn expected_magic() -> Result<[u8; 8], String> {
    let contract: std::collections::HashMap<String, String> =
        serde_json::from_str(CONTRACT).unwrap();
    let version = contract
        .get("meteorology_contract")
        .ok_or("contract lacks meteorology_contract")?;
    if version.len() != 1 {
        return Err("unsupported meteorology contract version".into());
    }
    let mut magic = [0u8; 8];
    magic[0..6].copy_from_slice(b"qm-met");
    magic[6] = version.as_bytes()[0];
    magic[7] = b'\n';
    Ok(magic)
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn sample_at(
    latitude: f64,
    longitude: f64,
    node: impl Fn(usize, usize) -> Option<MeteorologyNode>,
) -> Result<MeteorologySample, String> {
    if !latitude.is_finite() || !longitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
        return Err("invalid meteorology coordinate".into());
    }
    let x = longitude.rem_euclid(360.0) * 4.0;
    let y = (90.0 - latitude) * 4.0;
    let x0 = x.floor() as usize % COLUMNS;
    let y0 = (y.floor() as usize).min(ROWS - 1);
    let fx = (x - x.floor()) as f32;
    let fy = (y - y.floor()) as f32;
    let mut result = MeteorologySample::default();
    for (xx, wx) in [(x0, 1.0 - fx), ((x0 + 1) % COLUMNS, fx)] {
        for (yy, wy) in [(y0, 1.0 - fy), ((y0 + 1).min(ROWS - 1), fy)] {
            let Some(n) = node(xx, yy) else {
                return Err("receiver outside the square meteorology window".into());
            };
            let weight = wx * wy;
            for period in 0..3 {
                for sector in 0..SECTORS {
                    result.p[period][sector] +=
                        f32::from(n.p_percent[period][sector]) * 0.01 * weight;
                }
                for band in 0..8 {
                    result.alpha_mean[period][band] += n.alpha_mean[period][band] * weight;
                    result.alpha_variance[period][band] += n.alpha_variance[period][band] * weight;
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "meteorology_tests.rs"]
mod tests;
