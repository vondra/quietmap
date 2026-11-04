//! Read digest-pinned presence inventories and scale their exact observation window to a nominal month.
use super::{
    Format, MIN_MONTHLY_HOURS, Normalized, Specification,
    grid::{Grid, NativeGrid},
    observation, ship_presence,
    ship_raster::relative_path,
};
use crate::year::{Input, YearInputs};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use tiles::square::Square;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryObject {
    pub path: String,
    pub sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Object {
    path: String,
    bytes: u64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    bounds: [i32; 4],
    large: Object,
    work: Object,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    window_sha256: String,
    reports: Vec<Report>,
}

pub(super) fn read(
    spec: &Specification,
    root: &Path,
    inputs: &YearInputs,
    input: &Input,
    squares: &HashSet<Square>,
) -> Result<Normalized, String> {
    let Format::PresenceReports {
        dataset,
        window,
        inventory,
        tiff_member,
        csv_member,
        cell_degrees,
        acquisition,
    } = &spec.format
    else {
        unreachable!()
    };
    let bytes = std::fs::read(relative_path(root, &inventory.path)?).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != inventory.sha256 {
        return Err("ship inventory checksum differs from the manifest".into());
    }
    let mut inventory: Inventory = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(relative_path(root, window)?).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != inventory.window_sha256 {
        return Err("ship window checksum differs from the pinned inventory".into());
    }
    let window: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let (start, last) = input.observed.bounds()?;
    let end = last.succ_opt().ok_or("invalid ship observation end")?;
    let days = (end - start).num_days();
    if start.to_string() != inputs.manifest.flows.start
        || end.to_string() != inputs.manifest.flows.end
        || window["first_day"].as_str() != Some(&start.to_string())
        || window["last_day"].as_str() != Some(&last.to_string())
        || window["end_exclusive"].as_str() != Some(&end.to_string())
        || window["days"].as_i64() != Some(days)
        || window["dataset"].as_str() != Some(dataset)
        || window["classes"] != acquisition["classes"]
        || window["tile_deg"] != acquisition["tile_degrees"]
    {
        return Err("GFW observation window or dataset differs from the manifest".into());
    }
    let bounds: [i32; 4] =
        serde_json::from_value(acquisition["bounds"].clone()).map_err(|e| e.to_string())?;
    let step = acquisition["tile_degrees"]
        .as_i64()
        .filter(|v| *v > 0 && *v <= 360)
        .ok_or("invalid ship report tile size")? as i32;
    let [west, south, east, north] = bounds;
    if west < -180
        || east > 180
        || south < -90
        || north > 90
        || west >= east
        || south >= north
        || (east - west) % step != 0
        || (north - south) % step != 0
    {
        return Err("invalid ship report grid".into());
    }
    let expected: BTreeSet<[i32; 4]> = (west..east)
        .step_by(step as usize)
        .flat_map(|lon| {
            (south..north)
                .step_by(step as usize)
                .map(move |lat| [lon, lat, lon + step, lat + step])
        })
        .collect();
    let actual: BTreeSet<_> = inventory.reports.iter().map(|r| r.bounds).collect();
    if actual != expected || actual.len() != inventory.reports.len() {
        return Err("ship inventory does not cover every report tile exactly once".into());
    }
    inventory
        .reports
        .sort_by(|a, b| a.large.path.cmp(&b.large.path));
    let native = NativeGrid {
        epsg: 4326,
        width: *cell_degrees,
        height: *cell_degrees,
    };
    let grid = Grid::new(native.clone())?;
    let per_month = physics::emission::ships::HOURS_PER_MONTH / 24.0 / days as f64;
    let mut observations = Vec::new();
    let mut paths = HashSet::new();
    for report in inventory.reports {
        let selected = squares
            .iter()
            .any(|square| intersects(*square, report.bounds.map(f64::from)));
        let mut cells: BTreeMap<(i32, i32), [f64; 3]> = BTreeMap::new();
        for (class, object) in [report.large, report.work].iter().enumerate() {
            if !paths.insert(object.path.clone()) {
                return Err("ship inventory repeats a report object".into());
            }
            let path = relative_path(root, &object.path)?;
            let metadata =
                std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if metadata.len() != object.bytes {
                return Err(format!(
                    "{}: report size differs from the inventory",
                    path.display()
                ));
            }
            if !selected {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            if format!("{:x}", Sha256::digest(bytes)) != object.sha256 {
                return Err(format!(
                    "{}: report checksum differs from the inventory",
                    path.display()
                ));
            }
            for (key, hours) in ship_presence::cells(&path, tiff_member, csv_member, *cell_degrees)?
            {
                cells.entry(key).or_default()[class] += hours;
            }
        }
        for ((longitude, latitude), hours) in cells {
            let hours = hours.map(|v| v * per_month);
            if hours.iter().sum::<f64>() < MIN_MONTHLY_HOURS {
                continue;
            }
            let (longitude, latitude) =
                (f64::from(longitude) / 1000.0, f64::from(latitude) / 1000.0);
            if let Some(cell) = observation(
                longitude,
                latitude,
                grid.area_m2(latitude),
                hours,
                spec.source_id,
                squares,
            ) {
                observations.push(cell);
            }
        }
    }
    Ok(Normalized {
        observations,
        coverage: None,
        grid: native,
    })
}

fn intersects(square: Square, [west, south, east, north]: [f64; 4]) -> bool {
    let longitude = |x: u32| f64::from(x) / 512.0 * 360.0 - 180.0;
    let latitude = |y: u32| {
        (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 512.0))
            .sinh()
            .atan()
            .to_degrees()
    };
    longitude(square.x) <= east
        && longitude(square.x + 1) >= west
        && latitude(square.y) >= south
        && latitude(square.y + 1) <= north
}
