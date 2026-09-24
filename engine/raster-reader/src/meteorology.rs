//! Strict global ERA5 climatology loading and continuous receiver/direction sampling.
use arrow::{
    array::{Array, FixedSizeListArray, Float32Array, UInt16Array, UInt8Array},
    ipc::reader::FileReader,
};
use std::{fs::File, path::Path};

pub const ROWS: usize = 721;
pub const COLUMNS: usize = 1440;
pub const SECTORS: usize = 16;
const CONTRACT: &str = include_str!("../../noise-compute/meteorology-contract.json");

/// CUDA transfer layout: 48 probability bytes, then two 24-float moment arrays.
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
    nodes: Vec<MeteorologyNode>,
    maximum_probability: [f32; 3],
}

impl Meteorology {
    /// Reject incomplete normals, missing cells, reordered rows, nulls and nonphysical values.
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::read(path).map_err(|error| format!("meteorology {}: {error}", path.display()))
    }

    fn read(path: &Path) -> Result<Self, String> {
        let reader = FileReader::try_new(File::open(path).map_err(|e| e.to_string())?, None)
            .map_err(|e| e.to_string())?;
        let schema = reader.schema();
        let expected: std::collections::HashMap<String, String> =
            serde_json::from_str(CONTRACT).unwrap();
        for (key, value) in expected {
            if schema.metadata().get(&key) != Some(&value) {
                return Err(format!("wrong or missing {key} metadata"));
            }
        }
        if schema.metadata().get("complete").map(String::as_str) != Some("true")
            || schema
                .metadata()
                .get("source_identity")
                .is_none_or(String::is_empty)
        {
            return Err("incomplete climatology or missing source identity".into());
        }
        let mut nodes = Vec::with_capacity(ROWS * COLUMNS);
        let mut maxima = [0u8; 3];
        for batch in reader {
            let batch = batch.map_err(|e| e.to_string())?;
            let column = |name: &str| -> Result<&dyn Array, String> {
                let array = batch
                    .column_by_name(name)
                    .ok_or_else(|| format!("missing {name}"))?;
                if array.null_count() != 0 {
                    return Err(format!("null {name}"));
                }
                Ok(array.as_ref())
            };
            let xs = column("x")?
                .as_any()
                .downcast_ref::<UInt16Array>()
                .ok_or("x must be u16")?;
            let ys = column("y")?
                .as_any()
                .downcast_ref::<UInt16Array>()
                .ok_or("y must be u16")?;
            let begin = nodes.len();
            for row in 0..batch.num_rows() {
                let index = begin + row;
                if index >= ROWS * COLUMNS
                    || usize::from(xs.value(row)) != index % COLUMNS
                    || usize::from(ys.value(row)) != index / COLUMNS
                {
                    return Err(
                        "rows must cover the global ERA5 grid once in row-major order".into(),
                    );
                }
                nodes.push(MeteorologyNode::default());
            }
            for (period, name) in ["day", "evening", "night"].iter().enumerate() {
                let p = list(column(&format!("p_{name}"))?, SECTORS)?;
                let probabilities = p
                    .values()
                    .as_any()
                    .downcast_ref::<UInt8Array>()
                    .ok_or("p must be u8")?;
                let maximum_column = column(&format!("p_max_{name}"))?;
                let maximum = maximum_column
                    .as_any()
                    .downcast_ref::<UInt8Array>()
                    .ok_or("p_max must be u8")?;
                let mean = list(column(&format!("alpha_mean_{name}"))?, 8)?;
                let variance = list(column(&format!("alpha_variance_{name}"))?, 8)?;
                let mean_values = mean
                    .values()
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or("alpha mean must be f32")?;
                let variance_values = variance
                    .values()
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or("alpha variance must be f32")?;
                for row in 0..batch.num_rows() {
                    let node = &mut nodes[begin + row];
                    let p_offset = p.value_offset(row) as usize;
                    for sector in 0..SECTORS {
                        let value = probabilities.value(p_offset + sector);
                        if value > 100 {
                            return Err("p outside 0..100 percent".into());
                        }
                        node.p_percent[period][sector] = value;
                    }
                    let max = *node.p_percent[period].iter().max().unwrap();
                    if maximum.value(row) != max {
                        return Err("p_max disagrees with sectors".into());
                    }
                    maxima[period] = maxima[period].max(max);
                    for band in 0..8 {
                        let mu = mean_values.value(mean.value_offset(row) as usize + band);
                        let var = variance_values.value(variance.value_offset(row) as usize + band);
                        if !mu.is_finite() || mu < 0.0 || !var.is_finite() || var < 0.0 {
                            return Err("nonfinite or negative absorption moment".into());
                        }
                        node.alpha_mean[period][band] = mu;
                        node.alpha_variance[period][band] = var;
                    }
                }
            }
        }
        if nodes.len() != ROWS * COLUMNS {
            return Err("incomplete global meteorology grid".into());
        }
        Ok(Self {
            nodes,
            maximum_probability: maxima.map(|p| f32::from(p) / 100.0),
        })
    }

    pub fn at(&self, latitude: f64, longitude: f64) -> Result<MeteorologySample, String> {
        sample_at(latitude, longitude, |x, y| self.nodes[y * COLUMNS + x])
    }

    /// Conservatively bounds every interpolation and every receiver influence halo.
    pub fn maximum_probability(&self) -> [f32; 3] {
        self.maximum_probability
    }

    pub fn nodes(&self) -> &[MeteorologyNode] {
        &self.nodes
    }
}

fn list(array: &dyn Array, length: usize) -> Result<&FixedSizeListArray, String> {
    let list = array
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .ok_or("expected fixed-size list")?;
    if list.value_length() as usize != length || list.values().null_count() != 0 {
        return Err("wrong list length or null child value".into());
    }
    Ok(list)
}

fn sample_at(
    latitude: f64,
    longitude: f64,
    node: impl Fn(usize, usize) -> MeteorologyNode,
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
            let n = node(xx, yy);
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
