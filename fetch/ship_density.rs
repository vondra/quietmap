//! Sum density in f64 only for positive cells while retaining zero-density coverage of every raster.
use super::grid::Coverage;
use super::ship_raster::{self, Raster, relative_path};
use super::{Class, DensityPart, MIN_MONTHLY_HOURS, Normalized, observation};
use std::collections::HashSet;
use std::path::Path;
use tiles::square::Square;

pub(super) fn read(
    epsg: u16,
    parts: &[DensityPart],
    source_id: u16,
    root: &Path,
    squares: &HashSet<Square>,
) -> Result<Normalized, String> {
    let mut geometry = None;
    let mut indices = Vec::new();
    let mut hours: Vec<[f64; 3]> = Vec::new();
    let mut mask = Vec::new();
    for part in parts {
        let raster = Raster::read(ship_raster::bytes(
            &relative_path(root, &part.path)?,
            &part.member,
        )?)?;
        if raster.geometry.epsg != epsg {
            return Err(format!("{}: unexpected raster CRS", part.path));
        }
        if let Some(geometry) = &geometry {
            if *geometry != raster.geometry {
                return Err(format!("{}: density grids differ", part.path));
            }
        } else {
            indices = vec![u32::MAX; raster.valid.len()];
            mask = vec![false; raster.valid.len()];
            geometry = Some(raster.geometry.clone());
        }
        for index in 0..raster.valid.len() {
            mask[index] |= raster.valid[index];
            let value = raster.value(index);
            if raster.valid[index] && value > 0.0 {
                if indices[index] == u32::MAX {
                    indices[index] = u32::try_from(hours.len()).map_err(|e| e.to_string())?;
                    hours.push([0.0; 3]);
                }
                hours[indices[index] as usize][Class::index(part.class)] += value;
            }
        }
    }
    let coverage = Coverage::new(geometry.ok_or("empty density parts")?, mask)?;
    let mut observations = Vec::new();
    for (index, position) in indices.into_iter().enumerate() {
        if position == u32::MAX {
            continue;
        }
        let hours = hours[position as usize];
        if hours.iter().sum::<f64>() < MIN_MONTHLY_HOURS {
            continue;
        }
        let (longitude, latitude) = coverage.centre(index)?;
        if let Some(cell) = observation(
            longitude,
            latitude,
            coverage.grid.area_m2(latitude),
            hours,
            source_id,
            squares,
        ) {
            observations.push(cell);
        }
    }
    Ok(Normalized {
        observations,
        grid: coverage.grid.native.clone(),
        coverage: Some(coverage),
    })
}
