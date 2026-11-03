//! Fit the common height table from admitted footprint measurements and mapped floor counts.

use super::{
    height_model::HeightModel, height_raster::HeightRaster, prepare_geometry as geometry,
    prepare_rows,
};
use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{io::Write, path::Path};

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

pub fn fit(inputs: &YearInputs, squares: &[Square], output: &Path) -> Result<()> {
    ensure!(!output.exists(), "height calibration output must be new");
    let osm = inputs
        .required(Kind::OsmGeography)
        .map_err(anyhow::Error::msg)?;
    let rasters = inputs
        .structure_height_rasters()
        .map_err(anyhow::Error::msg)?
        .iter()
        .map(|p| HeightRaster::open(p))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !rasters.is_empty(),
        "height calibration requires measured heights"
    );
    let mut samples = Vec::new();
    let mut wall_heights = Vec::new();
    let mut selected = squares.to_vec();
    selected.sort_unstable();
    selected.dedup();
    for square in &selected {
        let directory = osm.join(format!("z9/{}/{}", square.x, square.y));
        ensure!(
            directory.join("square-country-city.bin").is_file(),
            "missing calibration OSM square"
        );
        for wall in prepare_rows::read_walls(&directory.join("barriers.arrow"))? {
            if wall.mapped && wall.height.is_finite() && wall.height > 0.0 {
                wall_heights.push(f64::from(wall.height));
            }
        }
        for building in prepare_rows::read_osm(&directory.join("buildings.arrow"))? {
            if building.ground_activity || building.building_use == 3 {
                continue;
            }
            let Some(ring) = building.row.emission_geometry.as_deref() else {
                continue;
            };
            let (longitude, latitude) =
                geometry::grid_to_lonlat(building.row.centroid.0, building.row.centroid.1);
            if !rasters.iter().any(|r| r.covers(longitude, latitude)) {
                continue;
            }
            let Some(polygon) = geometry::ring_geometry(ring)? else {
                continue;
            };
            let mut measured = None;
            for raster in &rasters {
                if let Some(height) = raster.sample(&polygon, longitude, latitude)? {
                    measured = Some(height);
                }
            }
            if let Some(height) = measured {
                samples.push((
                    geometry::area_m2(&polygon, latitude)?,
                    height,
                    building.row.floors.unwrap_or(0),
                    building
                        .row
                        .height
                        .is_some_and(|h| h.is_finite() && h > 0.0),
                ));
            }
        }
        eprintln!(
            "height-calibration: {}:{}: cumulative {} measured buildings, {} mapped walls",
            square.x,
            square.y,
            samples.len(),
            wall_heights.len()
        );
    }
    let floors: Vec<_> = samples.iter().filter(|s| s.2 > 0).collect();
    ensure!(
        floors.len() >= 100 && wall_heights.len() >= 100,
        "insufficient measured floors or walls"
    );
    let count = floors.len() as f64;
    let mean_floor = floors.iter().map(|s| f64::from(s.2)).sum::<f64>() / count;
    let mean_height = floors.iter().map(|s| s.1).sum::<f64>() / count;
    let covariance = floors
        .iter()
        .map(|s| (f64::from(s.2) - mean_floor) * (s.1 - mean_height))
        .sum::<f64>();
    let variance = floors
        .iter()
        .map(|s| (f64::from(s.2) - mean_floor).powi(2))
        .sum::<f64>();
    let slope = covariance / variance;
    let roof = mean_height - slope * mean_floor;
    ensure!(
        slope.is_finite() && slope > 0.0 && slope + roof > 0.0,
        "floor measurements do not identify a positive height model"
    );
    let mut unknown: Vec<_> = samples.iter().filter(|s| s.2 == 0 && !s.3).collect();
    unknown.sort_by(|a, b| a.0.total_cmp(&b.0));
    ensure!(
        unknown.len() >= 500,
        "insufficient untagged footprints for area medians"
    );
    let bounds: Vec<_> = (1..5).map(|i| unknown[unknown.len() * i / 5].0).collect();
    let mut bins: [Vec<f64>; 5] = std::array::from_fn(|_| vec![]);
    for sample in &unknown {
        let bin = bounds.iter().position(|b| sample.0 < *b).unwrap_or(4);
        bins[bin].push(sample.1);
    }
    ensure!(
        bins.iter().all(|b| !b.is_empty()),
        "duplicate area quantiles"
    );
    let counts: Vec<_> = bins.iter().map(Vec::len).collect();
    let heights: Vec<_> = bins.iter_mut().map(|b| median(b)).collect();
    let wall_mean = median(&mut wall_heights);
    let minimum_roof = samples.iter().map(|s| s.1).min_by(f64::total_cmp).unwrap();
    // The low-height cutoff comes from admitted roofs; the upper bound is the output representation.
    let value = json!({"measured_bounds_m":[minimum_roof,f64::from(i16::MAX)],"overture_minimum_m":minimum_roof,
        "area_upper_m2":bounds,"area_height_m":heights,
        "floor_ranges":[{"through":255,"slope_m":slope,"intercept_m":roof}],
        "storey_height_m":slope,"storey_roof_m":roof,
        "wall_default_m":wall_mean,"wall_country_m":{}});
    serde_json::from_value::<HeightModel>(value.clone())?.validate()?;
    let mut file = std::fs::File::options()
        .create_new(true)
        .write(true)
        .open(output)?;
    serde_json::to_writer_pretty(&mut file, &value)?;
    file.write_all(b"\n")?;
    eprintln!(
        "height-calibration receipt: {}",
        json!({"measured_buildings":samples.len(),
        "squares":selected.iter().map(|s| format!("{}:{}",s.x,s.y)).collect::<Vec<_>>(),
        "year":inputs.manifest.year,"floor_pairs":floors.len(),"untagged_buildings":unknown.len(),"area_bin_counts":counts,
        "mapped_walls":wall_heights.len(),"floor_slope_m":slope,"roof_m":roof,"wall_median_m":wall_mean,"minimum_roof_m":minimum_roof})
    );
    Ok(())
}
