//! Normalize manifest-selected vessel density and presence into monthly occupancy and coverage.
#[path = "ship_density.rs"]
mod density;
#[path = "ship_grid.rs"]
pub mod grid;
#[path = "ship_reports.rs"]
mod reports;
#[path = "ship_presence.rs"]
mod ship_presence;
#[path = "ship_raster.rs"]
pub mod ship_raster;

use crate::dev4::degrees_to_z30;
use crate::year::{Input, YearInputs};
use grid::{Coverage, NativeGrid};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use tiles::square::Square;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Large,
    Work,
    Leisure,
}
impl Class {
    pub fn index(self) -> usize {
        match self {
            Self::Large => 0,
            Self::Work => 1,
            Self::Leisure => 2,
        }
    }
}

#[derive(Deserialize)]
pub struct Specification {
    pub priority: u16,
    pub source_id: u16,
    #[serde(flatten)]
    format: Format,
}
impl Specification {
    pub fn has_coverage(&self) -> bool {
        matches!(self.format, Format::DensityRaster { .. })
    }
}

#[derive(Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case", deny_unknown_fields)]
enum Format {
    DensityRaster {
        epsg: u16,
        parts: Vec<DensityPart>,
    },
    PresenceReports {
        dataset: String,
        window: String,
        inventory: reports::InventoryObject,
        tiff_member: String,
        csv_member: String,
        cell_degrees: f64,
        acquisition: serde_json::Value,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DensityPart {
    path: String,
    member: String,
    class: Class,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub centroid: (i32, i32),
    pub area_m2: f32,
    pub hours: [f32; 3],
    pub source_id: u16,
}

pub struct Observation {
    pub longitude: f64,
    pub latitude: f64,
    pub cell: Cell,
}

pub struct Normalized {
    pub observations: Vec<(Square, Observation)>,
    pub coverage: Option<Coverage>,
    pub grid: NativeGrid,
}

// Retained prepare threshold: below it a large vessel cell is under 10 dBA at its edge.
const MIN_MONTHLY_HOURS: f64 = 0.5;

fn observation(
    longitude: f64,
    latitude: f64,
    area_m2: f64,
    hours: [f64; 3],
    source_id: u16,
    squares: &HashSet<Square>,
) -> Option<(Square, Observation)> {
    let centroid = degrees_to_z30(latitude, longitude);
    let square = Square::of_z30(centroid.0, centroid.1);
    squares.contains(&square).then(|| {
        (
            square,
            Observation {
                longitude,
                latitude,
                cell: Cell {
                    centroid,
                    area_m2: area_m2 as f32,
                    hours: hours.map(|v| v as f32),
                    source_id,
                },
            },
        )
    })
}

pub fn specification(input: &Input) -> Result<Specification, String> {
    serde_json::from_value(
        input
            .conversion
            .clone()
            .ok_or_else(|| format!("{}: missing ship decoding parameters", input.id))?,
    )
    .map_err(|e| format!("{}: {e}", input.id))
}

pub fn read(
    spec: &Specification,
    root: &Path,
    inputs: &YearInputs,
    input: &Input,
    squares: &HashSet<Square>,
) -> Result<Normalized, String> {
    match &spec.format {
        Format::DensityRaster { epsg, parts } => {
            density::read(*epsg, parts, spec.source_id, root, squares)
        }
        Format::PresenceReports { .. } => reports::read(spec, root, inputs, input, squares),
    }
}

#[cfg(test)]
#[path = "ships_tests.rs"]
pub(crate) mod tests;
