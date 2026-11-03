//! Manifest-selected GDAL/PROJ rasters into the four native one-arc-second square bands.
mod blend;
mod canopy;
pub mod config;
mod cover;
mod datum;
mod dem;
mod distance;
mod forest;
mod runtime;
mod source;
mod warp;

use crate::dev4::{Square, z9_raster_window};
use crate::year::YearInputs;
use chrono::{Datelike, NaiveDate};
use config::Band;
use source::Source;
use std::io::Write;
use std::path::Path;
use tiles::square::SQUARES_PER_AXIS;
use tiles::terrain::{NODES_PER_DEGREE, Window};

fn touches_land(coverage: &[f64], window: Window) -> bool {
    let south = (window.north_node - window.rows as i32)
        .div_euclid(NODES_PER_DEGREE)
        .max(-90);
    let north = window.north_node.div_euclid(NODES_PER_DEGREE).min(89);
    let west = (window.west_node - 1).div_euclid(NODES_PER_DEGREE);
    let east = (window.west_node + window.columns as i32 - 1).div_euclid(NODES_PER_DEGREE);
    (south..=north).any(|latitude| {
        (west..=east).any(|longitude| {
            coverage[((89 - latitude) * 360 + (longitude + 180).rem_euclid(360)) as usize] == 1.0
        })
    })
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        if std::fs::read(path).map_err(|e| e.to_string())? == bytes {
            return Ok(());
        }
        return Err(format!(
            "refusing to replace different raster {}",
            path.display()
        ));
    }
    let parent = path.parent().ok_or("raster output needs a parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    staged.write_all(bytes).map_err(|e| e.to_string())?;
    staged.as_file().sync_all().map_err(|e| e.to_string())?;
    staged.persist_noclobber(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn loss_year_before(cutoff: &str) -> Result<i32, String> {
    // An annual loss code has no month or day: only completed years precede the cutoff.
    let cutoff = NaiveDate::parse_from_str(cutoff, "%Y-%m-%d").map_err(|e| e.to_string())?;
    Ok(cutoff.year() - 1)
}

pub fn build(inputs: &YearInputs, squares: &[Square], out: &Path) -> Result<(), String> {
    let plan = inputs
        .manifest
        .rasters
        .as_ref()
        .ok_or("year manifest has no raster plan")?;
    for square in squares {
        if square.x >= SQUARES_PER_AXIS || square.y >= SQUARES_PER_AXIS {
            return Err(format!("invalid z9 square {square:?}"));
        }
    }
    runtime::configure(inputs, plan)?;
    let mut sources = Vec::new();
    for spec in &plan.sources {
        let observation = inputs
            .manifest
            .inputs
            .iter()
            .find(|input| input.id == spec.input)
            .ok_or("raster input metadata is missing")?;
        let observed_year = observation.observed.bounds()?.1.year();
        let mut source = Source::open(spec.clone(), inputs.by_id(&spec.input)?, observed_year)?;
        if let Some(wrapped) = &spec.antimeridian_input {
            source.antimeridian = Some(Box::new(Source::open(
                spec.clone(),
                inputs.by_id(wrapped)?,
                observed_year,
            )?));
        }
        sources.push(source);
    }
    sources.sort_by_key(|source| (source.spec.band, source.spec.priority));
    let coverage = sources
        .iter()
        .find(|s| s.spec.band == Band::Coverage)
        .ok_or("missing raster land coverage")?;
    if coverage.dataset.raster_size() != (360, 180)
        || coverage.spec.horizontal_crs != "EPSG:4326"
        || coverage
            .dataset
            .geo_transform()
            .map_err(|e| e.to_string())?
            != [-180.0, 1.0, 0.0, 90.0, 0.0, -1.0]
    {
        return Err("land coverage must have one cell per geographic degree".into());
    }
    let coverage = warp::values(&coverage.dataset)?;
    if coverage.iter().any(|v| *v != 0.0 && *v != 1.0) {
        return Err("land coverage must explicitly classify every degree".into());
    }
    let forest_year = loss_year_before(&inputs.manifest.as_of)?;
    for &square in squares {
        let window = z9_raster_window(square);
        eprintln!("raster {}/{}: building four bands", square.x, square.y);
        let bands = if touches_land(&coverage, window) {
            let dem = dem::build(&sources, window)?;
            let (imd, forest) = cover::build(&sources, window, forest_year)?;
            let canopy = canopy::build(&sources, window)?;
            [dem, imd, forest, canopy]
        } else {
            [Vec::new(), Vec::new(), Vec::new(), Vec::new()]
        };
        let root = out
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string());
        for (name, bytes) in ["dem.u16le", "imd.u8", "forest.u8", "canopy.u8"]
            .into_iter()
            .zip(bands)
        {
            write(&root.join(name), &bytes)?;
            eprintln!(
                "raster {}/{} {name}: {} bytes",
                square.x,
                square.y,
                bytes.len()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
