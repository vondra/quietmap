//! Top-level `NoiseResult` type — total + per-source breakdown + top
//! contributors + optional popup traces/timings. The shape the engine returns.
use super::*;

/// Noise computation result — periods + per-source breakdown + top contributors.
#[derive(Debug, Clone, Serialize)]
pub struct NoiseResult {
    pub total: NoisePeriods,
    /// Energy sum across all sources without terrain / screening / vegetation
    /// path effects. Airborne aircraft use their retained pre-screen Doc 29
    /// energy; source types whose kernels do not expose a second sum retain
    /// their existing received-equivalent value. Was previously absent; the
    /// popup wire shape exposed `total_lden_free` as the diff and it always
    /// read `null`. Now populated from each `SourceResult`.
    pub total_free: NoisePeriods,
    pub sources: Vec<SourceResult>,
    pub contributors: Vec<Contributor>,
    /// Energy sum (dB) of contributors not shown in `contributors` — those
    /// below the display threshold or truncated past the top-N cap.
    /// `NEG_INFINITY` when everything fits in `contributors`.
    #[serde(serialize_with = "serialize_lden_db_opt")]
    pub other_sources_lden: f64,
    pub confidence: crate::confidence::Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aircraft_detail: Option<AircraftBandData>,
    /// Populated only when the caller requests per-segment traces (popup path).
    /// Skipped from JSON when empty to keep pipeline responses lean.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentTrace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments_meta: Option<SegmentTracesSummary>,
    /// Per-layer compute timings (ms). Populated by `compute_at_point_inner`
    /// (road / rail / building / industrial) and by source-reader for the
    /// aircraft layers + outer load/collect/json. Always emitted so the
    /// popup can show a per-component breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<LayerTimings>,
}

/// Per-layer wall-clock breakdown of one popup request.
///
/// All fields are pre-serialization measurements — `total_ms` (XHR
/// wall clock) and JSON serialization cost are not represented here
/// (the JSON would have to include its own size, chicken-and-egg).
/// The frontend can derive them from network timing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LayerTimings {
    /// `ensure_hex` mmap load (cold).
    pub load_ms: f64,
    /// `collect_from_hex_data` Arrow → typed view conversion.
    pub collect_ms: f64,
    /// Per-layer compute (wrapping `compute_roads` etc. inside
    /// `compute_at_point_inner`). 0 when the layer is empty.
    pub road_ms: f64,
    pub rail_ms: f64,
    pub building_ms: f64,
    pub industrial_ms: f64,
    /// Aircraft sub-layers, populated by source-reader's `aircraft_v6` path.
    /// `aircraft_airborne_ms` includes `airb_detail` post-processing.
    pub aircraft_airborne_ms: f64,
    pub aircraft_cruise_ms: f64,
    pub aircraft_ground_ms: f64,
}

/// Serialize a possibly-infinite Lden dB scalar as JSON `number | null`.
/// `NEG_INFINITY` (silence) → `null`. Public so `source-reader/wire.rs`
/// can attach it to wire-shape fields.
pub fn serialize_lden_db_opt<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() {
        s.serialize_f64(*v)
    } else {
        s.serialize_none()
    }
}

impl NoiseResult {
    pub fn empty() -> Self {
        NoiseResult {
            total: NoisePeriods::silence(),
            total_free: NoisePeriods::silence(),
            sources: vec![],
            contributors: vec![],
            other_sources_lden: f64::NEG_INFINITY,
            confidence: crate::confidence::Confidence::new(),
            aircraft_detail: None,
            segments: Vec::new(),
            segments_meta: None,
            timings: None,
        }
    }
}

/// Period-resolved noise levels.
#[derive(Debug, Clone, Serialize)]
pub struct NoisePeriods {
    pub ld_db: f64,   // day   (07-19)
    pub le_db: f64,   // evening (19-23)
    pub ln_db: f64,   // night (23-07)
    pub lden_db: f64, // Lden (weighted)
}

impl NoisePeriods {
    pub fn silence() -> Self {
        NoisePeriods {
            ld_db: f64::NEG_INFINITY,
            le_db: f64::NEG_INFINITY,
            ln_db: f64::NEG_INFINITY,
            lden_db: f64::NEG_INFINITY,
        }
    }
}

impl Default for NoisePeriods {
    fn default() -> Self {
        Self::silence()
    }
}
