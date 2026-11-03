//! OSM tag classification for noise-relevant features.

use std::collections::HashMap;

mod evidence;
mod mappers;
mod nodes;
mod tags;
mod values;
mod ways;

pub use evidence::*;
pub use mappers::*;
pub use nodes::*;
pub use tags::*;
pub use values::*;
pub use ways::*;

#[derive(Debug, Clone, PartialEq)]
pub enum FeatureType {
    Road,
    Railway,
    AirportArea,
    AirportLine,
    Building,
    Industrial,
    WindTurbine,
    Barrier,
    /// Open-air AREA source (sports pitch / playground / pool / beer garden /
    /// car park), stored in `leisure.arrow` with a class u8.
    /// Nothing in that file ever screens.
    Leisure,
    /// A standalone function node (`amenity=`/`shop=`/`tourism=`/`healthcare=`)
    /// used ONLY by the finalize POI-in-footprint join to reclassify the
    /// `building=yes` it sits inside — never written to a final arrow itself.
    Poi,
}

impl FeatureType {
    pub fn is_linear(&self) -> bool {
        matches!(
            self,
            Self::Road | Self::Railway | Self::Barrier | Self::AirportLine
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Road => "roads",
            Self::Railway => "railways",
            Self::AirportArea => "airport_areas",
            Self::AirportLine => "airport_lines",
            Self::Building => "buildings",
            Self::Industrial => "industrial",
            Self::WindTurbine => "industrial", // stored in same file
            Self::Barrier => "barriers",
            Self::Leisure => "leisure",
            Self::Poi => "poi",
        }
    }
}

/// Tags extracted from OSM features, stored as simple key-value pairs.
pub type Tags = HashMap<String, String>;
