//! Convert manifest-selected publisher objects to the one structure-observation Arrow format.

#[path = "structure_attributes.rs"]
mod attributes;
#[path = "structure_footprint_spool.rs"]
mod footprint_spool;
#[path = "structure_footprints.rs"]
mod footprints;
#[path = "structure_parquet.rs"]
mod parquet_rows;
use parquet_rows::batches;
#[path = "structure_surveys.rs"]
mod surveys;
#[cfg(test)]
#[path = "structure_input_tests.rs"]
mod tests;

use crate::{
    dev4::Square,
    osm::geometry as grid,
    structures::{
        observations::{self, Observation},
        prepare_walls::SUPPORT_HALO_M,
    },
    year::{Kind, YearInputs},
};
use anyhow::{Result, ensure};
use arrow_array::{
    Array, Float32Array, Float64Array, Int32Array, Int64Array, LargeStringArray, RecordBatch,
    StringArray,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Objects {
    pub format: String,
    pub objects: Vec<Object>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub path: PathBuf,
    #[serde(default)]
    pub degree: Option<[i16; 2]>,
}

pub fn bounds(square: Square) -> [f64; 4] {
    let lat = |y: u32| {
        (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 512.0))
            .sinh()
            .atan()
            .to_degrees()
    };
    [
        f64::from(square.x) / 512.0 * 360.0 - 180.0,
        if square.y == 511 {
            -90.0
        } else {
            lat(square.y + 1)
        },
        f64::from(square.x + 1) / 512.0 * 360.0 - 180.0,
        if square.y == 0 { 90.0 } else { lat(square.y) },
    ]
}

pub fn owner(longitude: f64, latitude: f64) -> Square {
    let square = grid::square_of(
        latitude,
        crate::structures::prepare_geometry::normalize_longitude(longitude),
    );
    Square {
        x: u32::from(square.x),
        y: u32::from(square.y),
    }
}

pub fn selected(longitude: f64, latitude: f64, square: Square, halo: f64) -> (bool, bool) {
    let owned = owner(longitude, latitude) == square;
    if owned {
        return (true, true);
    }
    if halo == 0.0 {
        return (false, false);
    }
    let [west, south, east, north] = bounds(square);
    let dy = if (south..=north).contains(&latitude) {
        0.0
    } else {
        (latitude - south).abs().min((latitude - north).abs())
    };
    let x = crate::structures::prepare_geometry::wrapped_longitude_delta(west, longitude);
    let width = crate::structures::prepare_geometry::wrapped_longitude_delta(west, east);
    let dx = if (0.0..=width).contains(&x) {
        0.0
    } else {
        x.abs().min((x - width).abs())
    };
    let metres = 111_320.0 * latitude.to_radians().cos().max(0.01);
    ((dy * 111_320.0).hypot(dx * metres) <= halo, false)
}

/// Halos under this reach only a point's own square and its eight neighbours: half a column
/// (360/512 degrees) at `selected`'s cosine floor of 0.01, the other half a margin for the
/// rounding of a longitude on a column edge; a row spans at least 6.7 km.
pub const NEIGHBOUR_HALO_LIMIT_M: f64 = 360.0 / 512.0 * 111_320.0 * 0.01 / 2.0;
const _: () = assert!(SUPPORT_HALO_M < NEIGHBOUR_HALO_LIMIT_M);

/// The indices in `index` of the squares `selected` admits a point to, each with whether it owns
/// the point. Testing the owner and its neighbours instead of every square keeps a whole-world
/// selection linear in its points.
pub fn selected_squares(
    longitude: f64,
    latitude: f64,
    halo: f64,
    index: &std::collections::HashMap<Square, usize>,
) -> Vec<(usize, bool)> {
    debug_assert!(halo < NEIGHBOUR_HALO_LIMIT_M);
    let home = owner(longitude, latitude);
    let mut chosen: Vec<(usize, bool)> = index.get(&home).map(|&i| (i, true)).into_iter().collect();
    if halo > 0.0 {
        for square in home.with_neighbours() {
            if square != home
                && let Some(&i) = index.get(&square)
                && selected(longitude, latitude, square, halo).0
            {
                chosen.push((i, false));
            }
        }
    }
    chosen
}

/// Each selected square's position in the sorted selection.
pub fn square_index(squares: &[Square]) -> std::collections::HashMap<Square, usize> {
    squares.iter().enumerate().map(|(i, &s)| (s, i)).collect()
}

