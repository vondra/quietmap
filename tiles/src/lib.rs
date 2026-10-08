//! Prepared z12 tiles: numbering and coordinates ([`geo`]) and one module per kind with its
//! file format, writer and reader ([`terrain`], [`obstacles`], [`sources`], [`aircraft`]); [`read`]
//! reads whole files; [`aircraft_tracks`] holds the year's flights as the map draws them.

pub mod aircraft;
pub mod aircraft_events;
pub mod aircraft_tracks;
pub mod geo;
pub mod obstacles;
pub mod read;
pub mod sources;
pub mod terrain;

use std::path::{Path, PathBuf};

/// The kinds of tile files, one file per kind and tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Terrain,
    Obstacles,
    Sources,
    Aircraft,
    /// The same format with boxes of a coarser rule, read from the second ring on.
    AircraftFar,
    /// What flies over each cell: flights a day above 50, 60 and 70 dB.
    AircraftEvents,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Terrain => "terrain",
            Kind::Obstacles => "obstacles",
            Kind::Sources => "sources",
            Kind::Aircraft => "aircraft",
            Kind::AircraftFar => "aircraft-far",
            Kind::AircraftEvents => "aircraft-events",
        }
    }
}

/// `<year root>/<x9>/<y9>/<x12>_<y12>.<kind>`.
pub fn tile_path(year_root: &Path, tile: geo::TileId, kind: Kind) -> PathBuf {
    let (x9, y9) = tile.z9();
    year_root
        .join(x9.to_string())
        .join(y9.to_string())
        .join(format!("{}_{}.{}", tile.x, tile.y, kind.name()))
}

/// Written last by a builder: only a year root holding it is served, and only there does an
/// absent tile file mean "empty".
pub const COMPLETION_MARKER: &str = "complete";

/// A tile file whose bytes do not follow its kind's format: the click fails, never answers quieter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatError(pub &'static str);

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for FormatError {}
