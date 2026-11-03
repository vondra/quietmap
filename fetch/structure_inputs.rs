//! Convert manifest-selected publisher objects to the one structure-observation Arrow format.

#[path = "structure_attributes.rs"]
mod attributes;
#[path = "structure_footprints.rs"]
mod footprints;
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
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::Deserialize;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

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

pub fn selected(longitude: f64, latitude: f64, square: Square, halo: f64) -> (bool, bool) {
    let owned = grid::square_of(
        latitude,
        crate::structures::prepare_geometry::normalize_longitude(longitude),
    ) == grid::Square {
        x: square.x as u16,
        y: square.y as u16,
    };
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

pub fn batches(
    path: &Path,
    columns: &[&str],
    selection: Option<&[Square]>,
    contract: Option<(&str, &str)>,
    mut visit: impl FnMut(RecordBatch) -> Result<()>,
) -> Result<()> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    if let Some((key, version)) = contract {
        ensure!(
            builder.schema().metadata().get(key).map(String::as_str) == Some(version),
            "{}: expected {version}",
            path.display()
        );
    }
    let groups = (0..builder.metadata().num_row_groups())
        .filter(|&i| {
            selection.is_none_or(|squares| {
                row_group_intersects(builder.metadata().row_group(i), squares)
            })
        })
        .collect::<Vec<_>>();
    let roots = builder
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| columns.contains(&f.name().as_str()))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let mask = ProjectionMask::roots(builder.parquet_schema(), roots);
    for batch in builder
        .with_projection(mask)
        .with_row_groups(groups)
        .with_batch_size(8192)
        .build()?
    {
        visit(batch?)?;
    }
    Ok(())
}

fn row_group_intersects(
    group: &parquet::file::metadata::RowGroupMetaData,
    squares: &[Square],
) -> bool {
    use parquet::file::statistics::Statistics;
    let bound = |name: &str, minimum: bool| -> Option<f64> {
        let column = group
            .columns()
            .iter()
            .find(|c| c.column_path().string() == name)?;
        match column.statistics()? {
            Statistics::Float(v) => if minimum { v.min_opt() } else { v.max_opt() }
                .copied()
                .map(f64::from),
            Statistics::Double(v) => if minimum { v.min_opt() } else { v.max_opt() }.copied(),
            _ => None,
        }
    };
    let (Some(west), Some(south), Some(east), Some(north)) = (
        bound("bbox.xmin", true),
        bound("bbox.ymin", true),
        bound("bbox.xmax", false),
        bound("bbox.ymax", false),
    ) else {
        return true;
    };
    if [west, south, east, north].iter().any(|v| !v.is_finite()) {
        return true;
    }
    squares.iter().any(|&square| {
        let b = bounds(square);
        west <= b[2] && east >= b[0] && south <= b[3] && north >= b[1]
    })
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
                    footprints::convert(&source, path.parent().unwrap(), &squares, &mut rows)?
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
    for (square, rows) in squares.into_iter().zip(rows) {
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
