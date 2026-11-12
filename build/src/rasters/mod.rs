//! Manifest-selected GDAL/PROJ rasters into the four native one-arc-second square bands.
mod blend;
mod cache;
mod canopy;
pub mod config;
mod countries;
mod cover;
mod datum;
mod dem;
mod distance;
mod forest;
pub(crate) mod ground;
mod runtime;
mod source;
mod warp;

use crate::dev4::{Square, z9_raster_window};
use crate::year::YearInputs;
use chrono::{Datelike, NaiveDate};
use config::Band;
use countries::Countries;
use rayon::prelude::*;
use source::Source;
use std::io::Write;
use std::path::Path;
use tiles::square::SQUARES_PER_AXIS;
use tiles::terrain::{NODES_PER_DEGREE, Window};

const BANDS: [&str; 4] = ["dem.u16le", "imd.u8", "forest.u8", "canopy.u8"];

fn square_receipt(root: &Path) -> Result<serde_json::Value, String> {
    BANDS
        .iter()
        .map(|name| crate::pipeline_files::stamp(&root.join(name)))
        .collect::<anyhow::Result<Vec<_>>>()
        .map(serde_json::Value::from)
        .map_err(|error| error.to_string())
}

fn square_complete(root: &Path) -> Result<bool, String> {
    let path = root.join("complete.json");
    if !path.exists() {
        return Ok(false);
    }
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if expected != square_receipt(root)? {
        return Err(format!(
            "completed raster square changed: {}",
            root.display()
        ));
    }
    Ok(true)
}

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

pub(super) fn open_sources(
    inputs: &YearInputs,
    countries: &Countries,
    selected: impl Fn(Band) -> bool,
) -> Result<Vec<Source>, String> {
    let plan = inputs
        .manifest
        .rasters
        .as_ref()
        .ok_or("year manifest has no raster plan")?;
    runtime::configure(inputs, plan)?;
    open_configured_sources(inputs, countries, selected)
}

/// The year's country polygons, which bound each national terrain model to its own country.
pub(super) fn read_countries(inputs: &YearInputs) -> Result<Countries, String> {
    Countries::read(inputs.required(crate::year::Kind::PrepareRegions)?)
}

fn open_configured_sources(
    inputs: &YearInputs,
    countries: &Countries,
    selected: impl Fn(Band) -> bool,
) -> Result<Vec<Source>, String> {
    let plan = inputs
        .manifest
        .rasters
        .as_ref()
        .ok_or("year manifest has no raster plan")?;
    let mut sources = Vec::new();
    for spec in plan.sources.iter().filter(|spec| selected(spec.band)) {
        let observation = inputs
            .manifest
            .inputs
            .iter()
            .find(|input| input.id == spec.input)
            .ok_or("raster input metadata is missing")?;
        let observed_year = observation.observed.bounds()?.1.year();
        let mut source = Source::open(spec.clone(), inputs.by_id(&spec.input)?, observed_year)?;
        // A national survey holds its own country; the world's model is the only one tagged "world".
        let region = observation.region.split(':').next().unwrap_or_default();
        if spec.band == Band::GroundHeight && region != "world" {
            source.country = Some(countries.label(region).ok_or_else(|| {
                format!("{}: region {region} has no country polygon", spec.input)
            })?);
        }
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
    Ok(sources)
}

fn land_coverage(sources: &[Source]) -> Result<Vec<f64>, String> {
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
    Ok(coverage)
}

pub fn build(inputs: &YearInputs, squares: &[Square], out: &Path) -> Result<(), String> {
    for square in squares {
        if square.x >= SQUARES_PER_AXIS || square.y >= SQUARES_PER_AXIS {
            return Err(format!("invalid z9 square {square:?}"));
        }
    }
    let countries = read_countries(inputs)?;
    let sources = open_sources(inputs, &countries, |_| true)?;
    cache::prepare(inputs, &sources, out)?;
    let coverage = land_coverage(&sources)?;
    drop(sources);
    let forest_year = loss_year_before(&inputs.manifest.as_of)?;
    // Configure GDAL once above; each worker owns separate datasets and PROJ transformations.
    build_every_square(
        squares,
        || open_configured_sources(inputs, &countries, |_| true),
        |sources, square| {
            let sources = sources.as_ref().map_err(Clone::clone)?;
            let window = z9_raster_window(square);
            let land = touches_land(&coverage, window);
            let root = out
                .join("z9")
                .join(square.x.to_string())
                .join(square.y.to_string());
            if square_complete(&root)? {
                return Ok(());
            }
            eprintln!("raster {}/{}: building four bands", square.x, square.y);
            let bands = if land {
                let dem = dem::build(sources, &countries, window)?;
                let (imd, forest) = cover::build(sources, window, forest_year)?;
                let canopy = canopy::build(sources, window)?;
                [dem, imd, forest, canopy]
            } else {
                [Vec::new(), Vec::new(), Vec::new(), Vec::new()]
            };
            for (name, bytes) in BANDS.into_iter().zip(bands) {
                write(&root.join(name), &bytes)?;
                eprintln!(
                    "raster {}/{} {name}: {} bytes",
                    square.x,
                    square.y,
                    bytes.len()
                );
            }
            crate::pipeline_files::write_json(&root.join("complete.json"), &square_receipt(&root)?)
                .map_err(|error| error.to_string())
        },
    )
}

/// Builds every square, then fails with each failing square and its error, so that one defect
/// neither stops the others nor hides the next behind a restart, and no failure passes unseen.
fn build_every_square<Worker>(
    squares: &[Square],
    worker: impl Fn() -> Worker + Sync + Send,
    build_square: impl Fn(&mut Worker, Square) -> Result<(), String> + Sync + Send,
) -> Result<(), String> {
    let failures: Vec<String> = squares
        .par_iter()
        .map_init(worker, |worker, &square| {
            build_square(worker, square).err().map(|error| {
                eprintln!("raster {}/{}: failed: {error}", square.x, square.y);
                format!("{}:{}: {error}", square.x, square.y)
            })
        })
        .flatten()
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} of {} raster squares failed: {}",
        failures.len(),
        squares.len(),
        failures.join("; ")
    ))
}

#[cfg(test)]
mod tests;
