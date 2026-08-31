//! Popup-response wire shape.
//!
//! `noise-compute` carries internal `NoiseResult` / `SourceResult` /
//! `Contributor` structs shared with the tile-painter (which writes
//! Arrow tiles, not JSON). The popup JSON contract — what the frontend
//! reads — was previously assembled by the Fastify route in Node:
//! parse → rename fields (`periods.lden_db` → `received_lden`) → round
//! → re-stringify. That cost ~30-50 ms per popup AND was the root cause
//! of latent bugs (a field rename on the Rust side silently produced
//! `null` on the wire because Node's reshape looked at the old name).
//!
//! This module makes the Rust side the single source of truth: it
//! defines the exact shape the frontend's `noise.ts` interfaces match
//! and converts `NoiseResult` → `WireResult` at the end of the popup
//! compute. Node becomes a pass-through (`reply.type(application/json)
//! .send(finalJson)`) after a single `String.replace` for the
//! `compute_time_ms` placeholder.

use noise_compute::types::{
    Contributor, LayerKind, NoiseResult, PropagationBaseline, ScreeningBreakdown, SegmentTrace,
    SegmentTracesSummary, SourceMetadata, SourceResult, TerrainBreakdown, VegetationBreakdown,
    NUM_BANDS,
};
use serde::Serialize;

/// Unique sentinel for `compute_time_ms` — Node replaces this string
/// (including quotes) with the actual elapsed wall time. String literal
/// (not numeric placeholder) so it cannot collide with any dB value
/// elsewhere in the response.
pub const COMPUTE_TIME_MS_SENTINEL: &str = "__QM_COMPUTE_TIME_MS__";

#[inline]
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// `received_lden` / `received_lden_free` are non-nullable frontend fields;
/// map silence (`NEG_INFINITY`) to `0.0`.
#[inline]
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

#[derive(Serialize)]
pub struct WireSource {
    pub source_type: LayerKind,
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub lden: f64,
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub lden_free: f64,
    /// Per-source `L_day` (END 07:00–19:00, no penalty). Together with `le`
    /// this completes the period split on the wire — validation v2 ingests
    /// networks publishing ld/le/ln and must band each period directly.
    /// `null` when silent.
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub ld: f64,
    /// Per-source `L_evening` (END 19:00–23:00, no penalty). `null` when silent.
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub le: f64,
    /// Per-source `L_night` (END 23:00–07:00, no penalty) — exposed so the
    /// frontend + /check-world can band the night metric directly (C1: rail Ln
    /// is no longer the flat `Lden − 7.91`). `null` when the period is silent.
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub ln: f64,
    pub segment_count: usize,
    pub displayed_count: usize,
}

impl From<SourceResult> for WireSource {
    fn from(s: SourceResult) -> Self {
        WireSource {
            source_type: s.source_type,
            lden: s.periods.lden_db,
            lden_free: s.periods_free.lden_db,
            ld: s.periods.ld_db,
            le: s.periods.le_db,
            ln: s.periods.ln_db,
            segment_count: s.segment_count,
            displayed_count: s.displayed_count,
        }
    }
}

#[derive(Serialize)]
pub struct WireContributor {
    pub source_type: LayerKind,
    pub osm_id: Option<i64>,
    pub name: String,
    pub subtype: String,
    pub distance_m: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SourceMetadata>,
    pub emission_db: f64,
    /// Frontend `Contributor.emission_bands` is declared but never read
    /// (verified by grep 2026-05-24; previously Node defaulted to `[]`
    /// because Rust never emitted it).
    pub emission_bands: Vec<f64>,
    pub baseline: PropagationBaseline,
    pub terrain: TerrainBreakdown,
    pub screening: ScreeningBreakdown,
    pub vegetation: VegetationBreakdown,
    pub terrain_impact_db: f64,
    pub screening_impact_db: f64,
    pub vegetation_impact_db: f64,
    pub atmospheric_impact_db: f64,
    pub ground_impact_db: f64,
    pub received_lden: f64,
    pub received_lden_free: f64,
    /// Per-contributor `L_night` (END 23:00–07:00, no penalty). Mirrors
    /// `received_lden`'s `0.0`-for-silence mapping so the TS type stays
    /// non-nullable.
    pub received_ln: f64,
    pub received_bands: [f64; NUM_BANDS],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<serde_json::Value>,
}

