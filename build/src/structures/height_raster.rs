//! Measured relative-height rasters: footprint pixel centres, coverage guard and mean roof height.

use anyhow::{Result, ensure};
use gdal::{
    Dataset,
    spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef},
};
use geos::{Geom, Geometry};
use std::path::Path;

/// The lowest relative height that is a roof (the frozen producer's pixel rule): a raster pixel or
/// a surveyed footprint below it is ground, vegetation or a record without a height.
pub const MEASURED_ROOF_MINIMUM_M: f32 = 2.0;

pub struct HeightRaster {
    dataset: Dataset,
    transform: CoordTransform,
    grid: [f64; 6],
    dimensions: (usize, usize),
    nodata: Option<f64>,
    coverage: [f64; 4],
}

impl HeightRaster {
    pub fn open(path: &Path) -> Result<Self> {
        let dataset = Dataset::open(std::fs::canonicalize(path)?)?;
        let mut source = SpatialRef::from_epsg(4326)?;
        let mut target = dataset.spatial_ref()?;
        source.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        target.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        let transform = CoordTransform::new(&source, &target)?;
        let grid = dataset.geo_transform()?;
        ensure!(
            grid[1] > 0.0 && grid[5] < 0.0 && grid[2] == 0.0 && grid[4] == 0.0,
            "height raster must be north-up"
        );
        let dimensions = dataset.raster_size();
        let coverage = CoordTransform::new(&target, &source)?.transform_bounds(
            &[
                grid[0],
                grid[3] + dimensions.1 as f64 * grid[5],
                grid[0] + dimensions.0 as f64 * grid[1],
                grid[3],
            ],
            21,
        )?;
        let nodata = dataset.rasterband(1)?.no_data_value();
        Ok(Self {
            dataset,
            transform,
            grid,
            dimensions,
            nodata,
            coverage,
        })
    }

    fn projected(&self, longitude: f64, latitude: f64) -> Result<(f64, f64)> {
        let (mut x, mut y) = ([longitude], [latitude]);
        self.transform.transform_coords(&mut x, &mut y, &mut [])?;
        Ok((x[0], y[0]))
    }

    pub fn covers(&self, longitude: f64, latitude: f64) -> bool {
        let b = self.coverage;
        longitude >= b[0] && longitude <= b[2] && latitude >= b[1] && latitude <= b[3]
    }

    /// Whether its coverage meets a west, south, east, north box in degrees.
    pub fn overlaps(&self, [west, south, east, north]: [f64; 4]) -> bool {
        let b = self.coverage;
        west <= b[2] && b[0] <= east && south <= b[3] && b[1] <= north
    }

    pub fn sample(
        &self,
        geometry: &Geometry,
        longitude: f64,
        latitude: f64,
    ) -> Result<Option<f64>> {
        if !self.covers(longitude, latitude) {
            return Ok(None);
        }
        let (x, y) = self.projected(longitude, latitude)?;
        let g = self.grid;
        let (width, height) = self.dimensions;
        let column = (x - g[0]) / g[1];
        let row = (y - g[3]) / g[5];
        if !(0.0..width as f64).contains(&column) || !(0.0..height as f64).contains(&row) {
            return Ok(None);
        }
        let projected = geometry.transform_xy(|x, y| self.projected(x, y))?;
        let c0 = (((projected.get_x_min()? - g[0]) / g[1]).floor() as isize).max(0) as usize;
        let c1 = ((((projected.get_x_max()? - g[0]) / g[1]).ceil() as isize) + 1)
            .clamp(0, width as isize) as usize;
        let r0 = (((projected.get_y_max()? - g[3]) / g[5]).floor() as isize).max(0) as usize;
        let r1 = ((((projected.get_y_min()? - g[3]) / g[5]).ceil() as isize) + 1)
            .clamp(0, height as isize) as usize;
        if c1 <= c0 || r1 <= r0 || (c1 - c0).saturating_mul(r1 - r0) > 16_000_000 {
            return Ok(None);
        }
        let size = (c1 - c0, r1 - r0);
        let window = self.dataset.rasterband(1)?.read_as::<f32>(
            (c0 as isize, r0 as isize),
            size,
            size,
            None,
        )?;
        let prepared = projected.to_prepared_geom()?;
        let mut inside = 0;
        let mut measured = Vec::new();
        for row in r0..r1 {
            let y = g[3] + (row as f64 + 0.5) * g[5];
            for col in c0..c1 {
                let x = g[0] + (col as f64 + 0.5) * g[1];
                if !prepared.contains_xy(x, y)? {
                    continue;
                }
                inside += 1;
                let value = window.data()[(row - r0) * size.0 + col - c0];
                if value.is_finite()
                    && value >= MEASURED_ROOF_MINIMUM_M
                    && self.nodata != Some(f64::from(value))
                {
                    measured.push(value);
                }
            }
        }
        if measured.len() < 3 || (measured.len() as f64) < 0.30 * inside as f64 {
            return Ok(None);
        }
        Ok(Some(f64::from(
            pairwise_sum(&measured) / (measured.len() as f32),
        )))
    }
}

// NumPy's contiguous float32 reduction, used by the frozen producer before rounding to metres.
fn pairwise_sum(values: &[f32]) -> f32 {
    if values.len() < 8 {
        return values.iter().fold(-0.0, |sum, v| sum + v);
    }
    if values.len() <= 128 {
        let mut lanes: [f32; 8] = values[..8].try_into().unwrap();
        let end = values.len() - values.len() % 8;
        for chunk in values[8..end].chunks_exact(8) {
            for i in 0..8 {
                lanes[i] += chunk[i];
            }
        }
        let sum = ((lanes[0] + lanes[1]) + (lanes[2] + lanes[3]))
            + ((lanes[4] + lanes[5]) + (lanes[6] + lanes[7]));
        return values[end..].iter().fold(sum, |sum, v| sum + v);
    }
    let middle = values.len() / 2 - (values.len() / 2) % 8;
    pairwise_sum(&values[..middle]) + pairwise_sum(&values[middle..])
}
