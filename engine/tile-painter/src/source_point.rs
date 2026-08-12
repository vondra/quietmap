//! Shared point-source row consumed by [`crate::scatter_point`]. Industrial
//! sites and (M4) buildings discretise into one or more grid points; each
//! point is one [`PointRow`] with its share of the source power
//! (`Lw − 10·log10(N)`, split at load by `prepare_*_points`). Point sources
//! use ISO 9613-2 SPHERICAL divergence (`20·log10(d)+11`, no finite-line
//! correction) and an exclusion radius (the source's own footprint is not a
//! screening obstacle), unlike the line row in [`crate::source_line`].

use noise_compute::normalize::PreparedPoint;
use noise_compute::types::NUM_BANDS;

use crate::source_line::db_bands_to_lin;

/// One discretised point source: location + per-period linear band emission.
/// `emission_lin[period][band] = 10^(L_W_dB / 10)`, precomputed at load.
pub struct PointRow {
    pub lat: f64,
    pub lon: f64,
    /// Source height above ground (m): industrial 5/8/10, wind hub height,
    /// building per `prepare_building_points`.
    pub source_height_m: f64,
    /// Propagation cutoff (m): industrial = per-grid-point loudest-day-band
    /// audibility, capped at `INDUSTRIAL_MAX_RADIUS`; building = parent-Lw
    /// audibility.
    pub max_distance_m: f64,
    /// Self-screening footprint radius `√(area_per_point/π)` (m): buildings
    /// within it are the source's own footprint, not a barrier (ISO 9613-2).
    /// Also shrinks the effective propagation distance for the area source.
    pub exclusion_radius_m: f64,
    /// Max day-period band level (dB) — drives the free-field audibility
    /// pre-gate (`geo::below_free_field_threshold`) the oracle applies per
    /// receiver, so the scatter culls the same sources the popup does.
    pub max_day_emission_db: f64,
    /// `[day, evening, night][band]` linear A-unweighted band energy.
    pub emission_lin: [[f32; NUM_BANDS]; 3],
}

impl PointRow {
    /// Build a scatter row from a normalised grid point — the shared
    /// industrial/building conversion (linearise emission, hoist the max day
    /// band level for the free-field pre-gate). `dist_m` is per-receiver and
    /// not carried here (the scatter computes it per pixel).
    pub fn from_prepared(p: &PreparedPoint) -> Self {
        Self {
            lat: p.lat,
            lon: p.lon,
            source_height_m: p.source_height_m as f64,
            max_distance_m: p.max_radius_m,
            exclusion_radius_m: p.exclusion_radius_m as f64,
            max_day_emission_db: p.lw_day.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64,
            emission_lin: [
                db_bands_to_lin(p.lw_day),
                db_bands_to_lin(p.lw_evening),
                db_bands_to_lin(p.lw_night),
            ],
        }
    }
}