impl From<Contributor> for WireContributor {
    fn from(c: Contributor) -> Self {
        WireContributor {
            source_type: c.source_type,
            osm_id: c.osm_id,
            name: c.name,
            subtype: c.subtype,
            distance_m: c.distance_m.round() as i64,
            metadata: c.metadata,
            emission_db: c.emission_db,
            emission_bands: Vec::new(),
            baseline: c.baseline,
            terrain: c.terrain,
            screening: c.screening,
            vegetation: c.vegetation,
            terrain_impact_db: c.terrain_impact_db,
            screening_impact_db: c.screening_impact_db,
            vegetation_impact_db: c.vegetation_impact_db,
            atmospheric_impact_db: c.atmospheric_impact_db,
            ground_impact_db: c.ground_impact_db,
            received_lden: round1(finite_or_zero(c.periods.lden_db)),
            received_lden_free: round1(finite_or_zero(c.periods_free.lden_db)),
            received_ln: round1(finite_or_zero(c.periods.ln_db)),
            received_bands: c.received_bands,
            geometry: c.geometry,
        }
    }
}

#[derive(Serialize)]
pub struct WireTimings {
    pub load_ms: f64,
    pub collect_ms: f64,
    pub road_ms: f64,
    pub rail_ms: f64,
    pub building_ms: f64,
    pub industrial_ms: f64,
    pub aircraft_airborne_ms: f64,
    pub aircraft_cruise_ms: f64,
    pub aircraft_ground_ms: f64,
}

impl From<noise_compute::types::LayerTimings> for WireTimings {
    fn from(t: noise_compute::types::LayerTimings) -> Self {
        WireTimings {
            load_ms: t.load_ms,
            collect_ms: t.collect_ms,
            road_ms: t.road_ms,
            rail_ms: t.rail_ms,
            building_ms: t.building_ms,
            industrial_ms: t.industrial_ms,
            aircraft_airborne_ms: t.aircraft_airborne_ms,
            aircraft_cruise_ms: t.aircraft_cruise_ms,
            aircraft_ground_ms: t.aircraft_ground_ms,
        }
    }
}

#[derive(Serialize)]
pub struct WireResult {
    pub h3_center: [f64; 2],
    pub elevation_m: f64,
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    /// Aggregate display total. When the receiver is inside an enclosed
    /// footprint, this carries the indoor estimate; source, contributor and
    /// segment values below remain the façade values that produced it.
    pub total_lden: f64,
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub total_lden_free: f64,
    pub sources: Vec<WireSource>,
    pub top_contributors: Vec<WireContributor>,
    #[serde(serialize_with = "noise_compute::types::serialize_lden_db_opt")]
    pub other_sources_lden: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_delta_db: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facade_lden: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indoor_lden: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indoor_lden_tilted: Option<f64>,
    /// Unique string sentinel — Node route replaces this with the
    /// wall-time `Date.now() - t0` measurement. See
    /// [`COMPUTE_TIME_MS_SENTINEL`].
    pub compute_time_ms: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_meta: Option<SegmentTracesSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<WireTimings>,
}

/// `indoor`: the winning envelope class, its effective delta, and the
/// outdoor façade total; `None` outdoors.
pub fn build_wire_result(
    result: NoiseResult,
    lat: f64,
    lng: f64,
    elevation: f64,
    indoor: Option<(noise_compute::envelope::EnvelopeClass, f64, f64)>,
) -> WireResult {
    WireResult {
        h3_center: [lat, lng],
        elevation_m: round1(elevation),
        total_lden: result.total.lden_db,
        total_lden_free: result.total_free.lden_db,
        sources: result.sources.into_iter().map(WireSource::from).collect(),
        top_contributors: result
            .contributors
            .into_iter()
            .map(WireContributor::from)
            .collect(),
        other_sources_lden: result.other_sources_lden,
        envelope_class: indoor.map(|(class, _, _)| class.name()),
        envelope_delta_db: indoor.map(|(_, delta, _)| delta),
        facade_lden: indoor.map(|(_, _, facade)| round1(facade)),
        indoor_lden: indoor.and_then(|(_, delta, facade)| {
            facade
                .is_finite()
                .then(|| round1((facade - delta).max(0.0)))
        }),
        indoor_lden_tilted: indoor.and_then(|(_, _, facade)| {
            facade.is_finite().then(|| round1((facade - 15.0).max(0.0)))
        }),
        compute_time_ms: COMPUTE_TIME_MS_SENTINEL,
        segments: result.segments,
        segments_meta: result.segments_meta,
        timings: result.timings.map(WireTimings::from),
    }
}

/// Apply the façade-to-indoor product estimate only to aggregate display totals.
///
/// Source rows and segment traces are deliberately left at their façade
/// values: they describe the actual source-to-receiver computation, while the
/// indoor estimate is a display-level transformation of the final total.
pub fn attenuate_total_for_indoor_display(result: &mut NoiseResult, delta: f64) {
    let attenuate = |value: &mut f64| {
        if value.is_finite() {
            *value = (*value - delta).max(0.0)
        }
    };
    for periods in [&mut result.total, &mut result.total_free] {
        attenuate(&mut periods.ld_db);
        attenuate(&mut periods.le_db);
        attenuate(&mut periods.ln_db);
        attenuate(&mut periods.lden_db);
    }
}