pub fn touched_degrees(square: Square, halo: f64) -> Vec<[i16; 2]> {
    let [mut west, mut south, mut east, mut north] = bounds(square);
    let latitude_pad = halo / 111_320.0;
    let longitude_pad = latitude_pad / ((south + north) / 2.0).to_radians().cos().max(0.01);
    south = (south - latitude_pad).max(-90.0);
    north = (north + latitude_pad).min(90.0);
    west -= longitude_pad;
    east += longitude_pad;
    let mut degrees = Vec::new();
    for latitude in south.floor() as i16..(north.floor() as i16 + 1).min(90) {
        for longitude in west.floor() as i16..east.ceil() as i16 {
            degrees.push([latitude, (longitude + 180).rem_euclid(360) - 180]);
        }
    }
    degrees
}

pub fn number(batch: &RecordBatch, name: &str, row: usize) -> Option<f64> {
    let c = batch.column_by_name(name)?;
    if c.is_null(row) {
        return None;
    }
    if let Some(a) = c.as_any().downcast_ref::<Float64Array>() {
        return Some(a.value(row));
    }
    if let Some(a) = c.as_any().downcast_ref::<Float32Array>() {
        return Some(f64::from(a.value(row)));
    }
    if let Some(a) = c.as_any().downcast_ref::<Int32Array>() {
        return Some(f64::from(a.value(row)));
    }
    c.as_any()
        .downcast_ref::<Int64Array>()
        .map(|a| a.value(row) as f64)
}

pub fn text<'a>(batch: &'a RecordBatch, name: &str, row: usize) -> Option<&'a str> {
    let c = batch.column_by_name(name)?;
    if c.is_null(row) {
        return None;
    }
    if let Some(a) = c.as_any().downcast_ref::<StringArray>() {
        return Some(a.value(row));
    }
    c.as_any()
        .downcast_ref::<LargeStringArray>()
        .map(|a| a.value(row))
}

pub fn convert(inputs: &YearInputs, squares: &[Square], out: &Path) -> Result<()> {
    ensure!(!out.exists(), "normalized structures output must be new");
    let mut squares = squares.to_vec();
    squares.sort_unstable();
    squares.dedup();
    ensure!(
        !squares.is_empty() && squares.iter().all(|s| s.x < 512 && s.y < 512),
        "invalid structure square selection"
    );
    let mut rows: Vec<Vec<Observation>> = squares.iter().map(|_| vec![]).collect();
    let mut footprints = Vec::new();
    for kind in [
        Kind::PrepareBuildingFootprints,
        Kind::PrepareBuildingHeights,
        Kind::PrepareNoiseBarriers,
        Kind::PrepareBuildingAttributes,
    ] {
        let mut survey_sources = Vec::new();
        for path in inputs
            .structure_observation_sources(kind)
            .map_err(anyhow::Error::msg)?
        {
            let source: Objects = serde_json::from_slice(&std::fs::read(path)?)?;
            ensure!(
                !source.objects.is_empty(),
                "{}: empty source object inventory",
                path.display()
            );
            for object in &source.objects {
                ensure!(
                    !object.path.is_absolute()
                        && object
                            .path
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_))),
                    "source objects need safe relative paths"
                );
            }
            match (kind, source.format.as_str()) {
                (Kind::PrepareBuildingFootprints, "overture") => {
                    footprints.push(footprints::convert(
                        &source,
                        path.parent().unwrap(),
                        &squares,
                        out,
                        &inputs.manifest.year,
                    )?);
                }
                (Kind::PrepareBuildingHeights, "measured-heights")
                | (Kind::PrepareNoiseBarriers, "official-walls") => {
                    survey_sources.push((source, path.parent().unwrap().to_owned()));
                }
                (Kind::PrepareBuildingAttributes, "ruian" | "catastro") => {
                    attributes::convert(&source, path.parent().unwrap(), &squares, &mut rows)?
                }
                _ => anyhow::bail!("{kind}: unsupported publisher format {}", source.format),
            }
        }
        if !survey_sources.is_empty() {
            surveys::convert(
                &survey_sources,
                &squares,
                &mut rows,
                kind == Kind::PrepareNoiseBarriers,
            )?;
        }
    }
    for (index, (square, attributes)) in squares.into_iter().zip(rows).enumerate() {
        let mut rows = Vec::new();
        for source in &mut footprints {
            rows.extend(source.take(index)?);
        }
        rows.extend(attributes);
        let path = out.join(format!(
            "z9/{}/{}/structure-observations.arrow",
            square.x, square.y
        ));
        observations::write(&path, &rows, &inputs.manifest.year)?;
        eprintln!(
            "structure-observations: {}:{}: {} observations",
            square.x,
            square.y,
            rows.len()
        );
    }
    Ok(())
}
