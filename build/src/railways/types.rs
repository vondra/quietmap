//! Explicit passenger and freight evidence, with zero distinct from unknown.

/// Per-track crossing count carried by a crossing-inventory interval.
/// Values are already divided by `matched_tracks` (the physical main tracks
/// that shared the crossing on the global rail graph).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossingTotal {
    pub periods_per_track: [f64; 3],
    /// Inventory's own per-track passenger trains/day (`None` when unknown).
    pub passenger_per_track: Option<f64>,
    /// Physical main tracks that shared the crossing (`>= 1`).
    pub matched_tracks: u8,
}

/// Marks a crossing-derived freight `CategoryFlow` whose `periods` hold total
/// per-track through trains until the track's resolved passenger flow is
/// subtracted in parallel-track finalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossingFreight {
    /// Crossing inventory's own per-track passenger trains/day (`None` when unknown).
    pub inventory_passenger: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CategoryFlow {
    pub periods: [f64; 3],
    pub missing_calendar_estimate: [f64; 3],
    pub status: u8,
    pub source_id: u16,
    pub matching: u8,
    /// The stop whose departures stamp this flow (`local_match`), so a line counts each stop
    /// once however many of its tracks the stop stamps; 0 for a routed or modelled flow.
    pub stop: u32,
    /// Physical tracks sharing a crossing observation (`0` for non-crossing flows).
    pub matched_tracks: u8,
    /// Set on crossing-derived freight flows until passenger trains are subtracted.
    pub crossing_freight: Option<CrossingFreight>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowTraffic {
    pub passenger: CategoryFlow,
    pub freight: CategoryFlow,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Crossing {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    pub soundings: [f64; 3],
    pub speed_kmh: Option<f64>,
    pub source_id: u16,
    pub synth_osm: i64,
    /// Through trains across END [day, evening, night] over all main tracks;
    /// `None` when the inventory carries no usable through-train count.
    #[serde(default)]
    pub total_periods: Option<[f64; 3]>,
    /// Daily passenger trains over all main tracks; `None` when unknown.
    #[serde(default)]
    pub passenger_daily: Option<f64>,
    /// Main tracks reported at the crossing (`> 0` when present).
    #[serde(default)]
    pub main_tracks: Option<u8>,
    /// True when the inventory explicitly declares freight-only train service.
    #[serde(default)]
    pub freight_only: bool,
}
