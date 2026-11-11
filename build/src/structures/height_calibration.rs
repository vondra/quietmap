//! Fit the common height table from the year's surveyed roofs (rasters and footprint surveys) and mapped floors.

use super::{
    height_model::HeightModel,
    height_raster::{HeightRaster, MEASURED_ROOF_MINIMUM_M},
    observations::{self, HEIGHT, Observation},
    prepare::surveyed_roof,
    prepare_geometry::{self as geometry, FootprintIndex},
    prepare_rows,
};
use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::{collections::BTreeSet, io::Write, path::Path};

pub(crate) fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// A mapped building with a surveyed roof: its OSM outline's area, the roof, its mapped floors and
/// whether OSM maps its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Sample {
    pub area_m2: f64,
    pub roof_m: f64,
    pub floors: u8,
    pub mapped_height: bool,
}

/// The surveyed buildings and mapped wall heights of one OSM square. Each building's roof is the
/// one structures would take as measured (`surveyed_roof`, its own outline); a roof below the
/// measured minimum is no roof, so a survey's empty records cannot set the table's low bound.
pub(super) fn square_samples(
    osm_square: &Path,
    surveys: &[Observation],
    rasters: &[HeightRaster],
) -> Result<(Vec<Sample>, Vec<f64>)> {
    let index = FootprintIndex::new(
        surveys
            .iter()
            .map(Observation::geometry)
            .collect::<Result<_>>()?,
    )?;
    let walls = prepare_rows::read_walls(&osm_square.join("barriers.arrow"))?
        .into_iter()
        .filter(|wall| wall.mapped && wall.height.is_finite() && wall.height > 0.0)
        .map(|wall| f64::from(wall.height))
        .collect();
    let mut samples = Vec::new();
    for building in prepare_rows::read_osm(&osm_square.join("buildings.arrow"))? {
        if building.ground_activity || building.building_use == 3 {
            continue;
        }
        let Some(ring) = building.row.emission_geometry.as_deref() else {
            continue;
        };
        let Some(polygon) = geometry::ring_geometry(ring)? else {
            continue;
        };
        let (longitude, latitude) =
            geometry::grid_to_lonlat(building.row.centroid.0, building.row.centroid.1);
        let Some(roof_m) = surveyed_roof(&polygon, longitude, latitude, rasters, &index, surveys)?
            .filter(|roof| *roof >= f64::from(MEASURED_ROOF_MINIMUM_M))
        else {
            continue;
        };
        samples.push(Sample {
            area_m2: geometry::area_m2(&polygon, latitude)?,
            roof_m,
            floors: building.row.floors.unwrap_or(0),
            mapped_height: building
                .row
                .height
                .is_some_and(|h| h.is_finite() && h > 0.0),
        });
    }
    Ok((samples, walls))
}

/// The height table of the surveyed buildings and mapped walls, and the counts behind it: least
/// squares of roofs on mapped floors, five area quantiles of buildings mapping neither height nor
/// floors with their median roofs, and the median mapped wall.
pub(super) fn fitted_model(samples: &[Sample], wall_heights: &mut [f64]) -> Result<(Value, Value)> {
    let floors: Vec<_> = samples.iter().filter(|s| s.floors > 0).collect();
    ensure!(
        floors.len() >= 100 && wall_heights.len() >= 100,
        "insufficient measured floors or walls"
    );
    let count = floors.len() as f64;
    let mean_floor = floors.iter().map(|s| f64::from(s.floors)).sum::<f64>() / count;
    let mean_height = floors.iter().map(|s| s.roof_m).sum::<f64>() / count;
    let covariance = floors
        .iter()
        .map(|s| (f64::from(s.floors) - mean_floor) * (s.roof_m - mean_height))
        .sum::<f64>();
    let variance = floors
        .iter()
        .map(|s| (f64::from(s.floors) - mean_floor).powi(2))
        .sum::<f64>();
    let slope = covariance / variance;
    let roof = mean_height - slope * mean_floor;
    ensure!(
        slope.is_finite() && slope > 0.0 && slope + roof > 0.0,
        "floor measurements do not identify a positive height model"
    );
    let mut unknown: Vec<_> = samples
        .iter()
        .filter(|s| s.floors == 0 && !s.mapped_height)
        .collect();
    unknown.sort_by(|a, b| a.area_m2.total_cmp(&b.area_m2));
    ensure!(
        unknown.len() >= 500,
        "insufficient untagged footprints for area medians"
    );
    let bounds: Vec<_> = (1..5)
        .map(|i| unknown[unknown.len() * i / 5].area_m2)
        .collect();
    let mut bins: [Vec<f64>; 5] = std::array::from_fn(|_| vec![]);
    for sample in &unknown {
        let bin = bounds.iter().position(|b| sample.area_m2 < *b).unwrap_or(4);
        bins[bin].push(sample.roof_m);
    }
    ensure!(
        bins.iter().all(|b| !b.is_empty()),
        "duplicate area quantiles"
    );
    let counts: Vec<_> = bins.iter().map(Vec::len).collect();
    let heights: Vec<_> = bins.iter_mut().map(|b| median(b)).collect();
    let wall_median = median(wall_heights);
    let minimum_roof = samples
        .iter()
        .map(|s| s.roof_m)
        .min_by(f64::total_cmp)
        .unwrap();
    // The low-height cutoff comes from admitted roofs; the upper bound is the output representation.
    let value = json!({"measured_bounds_m":[minimum_roof,f64::from(i16::MAX)],"overture_minimum_m":minimum_roof,
        "area_upper_m2":bounds,"area_height_m":heights,
        "floor_ranges":[{"through":255,"slope_m":slope,"intercept_m":roof}],
        "storey_height_m":slope,"storey_roof_m":roof,
        "wall_default_m":wall_median,"wall_country_m":{}});
    serde_json::from_value::<HeightModel>(value.clone())?.validate()?;
    let support = json!({"measured_buildings":samples.len(),"floor_pairs":floors.len(),
        "untagged_buildings":unknown.len(),"area_bin_counts":counts,"mapped_walls":wall_heights.len(),
        "floor_slope_m":slope,"roof_m":roof,"wall_median_m":wall_median,"minimum_roof_m":minimum_roof});
    Ok((value, support))
}

