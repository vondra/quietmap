//! One square's ERA5 climatology window and continuous receiver/direction sampling.
use grid::{raster::RasterWindow, square_name, Square};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    alpha_min_db_per_km: [f32; 8],
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
        Ok(Self::build(window, nodes, maxima.map(|p| f32::from(p) / 100.0)))
    }

    /// A window from already-parsed nodes (the synthetic defaults window of a release
    /// without files, test fixtures): panics on contract-invalid values, a programming
    /// error, not data.
    pub fn from_nodes(window: RasterWindow, nodes: Vec<MeteorologyNode>) -> Self {
        assert!(!nodes.is_empty(), "meteorology window without nodes");
        let mut maxima = [0u8; 3];
        for node in &nodes {
            for period in 0..3 {
                for sector in 0..SECTORS {
                    assert!(node.p_percent[period][sector] <= 100, "p outside 0..100 percent");
                }
                maxima[period] = maxima[period].max(*node.p_percent[period].iter().max().unwrap());
                for band in 0..8 {
                    let (mean, variance) = (node.alpha_mean[period][band], node.alpha_variance[period][band]);
                    assert!(
                        mean.is_finite() && mean >= 0.0 && variance.is_finite() && variance >= 0.0,
                        "nonfinite or negative absorption moment"
                    );
                }
            }
        }
        Self::build(window, nodes, maxima.map(|p| f32::from(p) / 100.0))
    }

    fn build(window: RasterWindow, nodes: Vec<MeteorologyNode>, maximum_probability: [f32; 3]) -> Self {
        let alpha_min_db_per_km = window_alpha_min_db_per_km(&nodes);
        Self { window, nodes, maximum_probability, alpha_min_db_per_km }
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

    /// The window's absorption minima per band [dB/km]: the linear bound of the long-term
    /// A_atm up to the profile ceiling, sound for every interpolated (μ, σ²).
    pub fn alpha_min_db_per_km(&self) -> [f32; 8] {
        self.alpha_min_db_per_km
    }

    /// The receiver's weather: this window sampled at (latitude, longitude), widened to
    /// the propagation struct, with the window's extremes behind the relevance bound.
    pub fn receiver_weather(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<noise_compute::propagation::meteorology::Meteorology, String> {
        use noise_compute::propagation::{air_absorption::AbsorptionClimate, meteorology::Meteorology};
        let sample = self.at(latitude, longitude)?;
        Ok(Meteorology {
            favourable_probability: sample.p.map(|row| row.map(f64::from)),
            absorption: std::array::from_fn(|period| {
                std::array::from_fn(|band| AbsorptionClimate {
                    mean_db_per_km: f64::from(sample.alpha_mean[period][band]),
                    variance_db2_per_km2: f64::from(sample.alpha_variance[period][band]),
                })
            }),
            bound_probability_max: self.maximum_probability.map(f64::from),
            bound_alpha_min_db_per_km: self.alpha_min_db_per_km.map(f64::from),
        })
    }
}

/// Per band, a linear absorption slope α with α·d ≤ A_atm(d) for every distance to the
/// profile ceiling and every interpolated (μ, σ²): the unfloored line μ − c·σ²·ceiling,
/// linear in (μ, σ²) so minimal at a node. Where every node and period peaks inside the
/// ceiling, the peak-region line tightens it: the receiver's peak μ̄/(4c·r̄) stays above
/// μ_min/(4c·r_max) (the receiver's ratio stays under the nodes' maximum), so that over
/// the ceiling lower-bounds the running maximum everywhere too. Anywhere else the peak
/// line overshoots the unreached peak (Dublin's calm bands read 15 dB/km against a true
/// 0.4) and must not apply. Rounded down one float.
fn window_alpha_min_db_per_km(nodes: &[MeteorologyNode]) -> [f32; 8] {
    const C: f64 = std::f64::consts::LN_10 / 20.0;
    const CEILING_KM: f64 = noise_compute::propagation::relevance_bound::LINE_REACH_CEILING_M / 1000.0;
    std::array::from_fn(|band| {
        let mut linear_min = f64::INFINITY;
        let mut mean_min = f64::INFINITY;
        let mut ratio_max = 0.0f64;
        let mut all_past_peak = true;
        for node in nodes {
            for period in 0..3 {
                let mu = f64::from(node.alpha_mean[period][band]);
                let var = f64::from(node.alpha_variance[period][band]);
                linear_min = linear_min.min(mu - C * var * CEILING_KM);
                mean_min = mean_min.min(mu);
                if mu > 0.0 {
                    ratio_max = ratio_max.max(var / mu);
                }
                // d_peak = μ/(2c·σ²) inside the ceiling, without dividing by zero.
                all_past_peak = all_past_peak && var > 0.0 && mu <= 2.0 * C * CEILING_KM * var;
            }
        }
        let peak = if all_past_peak && ratio_max > 0.0 {
            mean_min / (4.0 * C * CEILING_KM * ratio_max)
        } else {
            0.0
        };
        (linear_min.max(peak).max(0.0) as f32).next_down().max(0.0)
    })
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

/// Per-square meteorology windows beside the prepared rasters, loaded once: the painter's
/// receiver weather (the popup samples the same windows through `RealRasters::weather`).
pub struct WeatherCache {
    root: PathBuf,
    /// By receiver square (`None` = missing or refused file, the long-standing defaults;
    /// warned once per square).
    windows: Mutex<HashMap<Square, Option<Arc<Meteorology>>>>,
}

impl WeatherCache {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf(), windows: Mutex::new(HashMap::new()) }
    }

    /// One receiver's weather: its own square's window sampled at the receiver, the
    /// long-standing defaults where the square has no (readable) file.
    pub fn weather(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> noise_compute::propagation::meteorology::Meteorology {
        use noise_compute::propagation::meteorology::Meteorology;
        let square = grid::square_of(latitude, longitude);
        self.window(square)
            .as_ref()
            .and_then(|window| window.receiver_weather(latitude, longitude).ok())
            .unwrap_or_else(Meteorology::defaults)
    }

    /// The bound extremes over squares: per-period p maxima, per-band α minima
    /// (a square without a file contributes the defaults, matching the receiver
    /// fallback). The painter's row envelope: every painted receiver samples one of
    /// these windows, so no receiver's bound outruns it.
    pub fn envelope_maxima(&self, squares: &[Square]) -> ([f64; 3], [f64; 8]) {
        use noise_compute::propagation::meteorology::Meteorology;
        let mut pmax = [0.0f64; 3];
        let mut amin = [f64::INFINITY; 8];
        for square in squares {
            let (p, a) = match self.window(*square) {
                Some(window) => (
                    window.maximum_probability().map(f64::from),
                    window.alpha_min_db_per_km().map(f64::from),
                ),
                None => {
                    let defaults = Meteorology::defaults();
                    (defaults.bound_probability_max, defaults.bound_alpha_min_db_per_km)
                }
            };
            for period in 0..3 {
                pmax[period] = pmax[period].max(p[period]);
            }
            for band in 0..8 {
                amin[band] = amin[band].min(a[band]);
            }
        }
        (pmax, amin)
    }

    fn window(&self, square: Square) -> Option<Arc<Meteorology>> {
        let mut windows = self.windows.lock().unwrap();
        if let Some(cached) = windows.get(&square) {
            return cached.clone();
        }
        let path = Meteorology::path(&self.root, square);
        let loaded = match Meteorology::load(&path, square) {
            Ok(window) => Some(Arc::new(window)),
            Err(error) => {
                if std::fs::metadata(&path).is_ok() {
                    eprintln!("raster-reader: REFUSED square {square:?}: {error}");
                }
                None
            }
        };
        windows.insert(square, loaded.clone());
        loaded
    }
}

#[cfg(test)]
#[path = "meteorology_tests.rs"]
mod tests;
