//! OSM tag classification for noise-relevant features.
//!
//! Shared core lives here: the [`FeatureType`] taxonomy, the [`Tags`] alias,
//! and the `QM_OSM_ONLY` [`scope_keeps`] gate. The per-kind logic is split into
//! submodules and re-exported flat so callers keep using `classify::<fn>`:
//! - [`ways`] — way classification + way tag extraction.
//! - [`nodes`] — standalone-node classification + node tag extraction.
//! - [`mappers`] — stateless tag→enum/value mappers (road/rail/aeroway/leisure,
//!   width + maxspeed parsers).

use std::collections::HashMap;

mod mappers;
mod nodes;
mod ways;

pub use mappers::*;
pub use nodes::*;
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
    /// Open-air leisure AREA (sports pitch / playground / pool / beer garden) —
    /// settlement v2 phase 2. No `building=*`, so it was dropped before phase 2;
    /// now spilled to its own `leisure.arrow` with a `sport` u8 + capacity.
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

/// Optional extract scope: `QM_OSM_ONLY="buildings,leisure,poi"` keeps only
/// the listed `FeatureType::name()` families (plus `poi`); everything else
/// classifies to None, so it is never assembled or spilled. Unset = full
/// extract. WHY: a layer-scoped planet run — the 2026-06-12 phase-2 world
/// re-extract needed buildings+leisure only, and the full-scope Pass-2 spill
/// (roads dominate) did not fit the host disk. The kept families' output is
/// byte-identical to a full run (the filter only drops, never alters).
pub(crate) fn scope_keeps(ft: &FeatureType) -> bool {
    use std::sync::OnceLock;
    static ONLY: OnceLock<Option<Vec<String>>> = OnceLock::new();
    let only = ONLY.get_or_init(|| {
        std::env::var("QM_OSM_ONLY").ok().map(|v| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
    });
    match only {
        None => true,
        Some(list) => {
            let fam = match ft {
                FeatureType::Poi => "poi",
                FeatureType::WindTurbine => "wind_turbine",
                other => other.name(),
            };
            list.iter().any(|k| k == fam)
        }
    }
}