/// The selected squares a survey can reach: those meeting a raster's coverage or touching a degree
/// tile of an admitted footprint survey, whose rows structure-observations gives to these squares.
fn surveyed_squares(
    inputs: &YearInputs,
    rasters: &[HeightRaster],
    squares: &[Square],
) -> Result<Vec<Square>> {
    let mut degrees = BTreeSet::new();
    for path in inputs
        .structure_observation_sources(Kind::PrepareBuildingHeights)
        .map_err(anyhow::Error::msg)?
    {
        let source: crate::structure_inputs::Objects =
            serde_json::from_slice(&std::fs::read(path)?)?;
        for object in source.objects {
            degrees.insert(
                object
                    .degree
                    .ok_or_else(|| anyhow!("{}: survey object needs its degree", path.display()))?,
            );
        }
    }
    Ok(squares
        .iter()
        .copied()
        .filter(|&square| {
            let bounds = crate::structure_inputs::bounds(square);
            rasters.iter().any(|raster| raster.overlaps(bounds))
                || crate::structure_inputs::touched_degrees(square, 0.0)
                    .iter()
                    .any(|degree| degrees.contains(degree))
        })
        .collect())
}

/// Fit the year's height table over the selected squares that hold survey data and write it with
/// its support (`<out>.samples.json`).
pub fn fit(inputs: &YearInputs, squares: &[Square], output: &Path) -> Result<()> {
    ensure!(!output.exists(), "height calibration output must be new");
    let osm = inputs
        .required(Kind::OsmGeography)
        .map_err(anyhow::Error::msg)?;
    let observed = inputs
        .required(Kind::StructureObservations)
        .map_err(anyhow::Error::msg)?;
    let raster_paths = inputs
        .structure_height_rasters()
        .map_err(anyhow::Error::msg)?;
    let open_rasters = || {
        raster_paths
            .iter()
            .map(|p| HeightRaster::open(p))
            .collect::<Result<Vec<_>>>()
    };
    let mut selected = squares.to_vec();
    selected.sort_unstable();
    selected.dedup();
    let surveyed = surveyed_squares(inputs, &open_rasters()?, &selected)?;
    let year = &inputs.manifest.year;
    // Each worker opens its own rasters: a GDAL dataset serves one thread.
    let per_square = surveyed
        .par_iter()
        .map_init(open_rasters, |rasters, square| {
            let rasters = rasters.as_ref().map_err(|e| anyhow!("{e:#}"))?;
            let name = format!("z9/{}/{}", square.x, square.y);
            let directory = osm.join(&name);
            ensure!(
                directory.join("square-country-city.bin").is_file(),
                "{name}: missing calibration OSM square"
            );
            let surveys: Vec<_> = observations::read(
                &observed.join(&name).join("structure-observations.arrow"),
                year,
            )?
            .into_iter()
            .filter(|row| row.kind == HEIGHT)
            .collect();
            square_samples(&directory, &surveys, rasters)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut samples = Vec::new();
    let mut walls = Vec::new();
    let mut support_squares = Vec::new();
    for (square, (square_samples, square_walls)) in surveyed.iter().zip(per_square) {
        support_squares.push([
            square.x,
            square.y,
            square_samples.len() as u32,
            square_walls.len() as u32,
        ]);
        samples.extend(square_samples);
        walls.extend(square_walls);
    }
    let (table, mut support) = fitted_model(&samples, &mut walls)?;
    support["year"] = json!(year);
    // Each surveyed square: x, y, its surveyed buildings and its mapped walls.
    support["squares"] = json!(support_squares);
    let mut file = std::fs::File::options()
        .create_new(true)
        .write(true)
        .open(output)?;
    serde_json::to_writer_pretty(&mut file, &table)?;
    file.write_all(b"\n")?;
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&support)?,
    )?;
    eprintln!(
        "height-calibration: {} surveyed squares of {}; {} measured buildings, {} floor pairs, {} untagged, {} mapped walls",
        surveyed.len(),
        selected.len(),
        samples.len(),
        support["floor_pairs"],
        support["untagged_buildings"],
        walls.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "height_calibration_tests.rs"]
mod tests;
