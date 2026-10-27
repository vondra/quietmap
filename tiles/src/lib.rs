//! Prepared z12 tiles: numbering and coordinates ([`geo`]), the z9 squares and their files, one a
//! square and kind ([`square`]: read and write), and one module per kind with its tile format,
//! writer and reader ([`terrain`], [`terrain_fine`], [`obstacles`], [`sources`], [`aircraft`],
//! [`aircraft_rolls`]); [`aircraft_tracks`] holds the year's flights as the map draws them.
//!
//! For visitors, down to this code: the About page Data preparation, The files
//! (`frontend/src/about/data-preparation.md`).

pub mod aircraft;
pub mod aircraft_events;
pub mod aircraft_rolls;
pub mod aircraft_tracks;
pub mod geo;
pub mod obstacles;
mod rate;
pub mod sources;
pub mod square;
pub mod terrain;
pub mod terrain_fine;

use std::path::Path;

/// The kinds of tiles, one file a square and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Terrain,
    /// The 1/6" heights where a national model has them, read near a ray's ends.
    TerrainFine,
    Obstacles,
    Sources,
    Aircraft,
    /// The same format with boxes of a coarser rule, read for tiles beyond 3 km of the click.
    AircraftFar,
    /// What flies over each cell: flights a day above 50, 60 and 70 dB.
    AircraftEvents,
    /// The runways' take-off and landing rolls that start in each tile.
    AircraftRolls,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Terrain => "terrain",
            Kind::TerrainFine => "terrain-fine",
            Kind::Obstacles => "obstacles",
            Kind::Sources => "sources",
            Kind::Aircraft => "aircraft",
            Kind::AircraftFar => "aircraft-far",
            Kind::AircraftEvents => "aircraft-events",
            Kind::AircraftRolls => "aircraft-rolls",
        }
    }
}

/// Written last by a builder: only a year root holding it is served, and only there does an
/// absent square file or tile mean "empty". Its first line names the layout ([`LAYOUT`]): a year
/// root of another layout would read as empty, so a reader refuses it.
pub const COMPLETION_MARKER: &str = "complete";
/// The completion marker's first line: the year root laid out as [`square`] reads and writes it.
pub const LAYOUT: &str = "qmsq1";

/// Marks a year root complete, its layout's line before `note`.
pub fn mark_year_complete(year_root: &Path, note: &str) -> Result<(), String> {
    let path = year_root.join(COMPLETION_MARKER);
    std::fs::write(&path, format!("{LAYOUT}\n{note}"))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Refuses a year root that is not complete or not laid out as this code reads it.
pub fn check_year_complete(year_root: &Path) -> Result<(), String> {
    let marker = std::fs::read_to_string(year_root.join(COMPLETION_MARKER))
        .map_err(|_| format!("{} is not a complete release", year_root.display()))?;
    if marker.lines().next() != Some(LAYOUT) {
        return Err(format!(
            "{} is not laid out as {LAYOUT}",
            year_root.display()
        ));
    }
    Ok(())
}

/// A tile whose bytes do not follow its kind's format: the click fails, never answers quieter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatError(pub &'static str);

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for FormatError {}
