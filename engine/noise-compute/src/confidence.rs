//! Data confidence and provenance reporting (from V44 design).
//!
//! Reports what data is available, what's estimated, what's missing.

use serde::Serialize;

/// Confidence assessment for a noise computation.
#[derive(Debug, Clone, Serialize)]
pub struct Confidence {
    pub overall: f64,       // 0.0 (no data) to 1.0 (full census + validation)
    pub notes: Vec<String>, // human-readable notes
}

impl Default for Confidence {
    fn default() -> Self {
        Self::new()
    }
}

impl Confidence {
    pub fn new() -> Self {
        Confidence {
            overall: 0.5,
            notes: Vec::new(),
        }
    }

    pub fn add_note(&mut self, note: &str) {
        self.notes.push(note.to_string());
    }

    /// Build confidence from available data.
    pub fn assess(
        has_traffic_census: bool,
        has_railway_census: bool,
        has_aircraft_data: bool,
        has_terrain: bool,
    ) -> Self {
        let mut c = Confidence::new();
        // 0.4 = 0.3 OSM geometry + 0.1 building heights. The building term used
        // to be conditional on a raster tile existing here; buildings are vector
        // polygons now and a region without them fails to load, so there is no
        // "we have no building data" state left to score. An EMPTY set (desert,
        // ocean) is an answer, not a gap, and must not cost the visitor score.
        let mut score = 0.4;

        if has_traffic_census {
            score += 0.2;
        } else {
            c.add_note("Road traffic: CNOSSOS defaults (no census data)");
        }

        if has_railway_census {
            score += 0.1;
        } else {
            c.add_note("Railway traffic: tier estimates (no schedule data)");
        }

        if has_aircraft_data {
            score += 0.15;
        } else {
            c.add_note("Aircraft: no ADS-B data for this area");
        }

        if has_terrain {
            score += 0.1;
        } else {
            c.add_note("Terrain: flat (no DEM available)");
        }

        // Always note settlement/industrial are estimates
        c.add_note("Settlement noise: estimated (custom model, not standardized)");
        c.add_note("Industrial noise: estimated spectra (not measured)");

        c.overall = if score > 1.0 { 1.0 } else { score };
        c
    }
}
