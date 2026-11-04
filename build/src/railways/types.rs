//! Explicit passenger and freight evidence, with zero distinct from unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CategoryFlow {
    pub periods: [f64; 3],
    pub missing_calendar_estimate: [f64; 3],
    pub status: u8,
    pub source_id: u16,
    pub matching: u8,
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
}
